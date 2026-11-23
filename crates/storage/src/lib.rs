//! Write-ahead log, Parquet segments partitioned by project/date,
//! compaction and (later) object-store backends.
//!
//! Layout under the data root:
//!
//! ```text
//! wal/<seq>.jsonl                                        appended + fsynced per batch
//! events/project=<p>/date=<YYYY-MM-DD>/<id>.parquet
//! ```
//!
//! The Hive-style directory names let the query layer prune partitions by
//! project and date without opening files. Partitions are daily: hourly
//! ones multiplied small files (24/day) and made every query slower; the
//! current day is compacted as it grows.
//!
//! Layer: L1 (see `docs/architecture.md`).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array, ArrayRef, RecordBatch, StringBuilder, TimestampMicrosecondBuilder};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use openlens_event::Event;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Filesystem error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Parquet encoding error.
    #[error("parquet: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    /// Arrow error.
    #[error("arrow: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
    /// A project id that is not safe to use as a directory name.
    #[error("invalid project id")]
    InvalidProject,
}

/// Result alias.
pub type Result<T> = std::result::Result<T, StorageError>;

/// Check that a project id is a safe directory name: 1–64 chars of
/// `[A-Za-z0-9_-]`.
pub fn valid_project(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Arrow schema of the Parquet files. `project` and `date` are not
/// in the files: they are the partition directories.
pub fn event_schema() -> SchemaRef {
    let ts = || DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    Arc::new(Schema::new(vec![
        Field::new("uuid", DataType::Utf8, false),
        Field::new("event", DataType::Utf8, false),
        Field::new("distinct_id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, true),
        Field::new("timestamp", ts(), false),
        Field::new("received_at", ts(), false),
        Field::new("url", DataType::Utf8, true),
        Field::new("path", DataType::Utf8, true),
        Field::new("feature", DataType::Utf8, true),
        Field::new("role", DataType::Utf8, true),
        Field::new("account", DataType::Utf8, true),
        Field::new("plan", DataType::Utf8, true),
        Field::new("environment", DataType::Utf8, false),
        Field::new("release", DataType::Utf8, true),
        Field::new("flags", DataType::Utf8, true),
        Field::new("properties", DataType::Utf8, false),
    ]))
}

/// Directory holding all event partitions.
pub fn events_dir(root: &Path) -> PathBuf {
    root.join("events")
}

/// Directory of one project's events.
pub fn project_dir(root: &Path, project: &str) -> Result<PathBuf> {
    if !valid_project(project) {
        return Err(StorageError::InvalidProject);
    }
    Ok(events_dir(root).join(format!("project={project}")))
}

/// When to flush the in-memory buffer to Parquet.
#[derive(Debug, Clone, Copy)]
pub struct FlushPolicy {
    /// Flush when this many events are buffered.
    pub max_events: usize,
    /// Flush when the oldest buffered event is this old.
    pub max_age: std::time::Duration,
}

impl Default for FlushPolicy {
    fn default() -> Self {
        Self {
            max_events: 50_000,
            max_age: std::time::Duration::from_secs(10),
        }
    }
}

/// Event uuids remembered to drop resent events (the SDK retries a failed
/// send with the same uuids). Retries come within minutes; compaction
/// removes any duplicate that gets past this window (a restart, say).
const RECENT_UUIDS: usize = 100_000;

/// The last [`RECENT_UUIDS`] event uuids, oldest first.
#[derive(Default)]
struct Recent {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl Recent {
    /// Remember `uuid`; false if it was already seen.
    fn insert(&mut self, uuid: &str) -> bool {
        if self.set.contains(uuid) {
            return false;
        }
        if self.order.len() >= RECENT_UUIDS
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        self.set.insert(uuid.to_owned());
        self.order.push_back(uuid.to_owned());
        true
    }
}

/// Appends events durably and turns them into Parquet segments.
pub struct Writer {
    recent: Recent,
    root: PathBuf,
    policy: FlushPolicy,
    wal: BufWriter<File>,
    wal_path: PathBuf,
    wal_seq: u64,
    buffer: Vec<Event>,
    oldest: Option<Instant>,
}

impl Writer {
    /// Open the store at `root`. Any WAL left by a previous run is turned
    /// into Parquet first; returns the writer and the number of recovered
    /// events.
    pub fn open(root: impl Into<PathBuf>, policy: FlushPolicy) -> Result<(Self, usize)> {
        let root = root.into();
        let wal_dir = root.join("wal");
        fs::create_dir_all(&wal_dir)?;
        fs::create_dir_all(events_dir(&root))?;

        let mut recovered = Vec::new();
        let mut max_seq = 0;
        let mut old_files = Vec::new();
        for entry in fs::read_dir(&wal_dir)? {
            let path = entry?.path();
            let seq = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u64>().ok());
            let Some(seq) = seq else { continue };
            max_seq = max_seq.max(seq);
            recovered.extend(read_wal(&path)?);
            old_files.push(path);
        }
        let count = recovered.len();
        if count > 0 {
            write_segments(&root, &recovered)?;
            tracing::info!(count, "recovered events from write-ahead log");
        }
        for f in old_files {
            fs::remove_file(f)?;
        }
        let wal_seq = max_seq + 1;
        let (wal, wal_path) = open_wal(&wal_dir, wal_seq)?;
        Ok((
            Self {
                recent: {
                    let mut r = Recent::default();
                    for ev in &recovered {
                        r.insert(&ev.uuid);
                    }
                    r
                },
                root,
                policy,
                wal,
                wal_path,
                wal_seq,
                buffer: Vec::new(),
                oldest: None,
            },
            count,
        ))
    }

    /// Append events. They are on disk (fsynced) when this returns `Ok`.
    /// Events whose uuid was stored recently (a resent batch) are dropped.
    pub fn append(&mut self, events: &[Event]) -> Result<()> {
        let events: Vec<Event> = events
            .iter()
            .filter(|ev| self.recent.insert(&ev.uuid))
            .cloned()
            .collect();
        if events.is_empty() {
            return Ok(());
        }
        for ev in &events {
            serde_json::to_writer(&mut self.wal, ev).map_err(std::io::Error::other)?;
            self.wal.write_all(b"\n")?;
        }
        self.wal.flush()?;
        self.wal.get_ref().sync_data()?;
        self.buffer.extend(events);
        self.oldest.get_or_insert_with(Instant::now);
        Ok(())
    }

    /// Whether the flush policy says it is time to flush.
    pub fn should_flush(&self) -> bool {
        self.buffer.len() >= self.policy.max_events
            || self
                .oldest
                .is_some_and(|t| t.elapsed() >= self.policy.max_age)
    }

    /// Number of buffered (not yet in Parquet) events.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Write buffered events to Parquet and start a fresh WAL. Returns the
    /// number of events flushed.
    pub fn flush(&mut self) -> Result<usize> {
        if self.buffer.is_empty() {
            return Ok(0);
        }
        write_segments(&self.root, &self.buffer)?;
        let n = self.buffer.len();
        self.buffer.clear();
        self.oldest = None;
        // Segments are durable: the WAL that covered them can go.
        let old = self.wal_path.clone();
        self.wal_seq += 1;
        let (wal, path) = open_wal(&self.root.join("wal"), self.wal_seq)?;
        self.wal = wal;
        self.wal_path = path;
        fs::remove_file(old)?;
        Ok(n)
    }
}

fn open_wal(dir: &Path, seq: u64) -> Result<(BufWriter<File>, PathBuf)> {
    let path = dir.join(format!("{seq:020}.jsonl"));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok((BufWriter::new(file), path))
}

/// Read a WAL file, skipping a torn last line (crash mid-write).
fn read_wal(path: &Path) -> Result<Vec<Event>> {
    let reader = BufReader::new(File::open(path)?);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        match serde_json::from_str::<Event>(&line) {
            Ok(ev) => out.push(ev),
            Err(_) => tracing::warn!(path = %path.display(), "skipping unreadable WAL line"),
        }
    }
    Ok(out)
}

type PartitionKey = (String, String);

fn partition_of(ev: &Event) -> PartitionKey {
    (
        ev.project.clone(),
        ev.timestamp.format("%Y-%m-%d").to_string(),
    )
}

fn partition_dir(root: &Path, key: &PartitionKey) -> Result<PathBuf> {
    Ok(project_dir(root, &key.0)?.join(format!("date={}", key.1)))
}

/// The current day is compacted once it holds more segments than this.
pub const TODAY_MAX_SEGMENTS: usize = 24;

/// Group events by partition and write one segment per partition.
fn write_segments(root: &Path, events: &[Event]) -> Result<()> {
    let mut groups: BTreeMap<PartitionKey, Vec<&Event>> = BTreeMap::new();
    for ev in events {
        if !valid_project(&ev.project) {
            tracing::warn!("dropping event with invalid project id");
            continue;
        }
        groups.entry(partition_of(ev)).or_default().push(ev);
    }
    for (key, evs) in groups {
        let dir = partition_dir(root, &key)?;
        write_parquet(&dir, &to_batch(&evs)?)?;
    }
    Ok(())
}

/// Write a batch to `dir/<uuid-v7>.parquet` atomically (temp file + rename).
fn write_parquet(dir: &Path, batch: &RecordBatch) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let name = uuid::Uuid::now_v7().to_string();
    let tmp = dir.join(format!(".{name}.tmp"));
    let dst = dir.join(format!("{name}.parquet"));
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
        .build();
    let file = File::create(&tmp)?;
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
    writer.write(batch)?;
    let file = writer.into_inner()?;
    file.sync_all()?;
    fs::rename(&tmp, &dst)?;
    Ok(dst)
}

fn to_batch(events: &[&Event]) -> Result<RecordBatch> {
    let n = events.len();
    let s = |f: &dyn Fn(&Event) -> Option<&str>| -> ArrayRef {
        let mut b = StringBuilder::with_capacity(n, n * 16);
        for ev in events {
            b.append_option(f(ev));
        }
        Arc::new(b.finish())
    };
    let uuid = s(&|e| Some(&e.uuid));
    let event = s(&|e| Some(&e.event));
    let distinct = s(&|e| Some(&e.distinct_id));
    let session = s(&|e| e.session_id.as_deref());
    let url = s(&|e| e.url.as_deref());
    let path = s(&|e| e.path.as_deref());
    let feature = s(&|e| e.feature.as_deref());
    let role = s(&|e| e.role.as_deref());
    let account = s(&|e| e.account.as_deref());
    let plan = s(&|e| e.plan.as_deref());
    let environment = s(&|e| Some(&e.environment));
    let release = s(&|e| e.release.as_deref());
    let flags = s(&|e| e.flags.as_deref());
    let props = s(&|e| Some(&e.properties));
    let ts = |f: &dyn Fn(&Event) -> DateTime<Utc>| -> ArrayRef {
        let mut b = TimestampMicrosecondBuilder::with_capacity(n).with_timezone("UTC");
        for ev in events {
            b.append_value(f(ev).timestamp_micros());
        }
        Arc::new(b.finish())
    };
    let timestamp = ts(&|e| e.timestamp);
    let received = ts(&|e| e.received_at);
    Ok(RecordBatch::try_new(
        event_schema(),
        vec![
            uuid,
            event,
            distinct,
            session,
            timestamp,
            received,
            url,
            path,
            feature,
            role,
            account,
            plan,
            environment,
            release,
            flags,
            props,
        ],
    )?)
}

/// Merge each day's segments into one file: closed days as soon as they
/// have more than one segment, the current day once it has more than
/// [`TODAY_MAX_SEGMENTS`]. Returns the number of days compacted.
pub fn compact(root: &Path, project: &str, now: DateTime<Utc>) -> Result<usize> {
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(0);
    }
    let today = format!("date={}", now.format("%Y-%m-%d"));
    let mut compacted = 0;
    for date in fs::read_dir(&pdir)? {
        let date = date?.path();
        if !date.is_dir() {
            continue;
        }
        let min = if date.file_name().and_then(|s| s.to_str()) == Some(today.as_str()) {
            TODAY_MAX_SEGMENTS + 1
        } else {
            2
        };
        if compact_dir(&date, min)? {
            compacted += 1;
        }
    }
    Ok(compacted)
}

fn compact_dir(dir: &Path, min_files: usize) -> Result<bool> {
    let files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
        .collect();
    if files.len() < min_files {
        return Ok(false);
    }
    let mut batches = Vec::new();
    for f in &files {
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(f)?)?.build()?;
        for b in reader {
            batches.push(b?);
        }
    }
    let merged = arrow::compute::concat_batches(&event_schema(), &batches)?;
    let merged = unique_uuids(&merged)?;
    write_parquet(dir, &merged)?;
    for f in files {
        fs::remove_file(f)?;
    }
    Ok(true)
}

/// Remove every event of these people from a project's files (#230,
/// right to erasure): each day file holding one is rewritten without them.
/// Returns the number of events removed. The WAL must be flushed first,
/// and nothing else may rewrite the files meanwhile (the caller holds the
/// maintenance lock).
pub fn erase(root: &Path, project: &str, ids: &HashSet<String>) -> Result<usize> {
    let pdir = project_dir(root, project)?;
    if ids.is_empty() || !pdir.is_dir() {
        return Ok(0);
    }
    let mut removed = 0;
    for date in fs::read_dir(&pdir)? {
        let date = date?.path();
        if !date.is_dir() {
            continue;
        }
        let files: Vec<PathBuf> = fs::read_dir(&date)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
            .collect();
        for f in files {
            let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&f)?)?.build()?;
            let mut kept = Vec::new();
            let mut gone = 0;
            for b in reader {
                let b = b?;
                let Some(col) = b
                    .column_by_name("distinct_id")
                    .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
                else {
                    kept.push(b);
                    continue;
                };
                let keep: arrow::array::BooleanArray = (0..col.len())
                    .map(|i| Some(col.is_null(i) || !ids.contains(col.value(i))))
                    .collect();
                gone += b.num_rows() - keep.true_count();
                kept.push(arrow::compute::filter_record_batch(&b, &keep)?);
            }
            if gone == 0 {
                continue;
            }
            removed += gone;
            let merged = arrow::compute::concat_batches(&event_schema(), &kept)?;
            if merged.num_rows() > 0 {
                write_parquet(&date, &merged)?;
            }
            fs::remove_file(&f)?;
        }
    }
    Ok(removed)
}

/// Keep the first row of each uuid (a resent event is stored once).
fn unique_uuids(batch: &RecordBatch) -> Result<RecordBatch> {
    let Some(col) = batch
        .column_by_name("uuid")
        .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
    else {
        return Ok(batch.clone());
    };
    let mut seen = HashSet::with_capacity(col.len());
    let keep: arrow::array::BooleanArray = (0..col.len())
        .map(|i| Some(col.is_null(i) || seen.insert(col.value(i))))
        .collect();
    if keep.true_count() == batch.num_rows() {
        return Ok(batch.clone());
    }
    Ok(arrow::compute::filter_record_batch(batch, &keep)?)
}

/// Days of events kept: 400, a little over 13 months, so a month can be
/// compared with the same month a year earlier. Fixed by design (D-015).
pub const EVENT_RETENTION_DAYS: i64 = 400;

/// Delete a project's day partitions older than [`EVENT_RETENTION_DAYS`].
/// Returns the number of days removed.
pub fn prune(root: &Path, project: &str, now: DateTime<Utc>) -> Result<usize> {
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(0);
    }
    let cutoff = format!(
        "date={}",
        (now - chrono::Duration::days(EVENT_RETENTION_DAYS)).format("%Y-%m-%d")
    );
    let mut removed = 0;
    for entry in fs::read_dir(&pdir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        // `date=YYYY-MM-DD` sorts like the date it names.
        if path.is_dir() && name.starts_with("date=") && name.len() == 15 && name < cutoff.as_str()
        {
            fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Count rows in all segments of a project (used by tests and `verify`).
pub fn count_rows(root: &Path, project: &str) -> Result<usize> {
    let mut total = 0;
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(0);
    }
    let mut stack = vec![pdir];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let p = entry?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "parquet") {
                let meta = ParquetRecordBatchReaderBuilder::try_new(File::open(&p)?)?;
                total += usize::try_from(meta.metadata().file_metadata().num_rows()).unwrap_or(0);
            }
        }
    }
    Ok(total)
}

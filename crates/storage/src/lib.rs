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

mod compaction;
mod pending;
mod segments;
pub use pending::Pending;
use pending::projects_of;
pub use segments::{SegmentFacts, count_rows, segment_facts, segment_files};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
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

pub mod rollup;

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

/// Group commit for the WAL (#333). Appends write under the writer's lock
/// and return a ticket; [`Durable::wait`] then makes the ticket durable
/// *outside* the lock. One `fsync` covers every write made before it
/// started: the first waiter syncs, the ones arriving meanwhile wait for
/// the next sync, which covers them all. A lone request syncs at once.
pub struct Durable {
    state: Mutex<SyncState>,
    done: Condvar,
}

struct SyncState {
    /// The WAL being appended to (a second handle on the same file).
    file: Arc<File>,
    /// Writes made so far (ticket numbers).
    written: u64,
    /// Every ticket up to this one is on disk.
    synced: u64,
    /// A sync is running.
    syncing: bool,
    /// `fsync` calls made, for `/metrics`.
    syncs: u64,
}

impl Durable {
    fn new(file: &File) -> Result<Self> {
        Ok(Self {
            state: Mutex::new(SyncState {
                file: Arc::new(file.try_clone()?),
                written: 0,
                synced: 0,
                syncing: false,
                syncs: 0,
            }),
            done: Condvar::new(),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, SyncState>> {
        self.state
            .lock()
            .map_err(|_| StorageError::Io(std::io::Error::other("WAL sync state poisoned")))
    }

    /// Return once every write up to `ticket` is on disk.
    pub fn wait(&self, ticket: u64) -> Result<()> {
        let mut s = self.lock()?;
        loop {
            if s.synced >= ticket {
                return Ok(());
            }
            if !s.syncing {
                break;
            }
            s = self
                .done
                .wait(s)
                .map_err(|_| StorageError::Io(std::io::Error::other("WAL sync state poisoned")))?;
        }
        s.syncing = true;
        s.syncs += 1;
        let (file, target) = (s.file.clone(), s.written);
        drop(s);
        let synced = file.sync_data();
        let mut s = self.lock()?;
        s.syncing = false;
        if synced.is_ok() {
            s.synced = s.synced.max(target);
        }
        drop(s);
        self.done.notify_all();
        Ok(synced?)
    }

    /// How many `fsync` calls the WAL took since the start.
    pub fn syncs(&self) -> u64 {
        self.lock().map_or(0, |s| s.syncs)
    }
}

/// Appends events durably and turns them into Parquet segments.
pub struct Writer {
    durable: Arc<Durable>,
    recent: Recent,
    root: PathBuf,
    policy: FlushPolicy,
    wal: BufWriter<File>,
    wal_path: PathBuf,
    wal_seq: u64,
    buffer: Vec<Event>,
    oldest: Option<Instant>,
    /// WAL files of a [`Pending`] whose write failed: removed by the next one.
    stale_wals: Vec<PathBuf>,
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
        sync_dir(&wal_dir)?;
        let durable = Arc::new(Durable::new(wal.get_ref())?);
        Ok((
            Self {
                durable,
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
                stale_wals: Vec::new(),
            },
            count,
        ))
    }

    /// Append events. They are on disk (fsynced) when this returns `Ok`.
    /// Events whose uuid was stored recently (a resent batch) are dropped.
    pub fn append(&mut self, events: &[Event]) -> Result<()> {
        let ticket = self.write(events)?;
        self.durable.wait(ticket)
    }

    /// The first half of [`Writer::append`], to call under the writer's
    /// lock: write the events to the WAL (not yet fsynced) and return a
    /// ticket for [`Durable::wait`], to call after releasing the lock.
    ///
    /// A resent batch whose events are all dropped as duplicates still
    /// gets a ticket covering every write so far, so its 200 waits until
    /// the first copy is on disk.
    pub fn write(&mut self, events: &[Event]) -> Result<u64> {
        let mut batch = HashSet::new();
        let fresh: Vec<Event> = events
            .iter()
            .filter(|ev| !self.recent.set.contains(&ev.uuid) && batch.insert(ev.uuid.as_str()))
            .cloned()
            .collect();
        if !fresh.is_empty() {
            for ev in &fresh {
                serde_json::to_writer(&mut self.wal, ev).map_err(std::io::Error::other)?;
                self.wal.write_all(b"\n")?;
            }
            self.wal.flush()?;
            // Only once written: a failed write leaves the uuids unknown,
            // so the client's retry is stored rather than dropped.
            for ev in &fresh {
                self.recent.insert(&ev.uuid);
            }
            self.buffer.extend(fresh);
            self.oldest.get_or_insert_with(Instant::now);
            self.durable.lock()?.written += 1;
        }
        Ok(self.durable.lock()?.written)
    }

    /// The WAL's group commit, to wait on outside the writer's lock.
    pub fn durable(&self) -> Arc<Durable> {
        self.durable.clone()
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

    /// The distinct projects of the events buffered now (#420: a flush
    /// invalidates only these projects' cached boards).
    pub fn buffered_projects(&self) -> std::collections::BTreeSet<String> {
        projects_of(&self.buffer)
    }

    /// Write buffered events to Parquet and start a fresh WAL. Returns the
    /// number of events flushed.
    pub fn flush(&mut self) -> Result<usize> {
        let Some(pending) = self.take()? else {
            return Ok(0);
        };
        pending
            .write()
            .inspect_err(|_| self.restore(pending.clone()))
    }

    /// Give back the events of a [`Pending`] whose write failed, so the next
    /// flush tries again (and removes its WAL then).
    pub fn restore(&mut self, pending: Pending) {
        let mut events = pending.events;
        events.append(&mut self.buffer);
        self.buffer = events;
        self.oldest.get_or_insert_with(Instant::now);
        self.stale_wals.extend(pending.old_wals);
    }

    /// The first half of a flush, cheap enough to hold the writer's lock
    /// for (#293): take the buffered events and start a fresh WAL. Appends
    /// can go on while [`Pending::write`] turns them into Parquet. If the
    /// process dies in between, the old WAL is still there and is replayed
    /// at the next start.
    pub fn take(&mut self) -> Result<Option<Pending>> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        let wal_dir = self.root.join("wal");
        let (wal, path) = open_wal(&wal_dir, self.wal_seq + 1)?;
        // Every write so far goes to disk with the old WAL, so the tickets
        // that point to it are covered before syncs move to the new one.
        self.wal.flush()?;
        self.wal.get_ref().sync_data()?;
        {
            let mut s = self.durable.lock()?;
            s.file = Arc::new(wal.get_ref().try_clone()?);
            s.synced = s.written;
            s.syncs += 1;
        }
        self.durable.done.notify_all();
        self.wal_seq += 1;
        let mut old_wals = std::mem::take(&mut self.stale_wals);
        old_wals.push(std::mem::replace(&mut self.wal_path, path));
        self.wal = wal;
        self.oldest = None;
        Ok(Some(Pending {
            root: self.root.clone(),
            events: std::mem::take(&mut self.buffer),
            old_wals,
        }))
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
        // Sorted as compaction sorts, so compaction is a streaming merge (#418).
        write_parquet(&dir, &sorted(&to_batch(&evs)?)?)?;
    }
    Ok(())
}

/// Make a directory's entries durable: a `rename` or a removal survives a
/// power cut only once its directory is synced (#292). No-op where
/// directories cannot be opened as files (Windows).
pub fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Write a batch to `dir/<uuid-v7>.parquet` atomically (temp file + rename),
/// durable on return: the file, its directory entry, and a directory just
/// created in its parent.
fn write_parquet(dir: &Path, batch: &RecordBatch) -> Result<PathBuf> {
    write_parquet_as(dir, &uuid::Uuid::now_v7().to_string(), batch)
}

/// Write a batch to `dir/<name>.parquet` atomically, replacing any file of
/// that name, with the same durability as [`write_parquet`].
pub(crate) fn write_parquet_as(dir: &Path, name: &str, batch: &RecordBatch) -> Result<PathBuf> {
    let created = !dir.is_dir();
    fs::create_dir_all(dir)?;
    if created && let Some(parent) = dir.parent() {
        sync_dir(parent)?;
    }
    let tmp = dir.join(format!(".{name}.tmp"));
    let dst = dir.join(format!("{name}.parquet"));
    let file = File::create(&tmp)?;
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(writer_props()?))?;
    writer.write(batch)?;
    let file = writer.into_inner()?;
    file.sync_all()?;
    fs::rename(&tmp, &dst)?;
    sync_dir(dir)?;
    Ok(dst)
}

/// How every segment is written. Row groups of 64k rows (not 1M) with
/// page statistics: a query on one event or a short time range reads only
/// the groups that hold it (#326).
fn writer_props() -> Result<WriterProperties> {
    Ok(WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
        .set_max_row_group_row_count(Some(64 * 1024))
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
        .build())
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
        if compaction::compact_dir(&date, min)? {
            compacted += 1;
        }
    }
    Ok(compacted)
}

/// Remove every event of these people from a project's files (#230,
/// right to erasure): each day file holding one is rewritten without them.
/// Returns the number of events removed. The WAL must be flushed first,
/// and nothing else may rewrite the files meanwhile (the caller holds the
/// maintenance lock).
pub fn erase(root: &Path, project: &str, ids: &HashSet<String>) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let removed = rewrite_files(root, project, |b| {
        let Some(col) = b
            .column_by_name("distinct_id")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
        else {
            return Ok((b.clone(), 0));
        };
        let keep: arrow::array::BooleanArray = (0..col.len())
            .map(|i| Some(col.is_null(i) || !ids.contains(col.value(i))))
            .collect();
        let gone = b.num_rows() - keep.true_count();
        Ok((arrow::compute::filter_record_batch(b, &keep)?, gone))
    })?;
    // The rollups hold these people too, also past retention (D-026).
    let days = rollup::erase(root, project, ids)?;
    if removed > 0 || days > 0 {
        rollup::bump();
    }
    Ok(removed)
}

/// [`erase`] for one day only (a day brought back from the cold tier,
/// #326). Returns the events removed.
pub fn erase_day(
    root: &Path,
    project: &str,
    day: chrono::NaiveDate,
    ids: &HashSet<String>,
) -> Result<usize> {
    let dir = project_dir(root, project)?.join(format!("date={}", day.format("%Y-%m-%d")));
    if ids.is_empty() || !dir.is_dir() {
        return Ok(0);
    }
    let removed = rewrite_day(&dir, &mut |b: &RecordBatch| {
        let Some(col) = b
            .column_by_name("distinct_id")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>())
        else {
            return Ok((b.clone(), 0));
        };
        let keep: arrow::array::BooleanArray = (0..col.len())
            .map(|i| Some(col.is_null(i) || !ids.contains(col.value(i))))
            .collect();
        let gone = b.num_rows() - keep.true_count();
        Ok((arrow::compute::filter_record_batch(b, &keep)?, gone))
    })?;
    if removed > 0 {
        rollup::bump();
    }
    Ok(removed)
}

/// Clean the page URL of every stored event with
/// [`openlens_event::clean_url`] (#290): files written before URLs were
/// cleaned at capture still hold query strings. Returns the number of
/// URLs changed. Same preconditions as [`erase`].
pub fn clean_urls(root: &Path, project: &str) -> Result<usize> {
    rewrite_files(root, project, |b| {
        let Some((idx, col)) = b.schema().index_of("url").ok().and_then(|i| {
            b.column(i)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .map(|c| (i, c.clone()))
        }) else {
            return Ok((b.clone(), 0));
        };
        let mut changed = 0;
        let cleaned: arrow::array::StringArray = col
            .iter()
            .map(|v| {
                v.map(|u| {
                    let c = openlens_event::clean_url(u);
                    if c != u {
                        changed += 1;
                    }
                    c
                })
            })
            .collect();
        if changed == 0 {
            return Ok((b.clone(), 0));
        }
        let mut cols = b.columns().to_vec();
        cols[idx] = Arc::new(cleaned) as ArrayRef;
        Ok((RecordBatch::try_new(b.schema(), cols)?, changed))
    })
}

/// Rewrite each day file of a project through `f`, which returns the new
/// batch and how many rows it changed. A file is rewritten only when
/// something changed, and deleted when nothing is left. Returns the total.
fn rewrite_files(
    root: &Path,
    project: &str,
    mut f: impl FnMut(&RecordBatch) -> Result<(RecordBatch, usize)>,
) -> Result<usize> {
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(0);
    }
    let mut total = 0;
    for date in fs::read_dir(&pdir)? {
        let date = date?.path();
        if date.is_dir() {
            total += rewrite_day(&date, &mut f)?;
        }
    }
    Ok(total)
}

/// [`rewrite_files`] for one day directory.
fn rewrite_day(
    date: &Path,
    f: &mut impl FnMut(&RecordBatch) -> Result<(RecordBatch, usize)>,
) -> Result<usize> {
    let mut total = 0;
    let files: Vec<PathBuf> = fs::read_dir(date)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
        .collect();
    for file in files {
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&file)?)?.build()?;
        let mut out = Vec::new();
        let mut changed = 0;
        for b in reader {
            let (nb, n) = f(&b?)?;
            changed += n;
            out.push(nb);
        }
        if changed == 0 {
            continue;
        }
        total += changed;
        let merged = arrow::compute::concat_batches(&event_schema(), &out)?;
        if merged.num_rows() > 0 {
            write_parquet(date, &merged)?;
        }
        fs::remove_file(&file)?;
        sync_dir(date)?;
    }
    Ok(total)
}

/// Days a raw click keeps its label and position (#324). After that the
/// day's `click_q`, `label_q` and `scroll_q` rollups hold them; the event
/// row and `$rage` stay.
pub const CLICK_DETAIL_DAYS: i64 = 30;

/// Click properties dropped after [`CLICK_DETAIL_DAYS`].
const CLICK_DETAIL: [&str; 7] = ["$text", "$tag", "$id", "$href", "$x", "$y", "$vw"];

const TRIMMED: &str = ".clicks-trimmed";

/// Drop the label and position of `$click` events older than
/// [`CLICK_DETAIL_DAYS`], once the day has its rollups. Returns the clicks
/// trimmed. A day is read again only when its files changed. Same
/// preconditions as [`erase`].
pub fn trim_clicks(root: &Path, project: &str, now: DateTime<Utc>) -> Result<usize> {
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(0);
    }
    let cutoff = (now - chrono::Duration::days(CLICK_DETAIL_DAYS)).date_naive();
    let mut total = 0;
    for entry in fs::read_dir(&pdir)? {
        let date = entry?.path();
        let Some(day) = date
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|n| n.strip_prefix("date="))
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if !date.is_dir() || day >= cutoff || !rollup::has(root, project, day)? {
            continue;
        }
        let listing = || -> Result<String> {
            let mut names: Vec<String> = fs::read_dir(&date)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".parquet"))
                .collect();
            names.sort();
            Ok(names.join("\n"))
        };
        if fs::read_to_string(date.join(TRIMMED)).ok() == Some(listing()?) {
            continue;
        }
        total += rewrite_day(&date, &mut trim_batch)?;
        fs::write(date.join(TRIMMED), listing()?)?;
    }
    Ok(total)
}

/// `batch` with the click detail removed from `$click` properties.
fn trim_batch(b: &RecordBatch) -> Result<(RecordBatch, usize)> {
    let (Some(ev), Some((idx, props))) = (
        b.column_by_name("event")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>()),
        b.schema().index_of("properties").ok().and_then(|i| {
            b.column(i)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .map(|c| (i, c.clone()))
        }),
    ) else {
        return Ok((b.clone(), 0));
    };
    let mut changed = 0;
    let out: arrow::array::StringArray = (0..b.num_rows())
        .map(|i| {
            if props.is_null(i) {
                return None;
            }
            let raw = props.value(i);
            if ev.is_null(i) || ev.value(i) != "$click" {
                return Some(raw.to_owned());
            }
            let Ok(serde_json::Value::Object(mut m)) = serde_json::from_str(raw) else {
                return Some(raw.to_owned());
            };
            let before = m.len();
            for k in CLICK_DETAIL {
                m.remove(k);
            }
            if m.len() == before {
                return Some(raw.to_owned());
            }
            changed += 1;
            Some(serde_json::Value::Object(m).to_string())
        })
        .collect();
    if changed == 0 {
        return Ok((b.clone(), 0));
    }
    let mut cols = b.columns().to_vec();
    cols[idx] = Arc::new(out) as ArrayRef;
    Ok((RecordBatch::try_new(b.schema(), cols)?, changed))
}

/// `batch` ordered by event then time (#326): each row group then covers a
/// few event names and a short span, so its statistics let a query skip it.
fn sorted(batch: &RecordBatch) -> Result<RecordBatch> {
    let (Some(ev), Some(ts)) = (
        batch.column_by_name("event"),
        batch.column_by_name("timestamp"),
    ) else {
        return Ok(batch.clone());
    };
    let idx = arrow::compute::lexsort_to_indices(
        &[
            arrow::compute::SortColumn {
                values: ev.clone(),
                options: None,
            },
            arrow::compute::SortColumn {
                values: ts.clone(),
                options: None,
            },
        ],
        None,
    )?;
    Ok(arrow::compute::take_record_batch(batch, &idx)?)
}

/// Days of events kept: 400, a little over 13 months, so a month can be
/// compared with the same month a year earlier. Fixed by design (D-015).
pub const EVENT_RETENTION_DAYS: i64 = 400;

/// Delete a project's day partitions older than [`EVENT_RETENTION_DAYS`].
/// Returns the number of days removed.
pub fn prune(root: &Path, project: &str, now: DateTime<Utc>) -> Result<usize> {
    let pdir = project_dir(root, project)?;
    let cutoff = format!(
        "date={}",
        (now - chrono::Duration::days(EVENT_RETENTION_DAYS)).format("%Y-%m-%d")
    );
    // Rollups stay: they hold counts for looking further back (D-026).
    if !pdir.is_dir() {
        return Ok(0);
    }
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

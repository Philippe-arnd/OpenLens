//! Compaction of one day as a streaming merge (#418, part 3).
//!
//! Compaction used to load every segment of the day, concatenate them, drop
//! repeated uuids and sort the result: four copies of the day in memory, so
//! a day of 4 M events held more than a gigabyte. Segments are now sorted
//! by (event, timestamp) when they are written, so the day is a k-way merge
//! of sorted inputs, written through one Parquet writer in row groups: about
//! one batch per input stays in memory, whatever the size of the day.
//!
//! The output is the same as before: every row, sorted by (event,
//! timestamp), one per uuid (a resent event is stored once).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use arrow::array::{Array, RecordBatch, StringArray, TimestampMicrosecondArray};
use parquet::arrow::ArrowWriter;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};

use crate::{Result, StorageError, event_schema, sorted, sync_dir, write_parquet, writer_props};

/// Rows read from each input at a time.
const READ_ROWS: usize = 8192;
/// Rows per batch handed to the writer (one row group each).
const WRITE_ROWS: usize = 64 * 1024;

/// Merge the Parquet files of `dir` into one when it holds at least
/// `min_files`. Returns whether it did.
pub(crate) fn compact_dir(dir: &Path, min_files: usize) -> Result<bool> {
    let files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
        .collect();
    if files.len() < min_files {
        return Ok(false);
    }
    // Segments written before #418 part 3 were not sorted: sort each on its
    // own first (one small file at a time).
    let mut inputs = Vec::with_capacity(files.len());
    for f in files {
        if is_sorted(&f)? {
            inputs.push(f);
        } else {
            let batch = read_all(&f)?;
            let new = write_parquet(dir, &sorted(&batch)?)?;
            fs::remove_file(&f)?;
            sync_dir(dir)?;
            inputs.push(new);
        }
    }
    let name = uuid::Uuid::now_v7().to_string();
    let tmp = dir.join(format!(".{name}.tmp"));
    let dst = dir.join(format!("{name}.parquet"));
    if let Err(e) = merge(&inputs, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // The merged segment is durable before the old ones go.
    fs::rename(&tmp, &dst)?;
    sync_dir(dir)?;
    for f in inputs {
        fs::remove_file(f)?;
    }
    sync_dir(dir)?;
    Ok(true)
}

/// Every row of one (small) file.
fn read_all(path: &Path) -> Result<RecordBatch> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?.build()?;
    let batches = reader.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(arrow::compute::concat_batches(&event_schema(), &batches)?)
}

/// Whether a file's rows are in (event, timestamp) order, reading only
/// those two columns.
fn is_sorted(path: &Path) -> Result<bool> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    let mask = ProjectionMask::columns(builder.parquet_schema(), ["event", "timestamp"]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(READ_ROWS)
        .build()?;
    // The last key of the previous batch.
    let mut last: Option<(String, i64)> = None;
    for b in reader {
        let b = b?;
        let (Some(ev), Some(ts)) = (strings(&b, "event"), times(&b, "timestamp")) else {
            return Ok(false);
        };
        let n = b.num_rows();
        for i in 0..n {
            let here = (ev.value(i), ts.value(i));
            let before = match i {
                0 => last.as_ref().map(|(e, t)| (e.as_str(), *t)),
                _ => Some((ev.value(i - 1), ts.value(i - 1))),
            };
            if before.is_some_and(|b| b > here) {
                return Ok(false);
            }
        }
        if n > 0 {
            last = Some((ev.value(n - 1).to_owned(), ts.value(n - 1)));
        }
    }
    Ok(true)
}

fn strings<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}

fn times<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a TimestampMicrosecondArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}

/// One sorted input and the batch it is at.
struct Cursor {
    reader: ParquetRecordBatchReader,
    /// Index of the current batch in the merge's `held` list.
    held: usize,
    row: usize,
    rows: usize,
}

/// A uuid, as 16 bytes when it parses, else as its text.
#[derive(Hash, PartialEq, Eq)]
enum Seen {
    Id(u128),
    Text(String),
}

fn seen_key(s: &str) -> Seen {
    match uuid::Uuid::parse_str(s) {
        Ok(u) => Seen::Id(u.as_u128()),
        Err(_) => Seen::Text(s.to_owned()),
    }
}

/// Merge sorted `inputs` into the Parquet file `out`.
fn merge(inputs: &[PathBuf], out: &Path) -> Result<()> {
    let schema = event_schema();
    let mut writer =
        ArrowWriter::try_new(File::create(out)?, schema.clone(), Some(writer_props()?))?;
    // Batches still referenced by a cursor or by rows waiting to be written.
    let mut held: Vec<RecordBatch> = Vec::new();
    let mut cursors: Vec<Cursor> = Vec::new();
    let mut heap: BinaryHeap<Reverse<(String, i64, usize)>> = BinaryHeap::new();
    for path in inputs {
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?
            .with_batch_size(READ_ROWS)
            .build()?;
        let mut c = Cursor {
            reader,
            held: 0,
            row: 0,
            rows: 0,
        };
        if advance(&mut c, &mut held)? {
            let (e, t) = key(&held, c.held, 0)?;
            heap.push(Reverse((e, t, cursors.len())));
            cursors.push(c);
        }
    }
    let mut seen: HashSet<Seen> = HashSet::new();
    let mut pending: Vec<(usize, usize)> = Vec::with_capacity(WRITE_ROWS);
    while let Some(Reverse((_, _, i))) = heap.pop() {
        let Some(c) = cursors.get_mut(i) else {
            return Err(StorageError::Io(std::io::Error::other("merge cursor")));
        };
        let uuid = held
            .get(c.held)
            .and_then(|b| strings(b, "uuid"))
            .map(|u| (u.is_null(c.row), u.value(c.row)));
        let keep = match uuid {
            Some((false, u)) => seen.insert(seen_key(u)),
            _ => true,
        };
        if keep {
            pending.push((c.held, c.row));
        }
        c.row += 1;
        let more = c.row < c.rows || advance(c, &mut held)?;
        if more {
            let (e, t) = key(&held, c.held, c.row)?;
            heap.push(Reverse((e, t, i)));
        }
        if pending.len() >= WRITE_ROWS {
            flush(&mut writer, &mut held, &mut cursors, &mut pending)?;
        }
    }
    flush(&mut writer, &mut held, &mut cursors, &mut pending)?;
    let file = writer.into_inner()?;
    file.sync_all()?;
    Ok(())
}

/// Load the cursor's next non-empty batch; false at the end of its file.
fn advance(c: &mut Cursor, held: &mut Vec<RecordBatch>) -> Result<bool> {
    for b in c.reader.by_ref() {
        let b = b?;
        if b.num_rows() > 0 {
            c.rows = b.num_rows();
            c.row = 0;
            c.held = held.len();
            held.push(b);
            return Ok(true);
        }
    }
    Ok(false)
}

/// The sort key of row `i` of held batch `at`.
fn key(held: &[RecordBatch], at: usize, i: usize) -> Result<(String, i64)> {
    let Some(b) = held.get(at) else {
        return Err(StorageError::Io(std::io::Error::other("merge batch")));
    };
    match (strings(b, "event"), times(b, "timestamp")) {
        (Some(e), Some(t)) => Ok((e.value(i).to_owned(), t.value(i))),
        _ => Err(StorageError::Io(std::io::Error::other(
            "segment without event or timestamp",
        ))),
    }
}

/// Write the pending rows, then keep only the batches cursors still use.
fn flush(
    writer: &mut ArrowWriter<File>,
    held: &mut Vec<RecordBatch>,
    cursors: &mut [Cursor],
    pending: &mut Vec<(usize, usize)>,
) -> Result<()> {
    if !pending.is_empty() {
        let refs: Vec<&RecordBatch> = held.iter().collect();
        let out = arrow::compute::interleave_record_batch(&refs, pending)?;
        writer.write(&out)?;
        pending.clear();
    }
    let mut kept = Vec::with_capacity(cursors.len());
    for c in cursors.iter_mut() {
        if let Some(b) = held.get(c.held) {
            kept.push(b.clone());
            c.held = kept.len() - 1;
        }
    }
    *held = kept;
    Ok(())
}

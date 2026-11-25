//! Streaming merge of a day's segments (#418).
//!
//! Compaction used to load the whole day, then copy it three times
//! (`concat_batches`, de-duplication, sort): a 4 M-event day held over 1 GB.
//! Now every segment is a sorted run read a batch at a time, the runs are
//! merged by (event, time), a resent uuid is dropped on the way, and the
//! result goes straight to the Parquet writer. What stays in memory is one
//! batch per run, one output batch, one row group being written and 16
//! bytes per distinct uuid of the day.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fs::File;
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};

use arrow::array::{Array, RecordBatch, RecordBatchReader, StringArray, TimestampMicrosecondArray};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::{Result, event_schema, write_parquet_stream};

/// Rows per input batch read from a segment.
const READ_ROWS: usize = 8192;
/// Rows per merged batch handed to the writer.
const OUT_ROWS: usize = 16 * 1024;

type Batches = Box<dyn Iterator<Item = Result<RecordBatch>>>;
/// What the files are ordered by: event name, then time. A null sorts first.
type Key = (Option<String>, Option<i64>);

/// Write one file in `dir` holding every row of `files`, ordered by event
/// then time (#326), each uuid once. Returns its path. The inputs are left
/// in place: the caller removes them once the result is durable.
pub(crate) fn merge_segments(dir: &Path, files: &[PathBuf]) -> Result<PathBuf> {
    let mut runs = Vec::with_capacity(files.len());
    for f in files {
        runs.push(Run::new(open_run(f)?)?);
    }
    let merge = Merge::new(runs);
    write_parquet_stream(
        dir,
        &uuid::Uuid::now_v7().to_string(),
        event_schema(),
        merge,
    )
}

/// A segment as a stream of batches ordered by event then time. A segment
/// the writer flushed straight from the log is not ordered: it is sorted in
/// memory, which is bounded by the flush size. A compacted one is streamed.
fn open_run(path: &Path) -> Result<Batches> {
    let ordered = is_ordered(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?
        .with_batch_size(READ_ROWS)
        .build()?;
    if ordered {
        return Ok(Box::new(reader.map(|b| Ok(b?))));
    }
    let schema = reader.schema();
    let mut batches = Vec::new();
    for b in reader {
        batches.push(b?);
    }
    let all = arrow::compute::concat_batches(&schema, &batches)?;
    drop(batches);
    Ok(Box::new(std::iter::once(sorted(&all))))
}

fn event_col(b: &RecordBatch) -> Option<StringArray> {
    b.column_by_name("event")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned())
}

fn time_col(b: &RecordBatch) -> Option<TimestampMicrosecondArray> {
    b.column_by_name("timestamp").and_then(|c| {
        c.as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .cloned()
    })
}

fn key_at<'a>(
    ev: Option<&'a StringArray>,
    ts: Option<&TimestampMicrosecondArray>,
    i: usize,
) -> (Option<&'a str>, Option<i64>) {
    (
        ev.filter(|a| a.is_valid(i)).map(|a| a.value(i)),
        ts.filter(|a| a.is_valid(i)).map(|a| a.value(i)),
    )
}

/// Whether a segment is already ordered by event then time. Reads only
/// those two columns.
fn is_ordered(path: &Path) -> Result<bool> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    let mask = ProjectionMask::columns(builder.parquet_schema(), ["event", "timestamp"]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(READ_ROWS)
        .build()?;
    let mut last: Option<Key> = None;
    for b in reader {
        let b = b?;
        let (ev, ts) = (event_col(&b), time_col(&b));
        let n = b.num_rows();
        if n == 0 {
            continue;
        }
        let first = key_at(ev.as_ref(), ts.as_ref(), 0);
        if let Some((e, t)) = &last
            && (e.as_deref(), *t) > first
        {
            return Ok(false);
        }
        for i in 1..n {
            if key_at(ev.as_ref(), ts.as_ref(), i - 1) > key_at(ev.as_ref(), ts.as_ref(), i) {
                return Ok(false);
            }
        }
        let (e, t) = key_at(ev.as_ref(), ts.as_ref(), n - 1);
        last = Some((e.map(str::to_owned), t));
    }
    Ok(true)
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

/// The batch a run is reading, with its columns already downcast.
struct Cur {
    batch: RecordBatch,
    uuid: Option<StringArray>,
    ev: Option<StringArray>,
    ts: Option<TimestampMicrosecondArray>,
    row: usize,
    /// Index of `batch` in the output batch being built, once a row of it
    /// was picked.
    slot: Option<usize>,
}

struct Run {
    rest: Batches,
    cur: Option<Cur>,
}

impl Run {
    fn new(rest: Batches) -> Result<Self> {
        let mut run = Run { rest, cur: None };
        run.refill()?;
        Ok(run)
    }

    /// Take the next non-empty batch, if any.
    fn refill(&mut self) -> Result<()> {
        self.cur = None;
        for b in self.rest.by_ref() {
            let batch = b?;
            if batch.num_rows() == 0 {
                continue;
            }
            self.cur = Some(Cur {
                uuid: batch
                    .column_by_name("uuid")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned()),
                ev: event_col(&batch),
                ts: time_col(&batch),
                batch,
                row: 0,
                slot: None,
            });
            break;
        }
        Ok(())
    }

    fn key(&self) -> Option<Key> {
        let c = self.cur.as_ref()?;
        let (e, t) = key_at(c.ev.as_ref(), c.ts.as_ref(), c.row);
        Some((e.map(str::to_owned), t))
    }
}

struct Merge {
    runs: Vec<Run>,
    /// The head key of every run that has rows left; the run index breaks
    /// ties so the order is stable.
    heap: BinaryHeap<Reverse<(Key, usize)>>,
    /// 128-bit hashes of the uuids written so far. Two independently keyed
    /// 64-bit hashes make a collision, which would drop a real event,
    /// negligible at any day's size.
    seen: HashSet<u128>,
    hashers: (RandomState, RandomState),
    failed: bool,
}

impl Merge {
    fn new(runs: Vec<Run>) -> Self {
        let heap = runs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.key().map(|k| Reverse((k, i))))
            .collect();
        Merge {
            runs,
            heap,
            seen: HashSet::new(),
            hashers: (RandomState::new(), RandomState::new()),
            failed: false,
        }
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let mut used: Vec<RecordBatch> = Vec::new();
        for c in self.runs.iter_mut().filter_map(|r| r.cur.as_mut()) {
            c.slot = None;
        }
        let mut picks: Vec<(usize, usize)> = Vec::new();
        while picks.len() < OUT_ROWS {
            let Some(Reverse((_, r))) = self.heap.pop() else {
                break;
            };
            let Some(cur) = self.runs[r].cur.as_mut() else {
                continue;
            };
            let row = cur.row;
            // A row without a uuid is always kept.
            let fresh = match cur.uuid.as_ref().filter(|a| a.is_valid(row)) {
                None => true,
                Some(a) => {
                    let u = a.value(row);
                    let (h0, h1) = (self.hashers.0.hash_one(u), self.hashers.1.hash_one(u));
                    self.seen.insert((u128::from(h0) << 64) | u128::from(h1))
                }
            };
            if fresh {
                let slot = *cur.slot.get_or_insert_with(|| {
                    used.push(cur.batch.clone());
                    used.len() - 1
                });
                picks.push((slot, row));
            }
            let run = &mut self.runs[r];
            if let Some(cur) = run.cur.as_mut() {
                cur.row += 1;
                if cur.row >= cur.batch.num_rows() {
                    run.refill()?;
                }
            }
            if let Some(k) = self.runs[r].key() {
                self.heap.push(Reverse((k, r)));
            }
        }
        if picks.is_empty() {
            return Ok(None);
        }
        let refs: Vec<&RecordBatch> = used.iter().collect();
        Ok(Some(arrow::compute::interleave_record_batch(
            &refs, &picks,
        )?))
    }
}

impl Iterator for Merge {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_batch() {
            Ok(b) => b.map(Ok),
            Err(e) => {
                self.failed = true;
                Some(Err(e))
            }
        }
    }
}

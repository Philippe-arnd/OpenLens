//! Facts read from segment files: file lists, row counts, environments (#447), moved out of lib.rs (#307).

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::{Result, project_dir};

/// Every segment file of a project.
pub fn segment_files(root: &Path, project: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(files);
    }
    let mut stack = vec![pdir];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let p = entry?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "parquet") {
                files.push(p);
            }
        }
    }
    Ok(files)
}

/// What one segment holds that the board shows: its row count (from the
/// footer) and the environments of its rows (from the `environment`
/// column alone, which is dictionary-encoded and small).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentFacts {
    /// Rows in the segment.
    pub rows: usize,
    /// Distinct environments in the segment.
    pub environments: std::collections::BTreeSet<String>,
}

/// Read the [`SegmentFacts`] of one segment file.
pub fn segment_facts(path: &Path) -> Result<SegmentFacts> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    let rows = usize::try_from(builder.metadata().file_metadata().num_rows()).unwrap_or(0);
    let mut environments = std::collections::BTreeSet::new();
    let schema = builder.parquet_schema();
    if let Some(idx) = schema
        .columns()
        .iter()
        .position(|c| c.name() == "environment")
    {
        let mask = parquet::arrow::ProjectionMask::leaves(schema, [idx]);
        for batch in builder.with_projection(mask).build()? {
            let batch = batch?;
            if let Some(col) = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
            {
                for v in col.iter().flatten() {
                    if !environments.contains(v) {
                        environments.insert(v.to_owned());
                    }
                }
            }
        }
    }
    Ok(SegmentFacts { rows, environments })
}

/// Count rows in all segments of a project (used by tests and `verify`).
pub fn count_rows(root: &Path, project: &str) -> Result<usize> {
    let mut total = 0;
    for p in segment_files(root, project)? {
        let meta = ParquetRecordBatchReaderBuilder::try_new(File::open(&p)?)?;
        total += usize::try_from(meta.metadata().file_metadata().num_rows()).unwrap_or(0);
    }
    Ok(total)
}

/// What one UTC day of a project's events weighs on disk (#514): enough to
/// estimate what reading it costs, without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaySize {
    /// The UTC day (its `date=` partition).
    pub day: chrono::NaiveDate,
    /// Segment files in the day.
    pub files: usize,
    /// Their size on disk, in bytes.
    pub bytes: u64,
    /// Their rows, from the Parquet footers.
    pub rows: u64,
}

/// The [`DaySize`] of each stored day of `project` from `from` to `to`
/// (UTC days, both included), oldest first. Only the file sizes and the
/// Parquet footers are read, never the rows. A day with no segment is
/// absent; a file compacted away while it is read is skipped.
pub fn day_sizes(
    root: &Path,
    project: &str,
    from: chrono::NaiveDate,
    to: chrono::NaiveDate,
) -> Result<Vec<DaySize>> {
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&pdir)? {
        let dir = entry?.path();
        let Some(day) = dir
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|n| n.strip_prefix("date="))
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if day < from || day > to || !dir.is_dir() {
            continue;
        }
        let mut size = DaySize {
            day,
            files: 0,
            bytes: 0,
            rows: 0,
        };
        for file in fs::read_dir(&dir)? {
            let path = file?.path();
            if path.extension().is_none_or(|x| x != "parquet") {
                continue;
            }
            let (Ok(meta), Ok(f)) = (fs::metadata(&path), File::open(&path)) else {
                continue; // compacted away since the listing
            };
            let rows = ParquetRecordBatchReaderBuilder::try_new(f)?
                .metadata()
                .file_metadata()
                .num_rows();
            size.files += 1;
            size.bytes += meta.len();
            size.rows += u64::try_from(rows).unwrap_or(0);
        }
        if size.files > 0 {
            out.push(size);
        }
    }
    out.sort_by_key(|s| s.day);
    Ok(out)
}

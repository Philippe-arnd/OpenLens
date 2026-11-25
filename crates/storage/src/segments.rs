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

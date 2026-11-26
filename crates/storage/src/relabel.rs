//! Re-label the environment of stored events (#484): after a host rule
//! changes, the events of that host over the last days are moved to the
//! environment the rules give now. Same rewrite as erasure
//! ([`crate::erase`]): each day file holding an event that moves is
//! rewritten, and the day's rollup is deleted so that the rollup pass
//! computes it again from the events.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, RecordBatch, StringArray};
use chrono::NaiveDate;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::{Result, project_dir, rewrite_day, rollup};

/// Events moved, per (stored environment, new environment).
pub type Moves = BTreeMap<(String, String), usize>;

/// The environment an event gets: `target(url, stored)`, `None` when it
/// stays where it is.
pub type Target<'a> = &'a dyn Fn(Option<&str>, &str) -> Option<String>;

/// Move the events of `project` stored on the UTC days `from..=to` to the
/// environment `target` gives. With `dry_run`, nothing is written and the
/// moves are only counted. Same preconditions as [`crate::erase`]: the WAL
/// is flushed and the caller holds the maintenance lock.
pub fn relabel(
    root: &Path,
    project: &str,
    from: NaiveDate,
    to: NaiveDate,
    target: Target<'_>,
    dry_run: bool,
) -> Result<Moves> {
    let pdir = project_dir(root, project)?;
    let mut moves = Moves::new();
    if !pdir.is_dir() {
        return Ok(moves);
    }
    let mut rewritten = false;
    for day in from.iter_days().take_while(|d| *d <= to) {
        let dir = pdir.join(format!("date={}", day.format("%Y-%m-%d")));
        if !dir.is_dir() {
            continue;
        }
        if dry_run {
            for file in parquet_files(&dir)? {
                let reader =
                    ParquetRecordBatchReaderBuilder::try_new(fs::File::open(&file)?)?.build()?;
                for b in reader {
                    moved(&b?, target, &mut moves)?;
                }
            }
            continue;
        }
        let n = rewrite_day(&dir, &mut |b: &RecordBatch| moved(b, target, &mut moves))?;
        if n > 0 {
            // A closed day's rollup holds the old environment: computed
            // again from the events by the next pass, as after an erasure.
            rollup::forget_day(root, project, day)?;
            rewritten = true;
        }
    }
    if rewritten {
        rollup::bump();
    }
    Ok(moves)
}

fn parquet_files(dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
        .collect())
}

/// `b` with its environments moved and the number of rows moved; the
/// moves are added to `moves`.
fn moved(b: &RecordBatch, target: Target<'_>, moves: &mut Moves) -> Result<(RecordBatch, usize)> {
    let schema = b.schema();
    let (Ok(ei), Ok(ui)) = (schema.index_of("environment"), schema.index_of("url")) else {
        return Ok((b.clone(), 0));
    };
    let (Some(envs), Some(urls)) = (
        b.column(ei).as_any().downcast_ref::<StringArray>(),
        b.column(ui).as_any().downcast_ref::<StringArray>(),
    ) else {
        return Ok((b.clone(), 0));
    };
    let mut new: Vec<Option<String>> = Vec::with_capacity(envs.len());
    let mut changed = 0;
    for i in 0..envs.len() {
        let url = (!urls.is_null(i)).then(|| urls.value(i));
        let to = if envs.is_null(i) {
            None
        } else {
            let env = envs.value(i);
            target(url, env).filter(|t| t != env).inspect(|t| {
                *moves.entry((env.to_owned(), t.clone())).or_default() += 1;
            })
        };
        changed += usize::from(to.is_some());
        new.push(to);
    }
    if changed == 0 {
        return Ok((b.clone(), 0));
    }
    let out: StringArray = new
        .iter()
        .enumerate()
        .map(|(i, to)| match to {
            Some(t) => Some(t.as_str()),
            None => (!envs.is_null(i)).then(|| envs.value(i)),
        })
        .collect();
    let mut cols = b.columns().to_vec();
    if let Some(c) = cols.get_mut(ei) {
        *c = Arc::new(out) as ArrayRef;
    }
    Ok((RecordBatch::try_new(schema, cols)?, changed))
}

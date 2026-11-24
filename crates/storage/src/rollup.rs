//! Rollups of closed days (#322): two small Parquet files per project and
//! UTC day, written once the day is closed. The query crate computes them;
//! this module owns where they live and how people are erased from them.
//!
//! Layout: `rollups/v<ROLLUP_VERSION>/project=<p>/date=<YYYY-MM-DD>/`
//! holding `person_q.parquet` and `metric_q.parquet`. Rollups outlive the
//! events (D-026), so a day past retention can never be rolled up again:
//! change the schema by adding nullable columns only, and a new version
//! must convert the old tree, never drop it.
//!
//! Erasing someone removes them from every rollup: a day whose events are
//! still stored is rolled up again without them, and an older day loses
//! that person's `person_q` rows (its `metric_q` keeps only counts, with no
//! one in them).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::{Array, BooleanArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, NaiveDate, Utc};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::{Result, project_dir, sync_dir, valid_project, write_parquet_as};

/// The rollup tree's version (see the module docs before changing it).
pub const ROLLUP_VERSION: u32 = 1;

/// Width of a rollup bucket. Every real time zone offset is a multiple of
/// 15 minutes, so a reader in any zone can rebuild its local days exactly.
pub const BUCKET_MINUTES: i64 = 15;

/// A day is closed one full day after it ends (UTC), so late retries of
/// the SDKs land before it is rolled up.
pub const GRACE_DAYS: i64 = 1;

/// Changes each time stored events are erased, so a rollup computed
/// before an erasure is not written after it.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The current erasure generation (see [`write`]).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

pub(crate) fn bump() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

fn ts() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

/// `person_q`: one row per person, 15-minute bucket, environment, release
/// and role, with that person's events and errors in it and the first and
/// last event time.
pub fn person_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("bucket", ts(), true),
        Field::new("person", DataType::Utf8, true),
        Field::new("environment", DataType::Utf8, true),
        Field::new("release", DataType::Utf8, true),
        Field::new("role", DataType::Utf8, true),
        Field::new("events", DataType::Int64, true),
        Field::new("errors", DataType::Int64, true),
        Field::new("first_ts", ts(), true),
        Field::new("last_ts", ts(), true),
    ]))
}

/// `metric_q`: events per 15-minute bucket, environment, release, role,
/// event name and feature, with the number of distinct people.
pub fn metric_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("bucket", ts(), true),
        Field::new("environment", DataType::Utf8, true),
        Field::new("release", DataType::Utf8, true),
        Field::new("role", DataType::Utf8, true),
        Field::new("event", DataType::Utf8, true),
        Field::new("feature", DataType::Utf8, true),
        Field::new("events", DataType::Int64, true),
        Field::new("persons", DataType::Int64, true),
    ]))
}

/// The two rollup tables: name and schema.
pub fn tables() -> [(&'static str, SchemaRef); 2] {
    [("person_q", person_schema()), ("metric_q", metric_schema())]
}

fn tree(root: &Path) -> PathBuf {
    root.join("rollups").join(format!("v{ROLLUP_VERSION}"))
}

/// Directory of a project's rollups (partitioned by `date=`).
pub fn rollups_dir(root: &Path, project: &str) -> Result<PathBuf> {
    if !valid_project(project) {
        return Err(crate::StorageError::InvalidProject);
    }
    Ok(tree(root).join(format!("project={project}")))
}

fn day_dir(root: &Path, project: &str, day: NaiveDate) -> Result<PathBuf> {
    Ok(rollups_dir(root, project)?.join(format!("date={}", day.format("%Y-%m-%d"))))
}

/// Whether `day` has both rollup files.
pub fn has(root: &Path, project: &str, day: NaiveDate) -> Result<bool> {
    let dir = day_dir(root, project, day)?;
    Ok(tables()
        .iter()
        .all(|(name, _)| dir.join(format!("{name}.parquet")).is_file()))
}

/// Closed days that have events but no rollup yet, oldest first.
pub fn missing(root: &Path, project: &str, now: DateTime<Utc>) -> Result<Vec<NaiveDate>> {
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(Vec::new());
    }
    let last_closed = now.date_naive() - chrono::Duration::days(GRACE_DAYS + 1);
    let mut days = Vec::new();
    for entry in fs::read_dir(&pdir)? {
        let path = entry?.path();
        let Some(day) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|n| n.strip_prefix("date="))
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if path.is_dir() && day <= last_closed && !has(root, project, day)? {
            days.push(day);
        }
    }
    days.sort();
    Ok(days)
}

/// Write one day's rollup tables, unless events were erased since
/// `generation` was read (the caller computed the batches before that,
/// and the day will be rolled up again on the next pass). Each table is
/// replaced atomically. Returns whether the files were written. The caller
/// holds the maintenance lock.
pub fn write(
    root: &Path,
    project: &str,
    day: NaiveDate,
    generation: u64,
    batches: &[(&str, RecordBatch)],
) -> Result<bool> {
    if generation != self::generation() {
        return Ok(false);
    }
    let dir = day_dir(root, project, day)?;
    for (name, batch) in batches {
        write_parquet_as(&dir, name, batch)?;
    }
    Ok(true)
}

/// Delete all of a project's rollups (they are rebuilt from the events).
pub fn drop_project(root: &Path, project: &str) -> Result<()> {
    let dir = rollups_dir(root, project)?;
    if dir.is_dir() {
        fs::remove_dir_all(&dir)?;
        if let Some(parent) = dir.parent() {
            sync_dir(parent)?;
        }
    }
    Ok(())
}

/// Remove `ids` from the project's rollups. A day that still has its
/// events is deleted, to be rolled up again from them; an older day keeps
/// its rollup without those people's `person_q` rows. Returns the days
/// changed. The caller holds the maintenance lock.
pub(crate) fn erase(root: &Path, project: &str, ids: &HashSet<String>) -> Result<usize> {
    let dir = rollups_dir(root, project)?;
    if ids.is_empty() || !dir.is_dir() {
        return Ok(0);
    }
    let events = project_dir(root, project)?;
    let mut changed = 0;
    for entry in fs::read_dir(&dir)? {
        let day = entry?.path();
        let file = day.join("person_q.parquet");
        if !file.is_file() {
            continue;
        }
        let mut kept = Vec::new();
        let mut gone = 0;
        for b in ParquetRecordBatchReaderBuilder::try_new(fs::File::open(&file)?)?.build()? {
            let b = b?;
            let Some(col) = b
                .column_by_name("person")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            else {
                kept.push(b);
                continue;
            };
            let keep: BooleanArray = (0..col.len())
                .map(|i| Some(col.is_null(i) || !ids.contains(col.value(i))))
                .collect();
            gone += b.num_rows() - keep.true_count();
            kept.push(arrow::compute::filter_record_batch(&b, &keep)?);
        }
        if gone == 0 {
            continue;
        }
        changed += 1;
        let name = day.file_name().unwrap_or_default();
        if events.join(name).is_dir() {
            fs::remove_dir_all(&day)?;
            sync_dir(&dir)?;
        } else {
            let merged = arrow::compute::concat_batches(&person_schema(), &kept)?;
            write_parquet_as(&day, "person_q", &merged)?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn only_closed_days_without_a_rollup_are_missing() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["2026-03-01", "2026-03-08", "2026-03-09", "2026-03-10"] {
            fs::create_dir_all(dir.path().join(format!("events/project=p/date={d}"))).unwrap();
        }
        let now = "2026-03-10T12:00:00Z".parse().unwrap();
        // The 9th ended at midnight but its grace day is not over yet.
        assert_eq!(
            missing(dir.path(), "p", now).unwrap(),
            [day("2026-03-01"), day("2026-03-08")]
        );
        let empty = RecordBatch::new_empty(person_schema());
        let metric = RecordBatch::new_empty(metric_schema());
        let g = generation();
        assert!(
            write(
                dir.path(),
                "p",
                day("2026-03-01"),
                g,
                &[("person_q", empty.clone()), ("metric_q", metric.clone())]
            )
            .unwrap()
        );
        assert_eq!(missing(dir.path(), "p", now).unwrap(), [day("2026-03-08")]);
        // An erasure in between: the stale rollup is not written.
        bump();
        assert!(
            !write(
                dir.path(),
                "p",
                day("2026-03-08"),
                g,
                &[("person_q", empty), ("metric_q", metric)]
            )
            .unwrap()
        );
        assert!(!has(dir.path(), "p", day("2026-03-08")).unwrap());
    }

    #[test]
    fn erasing_rebuilds_stored_days_and_trims_older_ones() {
        use arrow::array::{Int64Array, TimestampMicrosecondArray};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let batch = |people: &[&str]| {
            let n = people.len();
            let ts = || Arc::new(TimestampMicrosecondArray::from(vec![0; n]).with_timezone("UTC"));
            let txt = |v: Option<&str>| Arc::new(StringArray::from(vec![v; n]));
            RecordBatch::try_new(
                person_schema(),
                vec![
                    ts(),
                    Arc::new(StringArray::from(people.to_vec())),
                    txt(Some("production")),
                    txt(None),
                    txt(None),
                    Arc::new(Int64Array::from(vec![1; n])),
                    Arc::new(Int64Array::from(vec![0; n])),
                    ts(),
                    ts(),
                ],
            )
            .unwrap()
        };
        let metric = RecordBatch::new_empty(metric_schema());
        // The 1st is past retention (no events left), the 2nd still stored.
        fs::create_dir_all(root.join("events/project=p/date=2026-01-02")).unwrap();
        for d in ["2026-01-01", "2026-01-02"] {
            let tables = [
                ("person_q", batch(&["bob", "ann"])),
                ("metric_q", metric.clone()),
            ];
            assert!(write(root, "p", day(d), generation(), &tables).unwrap());
        }
        let ids: HashSet<String> = ["bob".to_owned()].into();
        assert_eq!(erase(root, "p", &ids).unwrap(), 2);
        assert!(!has(root, "p", day("2026-01-02")).unwrap());
        let old = rollups_dir(root, "p")
            .unwrap()
            .join("date=2026-01-01/person_q.parquet");
        let people: Vec<String> =
            ParquetRecordBatchReaderBuilder::try_new(fs::File::open(old).unwrap())
                .unwrap()
                .build()
                .unwrap()
                .flat_map(|b| {
                    let b = b.unwrap();
                    let c = b
                        .column(1)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap()
                        .clone();
                    (0..c.len())
                        .map(|i| c.value(i).to_owned())
                        .collect::<Vec<_>>()
                })
                .collect();
        assert_eq!(people, ["ann"]);
        // Done once: a second pass changes nothing.
        assert_eq!(erase(root, "p", &ids).unwrap(), 0);
        drop_project(root, "p").unwrap();
        assert!(!rollups_dir(root, "p").unwrap().exists());
    }
}

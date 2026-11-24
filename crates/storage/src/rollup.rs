//! Rollups of closed days (#322): two small Parquet files per project and
//! UTC day, written once the day is closed and rebuilt from the events
//! whenever those change. The query crate computes them; this module owns
//! where they live and when they go away.
//!
//! Layout: `rollups/v<ROLLUP_VERSION>/project=<p>/date=<YYYY-MM-DD>/`
//! holding `person_q.parquet` and `metric_q.parquet`. The version is in the
//! path, so a new schema starts from an empty tree and every closed day is
//! rolled up again; older trees are deleted.
//!
//! Rollups never outlive the events they come from: retention drops them
//! with their day, and erasing anyone drops the project's rollups so they
//! are rebuilt without that person (docs/privacy.md stays true).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::RecordBatch;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, NaiveDate, Utc};

use crate::{Result, project_dir, sync_dir, valid_project, write_parquet_as};

/// Bumped when a rollup schema changes: the tree moves to a new path.
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

/// Delete the rollups of days before `cutoff` (`date=YYYY-MM-DD`), and the
/// trees of older rollup versions.
pub(crate) fn prune(root: &Path, project: &str, cutoff: &str) -> Result<()> {
    let base = root.join("rollups");
    if base.is_dir() {
        let current = format!("v{ROLLUP_VERSION}");
        for entry in fs::read_dir(&base)? {
            let path = entry?.path();
            if path.is_dir() && path.file_name().and_then(|s| s.to_str()) != Some(current.as_str())
            {
                fs::remove_dir_all(&path)?;
            }
        }
    }
    let dir = rollups_dir(root, project)?;
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.is_dir() && name.starts_with("date=") && name.len() == 15 && name < cutoff {
            fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
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
    fn prune_drops_old_days_and_old_versions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("rollups/v0/project=p/date=2026-01-01")).unwrap();
        let base = rollups_dir(root, "p").unwrap();
        fs::create_dir_all(base.join("date=2026-01-01")).unwrap();
        fs::create_dir_all(base.join("date=2026-06-01")).unwrap();
        prune(root, "p", "date=2026-03-01").unwrap();
        assert!(!root.join("rollups/v0").exists());
        assert!(!base.join("date=2026-01-01").exists());
        assert!(base.join("date=2026-06-01").exists());
        drop_project(root, "p").unwrap();
        assert!(!base.exists());
    }
}

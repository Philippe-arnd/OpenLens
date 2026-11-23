//! `openlens backup <dest>`: a consistent copy of the data dir, safe while
//! the server runs (D-015).
//!
//! - `meta.db` is copied with SQLite's `VACUUM INTO`.
//! - Parquet segments, WAL segments and replay chunks are files that are
//!   written once and then only deleted, so they are copied as they are.
//!   A flush (WAL segment to Parquet) or a compaction during the copy could
//!   leave an event in both copies, or in neither, so the file lists are
//!   compared before and after, and the copy starts over if they changed.
//!
//! Restore: stop the server, replace the data dir with the backup, start.
//!
//! `openlens backup --into <dir> [--keep N]` names the copy after the UTC
//! time and keeps the newest N copies in `<dir>` (scheduled backups, e.g. a
//! Kubernetes CronJob: the image has no shell for `date` or cleanup).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const TREES: [&str; 3] = ["wal", "events", "replay"];

fn files(root: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<PathBuf> = TREES.iter().map(|t| root.join(t)).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_none_or(|x| x != "tmp")
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.insert(rel.to_path_buf());
            }
        }
    }
    out
}

/// Copy the data dir `src` into `dest`. Returns the number of files copied.
pub fn run(src: &Path, dest: &Path) -> Result<usize, String> {
    let src = fs::canonicalize(src).map_err(|e| format!("data dir {}: {e}", src.display()))?;
    if dest.exists()
        && fs::read_dir(dest)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!("{} exists and is not empty", dest.display()));
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let dest = fs::canonicalize(dest).map_err(|e| e.to_string())?;
    if TREES.iter().any(|t| dest.starts_with(src.join(t))) || dest == src {
        return Err("the backup cannot go inside the data trees".into());
    }
    for attempt in 1..=10 {
        let before = files(&src);
        let mut copied = 0;
        for rel in &before {
            let to = dest.join(rel);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            match fs::copy(src.join(rel), &to) {
                Ok(_) => copied += 1,
                // Deleted by a flush or compaction meanwhile: the list check below retries.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", rel.display())),
            }
        }
        if files(&src) == before {
            let meta =
                openlens_meta::Meta::open(&src.join("meta.db")).map_err(|e| e.to_string())?;
            meta.backup_to(&dest.join("meta.db"))
                .map_err(|e| format!("meta.db: {e}"))?;
            return Ok(copied + 1);
        }
        tracing::info!(
            attempt,
            "data changed during the backup copy, starting over"
        );
        for t in TREES {
            let d = dest.join(t);
            if d.exists() {
                fs::remove_dir_all(&d).map_err(|e| e.to_string())?;
            }
        }
    }
    Err("the data kept changing during the copy; try again when ingestion is quieter".into())
}

/// Name of a rotated backup: the UTC time, sortable, safe in paths.
fn stamp(now: chrono::DateTime<chrono::Utc>) -> String {
    now.format("%Y-%m-%dT%H-%M-%SZ").to_string()
}

/// Is `name` one of ours (so rotation never deletes anything else)?
fn is_stamp(name: &str) -> bool {
    chrono::NaiveDateTime::parse_from_str(name, "%Y-%m-%dT%H-%M-%SZ").is_ok()
}

/// Back up into `<into>/<UTC time>`, then delete all but the newest `keep`
/// copies there. Returns the new copy, its file count and the removed copies.
pub fn rotate(
    src: &Path,
    into: &Path,
    keep: usize,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(PathBuf, usize, Vec<PathBuf>), String> {
    if keep == 0 {
        return Err("--keep must be at least 1".into());
    }
    let dest = into.join(stamp(now));
    let n = run(src, &dest)?;
    let mut ours: Vec<PathBuf> = fs::read_dir(into)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().and_then(|n| n.to_str()).is_some_and(is_stamp))
        .collect();
    ours.sort();
    let extra = ours.len().saturating_sub(keep);
    let removed: Vec<PathBuf> = ours.into_iter().take(extra).collect();
    for old in &removed {
        fs::remove_dir_all(old).map_err(|e| format!("{}: {e}", old.display()))?;
    }
    Ok((dest, n, removed))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn copies_every_tree_and_the_store_and_refuses_bad_targets() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data");
        fs::create_dir_all(data.join("events/project=p/date=2026-01-01")).unwrap();
        fs::write(
            data.join("events/project=p/date=2026-01-01/a.parquet"),
            b"x",
        )
        .unwrap();
        fs::create_dir_all(data.join("wal")).unwrap();
        fs::write(data.join("wal/00001.jsonl"), b"{}\n").unwrap();
        fs::write(data.join("wal/00002.tmp"), b"").unwrap();
        fs::create_dir_all(data.join("replay/project=p/s")).unwrap();
        fs::write(data.join("replay/project=p/s/00000.zst"), b"z").unwrap();
        let meta = openlens_meta::Meta::open(&data.join("meta.db")).unwrap();
        meta.ensure_default_project(None).unwrap();
        drop(meta);

        let out = d.path().join("backup");
        assert_eq!(run(&data, &out).unwrap(), 4);
        assert!(
            out.join("events/project=p/date=2026-01-01/a.parquet")
                .exists()
        );
        assert!(!out.join("wal/00002.tmp").exists());
        let copy = openlens_meta::Meta::open(&out.join("meta.db")).unwrap();
        assert_eq!(copy.projects().unwrap().len(), 1);

        assert!(run(&data, &out).is_err(), "non-empty target");
        assert!(
            run(&data, &data.join("events/x")).is_err(),
            "inside the data"
        );
    }

    #[test]
    fn rotation_keeps_the_newest_copies_and_nothing_else_is_touched() {
        let src = tempfile::tempdir().unwrap();
        openlens_meta::Meta::open(&src.path().join("meta.db")).unwrap();
        let into = tempfile::tempdir().unwrap();
        fs::create_dir(into.path().join("keep-me")).unwrap();
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-11-20T03:17:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        for d in 0..4 {
            rotate(src.path(), into.path(), 2, t0 + chrono::Duration::days(d)).unwrap();
        }
        let mut names: Vec<String> = fs::read_dir(into.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["2026-11-22T03-17-00Z", "2026-11-23T03-17-00Z", "keep-me"]
        );
        assert!(into.path().join("2026-11-23T03-17-00Z/meta.db").exists());
        assert!(rotate(src.path(), into.path(), 0, t0).is_err());
    }
}

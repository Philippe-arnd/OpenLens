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
        eprintln!("data changed during the copy (attempt {attempt}), starting over");
        for t in TREES {
            let d = dest.join(t);
            if d.exists() {
                fs::remove_dir_all(&d).map_err(|e| e.to_string())?;
            }
        }
    }
    Err("the data kept changing during the copy; try again when ingestion is quieter".into())
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
}

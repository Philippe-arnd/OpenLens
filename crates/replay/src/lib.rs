//! Session recording chunks: ingest, compression, storage and playback index.
//!
//! Layer: L2 (see `docs/architecture.md`).
//!
//! A recording is the list of DOM events the browser recorder emits for one
//! session (`session_id`, the same id the analytics events carry). The SDK
//! sends it in numbered chunks; each chunk is stored as one zstd file:
//!
//! ```text
//! <data>/replay/project=<p>/<session_id>/info.json     first chunk's owner and time
//! <data>/replay/project=<p>/<session_id>/<seq>.zst      JSON array of events
//! ```
//!
//! The content is opaque to the server: it is validated as a JSON array and
//! never interpreted. Ids are restricted to a safe alphabet, so they can be
//! used as path components, and every size is bounded.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Highest chunk number in one session (a chunk every ~10 s for ~1 day).
pub const MAX_SEQ: u32 = 10_000;
/// Largest stored size for one session, compressed, in bytes.
pub const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;
/// Most recordings returned by [`list`].
pub const LIST_LIMIT: usize = 200;
const ZSTD_LEVEL: i32 = 6;

/// Errors from the replay store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A project or session id outside `[A-Za-z0-9_-]{1,64}`.
    #[error("invalid id")]
    InvalidId,
    /// A chunk number above [`MAX_SEQ`].
    #[error("chunk number out of range")]
    InvalidSeq,
    /// The chunk is not a JSON array.
    #[error("chunk must be a JSON array of recorder events")]
    InvalidChunk,
    /// The session reached [`MAX_SESSION_BYTES`].
    #[error("recording is too large")]
    TooLarge,
    /// Disk or encoding failure.
    #[error("replay storage: {0}")]
    Io(#[from] std::io::Error),
    /// JSON encoding failure.
    #[error("replay encoding: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Who a recording belongs to, written with its first stored chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Info {
    /// The person recorded.
    pub distinct_id: String,
    /// When the first chunk arrived.
    pub started: DateTime<Utc>,
}

/// One recording in a listing.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Summary {
    /// Session id (matches `session_id` on analytics events).
    pub session_id: String,
    /// The person recorded.
    pub distinct_id: String,
    /// When the first chunk arrived.
    pub started: DateTime<Utc>,
    /// When the last chunk arrived.
    pub updated: DateTime<Utc>,
    /// Number of stored chunks.
    pub chunks: usize,
    /// Stored size, compressed, in bytes.
    pub bytes: u64,
}

/// Is `s` usable as a path component?
pub fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn project_dir(root: &Path, project: &str) -> Result<PathBuf> {
    if !valid_id(project) {
        return Err(Error::InvalidId);
    }
    Ok(root.join("replay").join(format!("project={project}")))
}

fn session_dir(root: &Path, project: &str, session: &str) -> Result<PathBuf> {
    if !valid_id(session) {
        return Err(Error::InvalidId);
    }
    Ok(project_dir(root, project)?.join(session))
}

fn chunk_files(dir: &Path) -> Result<Vec<(u32, PathBuf, u64, std::time::SystemTime)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(seq) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".zst"))
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let md = entry.metadata()?;
        out.push((seq, entry.path(), md.len(), md.modified()?));
    }
    out.sort_by_key(|c| c.0);
    Ok(out)
}

/// Store one chunk. `events` is the raw JSON body of the chunk (an array).
/// Re-sending a chunk number overwrites it, so client retries are harmless.
pub fn append(
    root: &Path,
    project: &str,
    session: &str,
    distinct_id: &str,
    seq: u32,
    events: &serde_json::value::RawValue,
    now: DateTime<Utc>,
) -> Result<u64> {
    if seq > MAX_SEQ {
        return Err(Error::InvalidSeq);
    }
    if !events.get().trim_start().starts_with('[') {
        return Err(Error::InvalidChunk);
    }
    let dir = session_dir(root, project, session)?;
    fs::create_dir_all(&dir)?;
    let used: u64 = chunk_files(&dir)?
        .iter()
        .filter(|c| c.0 != seq)
        .map(|c| c.2)
        .sum();
    if used >= MAX_SESSION_BYTES {
        return Err(Error::TooLarge);
    }
    let info = dir.join("info.json");
    if !info.exists() {
        let id: String = distinct_id.chars().take(200).collect();
        write_atomic(
            &info,
            &serde_json::to_vec(&Info {
                distinct_id: id,
                started: now,
            })?,
        )?;
    }
    let packed = zstd::bulk::compress(events.get().as_bytes(), ZSTD_LEVEL)?;
    if used + packed.len() as u64 > MAX_SESSION_BYTES {
        return Err(Error::TooLarge);
    }
    write_atomic(&dir.join(format!("{seq:05}.zst")), &packed)?;
    Ok(packed.len() as u64)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}

/// The most recently updated recordings of a project, newest first,
/// optionally only those of one person.
pub fn list(root: &Path, project: &str, distinct_id: Option<&str>) -> Result<Vec<Summary>> {
    let dir = project_dir(root, project)?;
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(session) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_id(&session) {
            continue;
        }
        let path = entry.path();
        let Ok(raw) = fs::read(path.join("info.json")) else {
            continue;
        };
        let Ok(info) = serde_json::from_slice::<Info>(&raw) else {
            continue;
        };
        if distinct_id.is_some_and(|d| d != info.distinct_id) {
            continue;
        }
        let chunks = chunk_files(&path)?;
        let Some(last) = chunks.iter().map(|c| c.3).max() else {
            continue;
        };
        out.push(Summary {
            session_id: session,
            distinct_id: info.distinct_id,
            started: info.started,
            updated: DateTime::<Utc>::from(last),
            chunks: chunks.len(),
            bytes: chunks.iter().map(|c| c.2).sum(),
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.updated));
    out.truncate(LIST_LIMIT);
    Ok(out)
}

/// Days a recording is kept after its last chunk. Fixed by design (D-015):
/// replays are for looking at recent behaviour, and they are the bulk of
/// the data.
pub const RETENTION_DAYS: i64 = 30;

/// Delete a project's recordings whose last chunk is older than
/// [`RETENTION_DAYS`]. Returns the number removed.
pub fn prune(root: &Path, project: &str, now: DateTime<Utc>) -> Result<usize> {
    let dir = project_dir(root, project)?;
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(0);
    };
    let cutoff: std::time::SystemTime = (now - chrono::Duration::days(RETENTION_DAYS)).into();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let valid = entry.file_name().to_str().is_some_and(valid_id);
        if !valid || !path.is_dir() {
            continue;
        }
        let last = chunk_files(&path)?
            .iter()
            .map(|c| c.3)
            .max()
            .or_else(|| entry.metadata().and_then(|m| m.modified()).ok());
        if last.is_none_or(|t| t < cutoff) {
            fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// One recording's owner and all its events in chunk order, as one JSON
/// array, or `None` if it does not exist.
pub fn load(root: &Path, project: &str, session: &str) -> Result<Option<(Info, String)>> {
    let dir = session_dir(root, project, session)?;
    let Ok(raw) = fs::read(dir.join("info.json")) else {
        return Ok(None);
    };
    let info: Info = serde_json::from_slice(&raw)?;
    let mut out = String::from("[");
    let mut first = true;
    for (_, path, _, _) in chunk_files(&dir)? {
        let packed = fs::read(path)?;
        let mut text = String::new();
        std::io::Read::read_to_string(
            &mut zstd::stream::read::Decoder::new(&packed[..])?,
            &mut text,
        )?;
        let inner = text.trim();
        let inner = inner
            .strip_prefix('[')
            .and_then(|t| t.strip_suffix(']'))
            .unwrap_or("")
            .trim();
        if inner.is_empty() {
            continue;
        }
        if !first {
            out.push(',');
        }
        out.push_str(inner);
        first = false;
    }
    out.push(']');
    Ok(Some((info, out)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use serde_json::value::RawValue;

    fn raw(s: &str) -> Box<RawValue> {
        RawValue::from_string(s.to_owned()).unwrap()
    }

    #[test]
    fn round_trip_in_chunk_order() {
        let d = tempfile::tempdir().unwrap();
        let now = Utc::now();
        append(
            d.path(),
            "p1",
            "s1",
            "u1",
            1,
            &raw(r#"[{"t":2},{"t":3}]"#),
            now,
        )
        .unwrap();
        append(d.path(), "p1", "s1", "u1", 0, &raw(r#"[{"t":1}]"#), now).unwrap();
        append(d.path(), "p1", "s1", "u1", 2, &raw("[]"), now).unwrap();
        let (info, events) = load(d.path(), "p1", "s1").unwrap().unwrap();
        assert_eq!(info.distinct_id, "u1");
        let v: serde_json::Value = serde_json::from_str(&events).unwrap();
        assert_eq!(v, serde_json::json!([{"t":1},{"t":2},{"t":3}]));
        let l = list(d.path(), "p1", None).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].chunks, 3);
        assert!(list(d.path(), "p1", Some("other")).unwrap().is_empty());
        assert!(load(d.path(), "p1", "nope").unwrap().is_none());
    }

    #[test]
    fn prune_keeps_recent_recordings_only() {
        let d = tempfile::tempdir().unwrap();
        append(d.path(), "p", "s", "u", 0, &raw("[]"), Utc::now()).unwrap();
        assert_eq!(prune(d.path(), "p", Utc::now()).unwrap(), 0);
        let later = Utc::now() + chrono::Duration::days(RETENTION_DAYS + 1);
        assert_eq!(prune(d.path(), "p", later).unwrap(), 1);
        assert!(list(d.path(), "p", None).unwrap().is_empty());
        assert_eq!(prune(d.path(), "nobody", later).unwrap(), 0);
    }

    #[test]
    fn rejects_hostile_input() {
        let d = tempfile::tempdir().unwrap();
        let now = Utc::now();
        for bad in ["../x", "", "a/b", ".", &"a".repeat(65)] {
            assert!(matches!(
                append(d.path(), "p", bad, "u", 0, &raw("[]"), now),
                Err(Error::InvalidId)
            ));
        }
        assert!(matches!(
            append(d.path(), "p", "s", "u", 0, &raw("{}"), now),
            Err(Error::InvalidChunk)
        ));
        assert!(matches!(
            append(d.path(), "p", "s", "u", MAX_SEQ + 1, &raw("[]"), now),
            Err(Error::InvalidSeq)
        ));
    }

    #[test]
    fn retries_overwrite_and_compression_is_effective() {
        let d = tempfile::tempdir().unwrap();
        let body = format!(
            "[{}]",
            vec![r#"{"type":3,"data":{"source":1,"x":10}}"#; 500].join(",")
        );
        let n = append(d.path(), "p", "s", "u", 0, &raw(&body), Utc::now()).unwrap();
        assert!(n * 10 < body.len() as u64);
        append(d.path(), "p", "s", "u", 0, &raw(&body), Utc::now()).unwrap();
        assert_eq!(list(d.path(), "p", None).unwrap()[0].chunks, 1);
    }
}

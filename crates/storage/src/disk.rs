//! What a project's events take on disk (#606, D-088).

use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::{Result, project_dir};

/// Closed days the measured cost of events is averaged over (#606, D-088).
pub const COST_DAYS: i64 = 7;

/// What a project's events take on disk (#606, D-088).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskUse {
    /// Bytes of every day partition on the disk.
    pub total: u64,
    /// Bytes of the closed days among the last [`COST_DAYS`].
    pub recent: u64,
    /// How many of those days are on the disk.
    pub recent_days: u32,
}

impl DiskUse {
    /// Bytes a day, from the recent closed days on the disk (0 with none).
    #[must_use]
    pub fn per_day(&self) -> u64 {
        if self.recent_days == 0 {
            0
        } else {
            self.recent / u64::from(self.recent_days)
        }
    }
}

/// The bytes of a project's event days: all of them, and the closed days
/// among the last [`COST_DAYS`]. Rollups are not counted (they are kept
/// whatever the retention, D-026).
pub fn disk_use(root: &Path, project: &str, now: DateTime<Utc>) -> Result<DiskUse> {
    let pdir = project_dir(root, project)?;
    let mut out = DiskUse::default();
    let Ok(entries) = fs::read_dir(&pdir) else {
        return Ok(out);
    };
    let today = now.date_naive();
    let from = today - chrono::Duration::days(COST_DAYS);
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(day) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|n| n.strip_prefix("date="))
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        let bytes = tree_bytes(&path, 4);
        out.total = out.total.saturating_add(bytes);
        if day >= from && day < today {
            out.recent = out.recent.saturating_add(bytes);
            out.recent_days += 1;
        }
    }
    Ok(out)
}

/// Bytes of the files under `path`, `depth` directories down at most;
/// links are not followed and what cannot be read counts as nothing.
#[must_use]
pub fn tree_bytes(path: &Path, depth: u32) -> u64 {
    let Ok(md) = fs::symlink_metadata(path) else {
        return 0;
    };
    if md.is_file() {
        return md.len();
    }
    if !md.is_dir() || depth == 0 {
        return 0;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().fold(0u64, |n, e| {
        n.saturating_add(tree_bytes(&e.path(), depth - 1))
    })
}

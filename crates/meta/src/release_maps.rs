//! The feature map version each release shipped with (#483 item 2, D-070).
//!
//! A feature's numbers per release are counted with the map version in
//! force when the release came out (#283): the latest saved before its
//! first event. That is derived from the events of the last 90 days and the
//! newest 50 stored versions, so it would drift once the release's first
//! event leaves the window or its version leaves the 50. The first time a
//! release's version is derived it is recorded here and kept: later reads
//! use the record. One small row per (project, environment, release), at
//! most [`MAX_RELEASE_MAPS`] per project, written once.

use super::*;

/// Releases recorded per project; past it the oldest first events go.
pub const MAX_RELEASE_MAPS: i64 = 1000;

/// The map version a release of an environment is counted with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseMap {
    /// Environment.
    pub environment: String,
    /// Release name.
    pub release: String,
    /// Its first event, as it was shown when recorded.
    pub first_seen: String,
    /// The map version in force then.
    pub map_version: i64,
    /// The release is older than every stored version, so it is counted
    /// with the oldest one.
    pub before_first_map: bool,
}

impl Meta {
    /// The recorded releases of `project` in `environment` (at most
    /// [`MAX_RELEASE_MAPS`]).
    pub fn release_maps(&self, project: &str, environment: &str) -> Result<Vec<ReleaseMap>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT environment, release, first_seen, map_version, before_first_map \
                 FROM release_maps WHERE project = ?1 AND environment = ?2 LIMIT ?3",
            )?;
            let rows = st.query_map(params![project, environment, MAX_RELEASE_MAPS], |r| {
                Ok(ReleaseMap {
                    environment: r.get(0)?,
                    release: r.get(1)?,
                    first_seen: r.get(2)?,
                    map_version: r.get(3)?,
                    before_first_map: r.get::<_, i64>(4)? != 0,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Record `rows` for `project`; a release already recorded keeps its
    /// row. Returns how many were added. The project is then cut back to
    /// [`MAX_RELEASE_MAPS`], oldest first events first.
    pub fn record_release_maps(&self, project: &str, rows: &[ReleaseMap]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let mut added = 0;
            for r in rows {
                added += tx.execute(
                    "INSERT OR IGNORE INTO release_maps \
                     (project, environment, release, first_seen, map_version, before_first_map) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        project,
                        r.environment,
                        r.release,
                        r.first_seen,
                        r.map_version,
                        i64::from(r.before_first_map)
                    ],
                )?;
            }
            tx.execute(
                "DELETE FROM release_maps WHERE project = ?1 AND rowid NOT IN \
                 (SELECT rowid FROM release_maps WHERE project = ?1 \
                  ORDER BY first_seen DESC, rowid DESC LIMIT ?2)",
                params![project, MAX_RELEASE_MAPS],
            )?;
            tx.commit()?;
            Ok(added)
        })
    }
}

//! The features each release touched (#576, D-095): its changed files
//! (what the git host read, [`crate::release_reads`]) matched once against
//! the code paths of the feature map in force for it, and kept here, so
//! the verdict, the Releases page, `release_report` and the feature page
//! read them without matching again.
//!
//! One row per (project, environment, release) in `release_attribution`,
//! with the range it was matched for and the counts; the touched features
//! in `release_attribution_features`. The caller bounds what it stores. A
//! row goes when its release's read goes.

use super::*;

/// The tables, created next to the main schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS release_attribution (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  from_sha TEXT NOT NULL,
  sha TEXT NOT NULL,
  map_version INTEGER,
  features_with_paths INTEGER NOT NULL DEFAULT 0,
  files INTEGER NOT NULL DEFAULT 0,
  unmatched INTEGER NOT NULL DEFAULT 0,
  skipped INTEGER NOT NULL DEFAULT 0,
  features_total INTEGER NOT NULL DEFAULT 0,
  computed_at INTEGER NOT NULL,
  PRIMARY KEY (project, environment, release)
);
CREATE TABLE IF NOT EXISTS release_attribution_features (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  seq INTEGER NOT NULL,
  feature TEXT NOT NULL,
  files INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS release_attribution_features_rel ON release_attribution_features (project, environment, release);
";

/// A release's changed files matched to features, as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseFeatures {
    /// Environment.
    pub environment: String,
    /// Release name.
    pub release: String,
    /// The first commit of the range matched (exclusive).
    pub from: String,
    /// The last commit of the range matched.
    pub to: String,
    /// The map version matched with; `None` when no map was saved then.
    pub map_version: Option<i64>,
    /// Features of that map that declare code paths.
    pub features_with_paths: i64,
    /// Changed files examined.
    pub files: i64,
    /// Of those, files that match no feature.
    pub unmatched: i64,
    /// Changed files not examined (the matcher's bounds).
    pub skipped: i64,
    /// Features touched, kept or not.
    pub features_total: i64,
    /// The features touched (kept), each with its matching files, in map
    /// order.
    pub features: Vec<(String, i64)>,
    /// When it was matched (Unix seconds).
    pub computed_at: i64,
}

/// Rows read at most by [`Meta::release_features_of`].
const MAX_LISTED_ROWS: i64 = 20_000;

impl Meta {
    /// Up to `limit` releases of `project` whose range was read from the
    /// git host but not matched to features yet: never matched, matched
    /// for another range, or matched before the project had a feature map.
    /// Newest read first.
    pub fn release_features_pending(
        &self,
        project: &str,
        limit: i64,
    ) -> Result<Vec<release_reads::ReleaseRange>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT g.environment, g.release, g.from_sha, g.sha FROM release_reads g \
                 WHERE g.project = ?1 AND g.fetched_at IS NOT NULL AND NOT EXISTS \
                 (SELECT 1 FROM release_attribution a WHERE a.project = g.project \
                  AND a.environment = g.environment AND a.release = g.release \
                  AND a.from_sha = g.from_sha AND a.sha = g.sha \
                  AND (a.map_version IS NOT NULL OR NOT EXISTS \
                       (SELECT 1 FROM feature_maps m WHERE m.project = g.project))) \
                 ORDER BY g.fetched_at DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![project, limit], |r| {
                Ok(release_reads::ReleaseRange {
                    environment: r.get(0)?,
                    release: r.get(1)?,
                    from: r.get(2)?,
                    to: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// When `release` of `environment` was first deployed (Unix seconds),
    /// as the deploy step or hook recorded it.
    pub fn release_first_deploy(
        &self,
        project: &str,
        environment: &str,
        release: &str,
    ) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT min(at) FROM deploys WHERE project = ?1 AND environment = ?2 AND release = ?3",
                params![project, environment, release],
                |r| r.get::<_, Option<i64>>(0),
            )?)
        })
    }

    /// Keep a release's match, replacing the one kept before. Matches of
    /// releases whose read is gone go too.
    pub fn store_release_features(&self, project: &str, a: &ReleaseFeatures) -> Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let key = params![project, a.environment, a.release];
            tx.execute(
                "DELETE FROM release_attribution_features WHERE project = ?1 AND environment = ?2 \
                 AND release = ?3",
                key,
            )?;
            tx.execute(
                "INSERT INTO release_attribution (project, environment, release, from_sha, sha, \
                 map_version, features_with_paths, files, unmatched, skipped, features_total, \
                 computed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                 ON CONFLICT (project, environment, release) DO UPDATE SET \
                 from_sha = excluded.from_sha, sha = excluded.sha, \
                 map_version = excluded.map_version, \
                 features_with_paths = excluded.features_with_paths, files = excluded.files, \
                 unmatched = excluded.unmatched, skipped = excluded.skipped, \
                 features_total = excluded.features_total, computed_at = excluded.computed_at",
                params![
                    project,
                    a.environment,
                    a.release,
                    a.from,
                    a.to,
                    a.map_version,
                    a.features_with_paths,
                    a.files,
                    a.unmatched,
                    a.skipped,
                    a.features_total,
                    a.computed_at
                ],
            )?;
            for (i, (feature, files)) in a.features.iter().enumerate() {
                tx.execute(
                    "INSERT INTO release_attribution_features (project, environment, release, seq, \
                     feature, files) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![project, a.environment, a.release, i as i64, feature, files],
                )?;
            }
            for t in ["release_attribution", "release_attribution_features"] {
                tx.execute(
                    &format!(
                        "DELETE FROM {t} WHERE project = ?1 AND NOT EXISTS (SELECT 1 FROM \
                         release_reads g WHERE g.project = {t}.project \
                         AND g.environment = {t}.environment AND g.release = {t}.release)"
                    ),
                    [project],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// The match kept for `release` of `environment`, if any (whatever
    /// range it was matched for: the caller compares it with the read).
    pub fn release_features(
        &self,
        project: &str,
        environment: &str,
        release: &str,
    ) -> Result<Option<ReleaseFeatures>> {
        self.read(|c| {
            let head = c
                .query_row(
                    "SELECT environment, release, from_sha, sha, map_version, features_with_paths, \
                     files, unmatched, skipped, features_total, computed_at FROM release_attribution \
                     WHERE project = ?1 AND environment = ?2 AND release = ?3",
                    params![project, environment, release],
                    head_of,
                )
                .optional()?;
            let Some(mut a) = head else {
                return Ok(None);
            };
            a.features = c
                .prepare(
                    "SELECT feature, files FROM release_attribution_features WHERE project = ?1 \
                     AND environment = ?2 AND release = ?3 ORDER BY seq",
                )?
                .query_map(params![project, environment, release], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(a))
        })
    }

    /// The touched features of every release of `environment` whose match
    /// is for the range read now: `(release, feature, files)`, bounded.
    pub fn release_features_of(
        &self,
        project: &str,
        environment: &str,
    ) -> Result<Vec<(String, String, i64)>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT f.release, f.feature, f.files FROM release_attribution_features f \
                 JOIN release_attribution a ON a.project = f.project \
                 AND a.environment = f.environment AND a.release = f.release \
                 JOIN release_reads g ON g.project = a.project AND g.environment = a.environment \
                 AND g.release = a.release AND g.from_sha = a.from_sha AND g.sha = a.sha \
                 WHERE f.project = ?1 AND f.environment = ?2 \
                 ORDER BY a.computed_at DESC, f.release, f.seq LIMIT ?3",
            )?;
            let rows = st.query_map(params![project, environment, MAX_LISTED_ROWS], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

fn head_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReleaseFeatures> {
    Ok(ReleaseFeatures {
        environment: r.get(0)?,
        release: r.get(1)?,
        from: r.get(2)?,
        to: r.get(3)?,
        map_version: r.get(4)?,
        features_with_paths: r.get(5)?,
        files: r.get(6)?,
        unmatched: r.get(7)?,
        skipped: r.get(8)?,
        features_total: r.get(9)?,
        features: Vec::new(),
        computed_at: r.get(10)?,
    })
}

//! A feature's numbers per release, each counted with the map version in
//! force when the release came out (#283, decision B of #240). The version
//! in force is the latest one saved before the release's first event, so
//! pushing a new map does not change the numbers of earlier releases: they
//! keep the routes and success event they shipped with. A release older
//! than every stored version is counted with the oldest one, and says so.
//!
//! The version found the first time a release is counted is recorded
//! (`release_maps`, #483 item 2, D-070) and read from then on, so it does
//! not move when the release's first event leaves the 90 days or its
//! version leaves the newest 50.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use openlens_featuremap::{Feature, FeatureMap};
use openlens_meta::release_maps::ReleaseMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, int, source, text};

/// Releases shown at most (the newest).
const MAX_RELEASES: usize = 10;

/// One stored version of the map, and when it was saved.
#[derive(Debug, Clone)]
pub struct MapAt {
    /// Version number.
    pub version: i64,
    /// When it was saved.
    pub saved: DateTime<Utc>,
    /// The map.
    pub map: FeatureMap,
}

/// The stored versions of a project's map (the newest 50) with when each
/// was saved, for the numbers counted with the version in force (#283).
/// Versions that cannot be read or parsed are left out.
#[must_use]
pub fn history(meta: &openlens_meta::Meta, project: &str) -> Vec<MapAt> {
    meta.feature_map_versions(project)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            let saved = DateTime::parse_from_rfc3339(&v.created_at).ok()?;
            let yaml = meta.feature_map_version(project, v.version).ok()??;
            Some(MapAt {
                version: v.version,
                saved: saved.with_timezone(&Utc),
                map: openlens_featuremap::parse(&yaml).map?,
            })
        })
        .collect()
}

/// What the numbers per release are counted with: the stored versions
/// of the map and the releases already recorded (D-070).
#[derive(Debug, Clone, Default)]
pub struct History {
    /// The newest 50 stored versions, with when each was saved.
    pub versions: Vec<MapAt>,
    /// The releases of one environment whose version is recorded.
    pub recorded: Vec<ReleaseMap>,
    /// Recorded versions that are not among the newest 50.
    pub older: Vec<(i64, FeatureMap)>,
    /// The features each release of the environment touched (#576),
    /// newest first, keyed as in the map version matched with.
    pub touched: Vec<(String, Vec<crate::release_features::TouchedFeature>)>,
}

impl From<Vec<MapAt>> for History {
    fn from(versions: Vec<MapAt>) -> Self {
        Self {
            versions,
            ..Self::default()
        }
    }
}

impl History {
    /// The stored versions of `project`'s map, the releases of
    /// `environment` recorded with one and the features each touched.
    /// Unreadable rows are left out.
    #[must_use]
    pub fn load(meta: &openlens_meta::Meta, project: &str, environment: &str) -> Self {
        let versions = history(meta, project);
        let recorded = meta.release_maps(project, environment).unwrap_or_default();
        let mut older: Vec<(i64, FeatureMap)> = Vec::new();
        for r in &recorded {
            let n = r.map_version;
            if versions.iter().any(|v| v.version == n) || older.iter().any(|o| o.0 == n) {
                continue;
            }
            let map = meta
                .feature_map_version(project, n)
                .ok()
                .flatten()
                .and_then(|y| openlens_featuremap::parse(&y).map);
            if let Some(m) = map {
                older.push((n, m));
            }
        }
        Self {
            versions,
            recorded,
            older,
            touched: crate::release_features::by_release(meta, project, environment),
        }
    }

    /// The stored map `version`.
    fn map(&self, version: i64) -> Option<&FeatureMap> {
        self.versions
            .iter()
            .find(|v| v.version == version)
            .map(|v| &v.map)
            .or_else(|| self.older.iter().find(|o| o.0 == version).map(|o| &o.1))
    }
}

/// The releases of `counted` to record (D-070): those counted with a
/// stored version that is not recorded yet, under the filters every view
/// starts from (no role or release filter, the team's own users left out),
/// so a record is what that view derives. Events with no release are not
/// a release.
#[must_use]
pub fn to_record(f: &Filters, counted: &[FeatureRelease]) -> Vec<ReleaseMap> {
    if f.role.is_some() || f.release.is_some() || f.internal {
        return Vec::new();
    }
    counted
        .iter()
        .filter(|r| !r.recorded && r.release != "untagged")
        .filter_map(|r| {
            Some(ReleaseMap {
                environment: f.environment.clone(),
                release: r.release.clone(),
                first_seen: r.first_seen.clone(),
                map_version: r.map_version?,
                before_first_map: r.before_first_map,
            })
        })
        .collect()
}

/// Record the versions [`to_record`] picks from a feature's releases.
///
/// # Errors
/// The metadata store could not be written.
pub fn record(
    meta: &openlens_meta::Meta,
    project: &str,
    f: &Filters,
    counted: &[FeatureRelease],
) -> std::result::Result<usize, openlens_meta::MetaError> {
    meta.record_release_maps(project, &to_record(f, counted))
}

/// One release's use of a feature.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeatureRelease {
    /// The release (`untagged` when none was sent).
    pub release: String,
    /// When its first event (any feature) arrived.
    pub first_seen: String,
    /// Users.
    pub users: i64,
    /// Uses.
    pub uses: i64,
    /// Users who reached the success event.
    pub success_users: i64,
    /// The map version the numbers are counted with (None without stored
    /// versions: then the current map).
    pub map_version: Option<i64>,
    /// The release is older than every stored version: counted with the
    /// oldest one.
    pub before_first_map: bool,
    /// The feature was not in that version (the numbers are zero).
    pub not_in_map: bool,
    /// The version is the one recorded when the release was first counted
    /// (D-070); otherwise it was derived now.
    pub recorded: bool,
}

/// The version in force at `t` (seconds since the epoch): the latest saved
/// at or before it, else the oldest (with `true`).
#[must_use]
pub fn in_force(history: &[MapAt], t: f64) -> Option<(&MapAt, bool)> {
    let at = DateTime::<Utc>::from_timestamp_micros((t * 1e6) as i64)?;
    history
        .iter()
        .filter(|v| v.saved <= at)
        .max_by_key(|v| v.version)
        .map(|v| (v, false))
        .or_else(|| history.iter().min_by_key(|v| v.version).map(|v| (v, true)))
}

/// The key `def` had in `old` (itself, or one of its former keys).
fn key_in<'a>(old: &'a FeatureMap, key: &str, def: Option<&Feature>) -> Option<&'a Feature> {
    let names = std::iter::once(key).chain(
        def.into_iter()
            .flat_map(|d| d.renamed_from.iter().map(String::as_str)),
    );
    for n in names {
        if let Some(f) = old.features.iter().find(|f| f.key == n) {
            return Some(f);
        }
    }
    None
}

/// `key`'s use in the newest releases of the last 90 days, each with the
/// map version recorded for it, else the one in force. `current` is used
/// when no version is stored.
pub(crate) async fn releases(
    q: &QueryEngine,
    project: &str,
    current: &FeatureMap,
    history: &History,
    f: &Filters,
    key: &str,
) -> Result<Vec<FeatureRelease>> {
    let w90 = f.clause(90)?;
    let all = source(Some(current));
    let list = format!(
        "SELECT coalesce(release, 'untagged') AS release, date_part('epoch', min(timestamp)) AS t, \
         CAST(min(timestamp) AS VARCHAR) AS first_seen FROM {all} WHERE {w90} \
         GROUP BY 1 ORDER BY t DESC LIMIT {MAX_RELEASES}"
    );
    let def = current.features.iter().find(|x| x.key == key);
    let mut out = Vec::new();
    // Version (or None for the current map) → the releases it counts.
    let mut by_version: BTreeMap<Option<i64>, Vec<usize>> = BTreeMap::new();
    for r in &q.sql(project, &list).await?.rows {
        let Some(release) = text(r, "release") else {
            continue;
        };
        let t = r
            .get("t")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let kept = history.recorded.iter().find(|k| k.release == release);
        let (version, before, first_seen) = match (kept, in_force(&history.versions, t)) {
            (Some(k), _) => (
                Some(k.map_version),
                k.before_first_map,
                k.first_seen.clone(),
            ),
            (None, Some((v, b))) => (
                Some(v.version),
                b,
                text(r, "first_seen").unwrap_or_default(),
            ),
            (None, None) => (None, false, text(r, "first_seen").unwrap_or_default()),
        };
        by_version.entry(version).or_default().push(out.len());
        out.push(FeatureRelease {
            release,
            first_seen,
            map_version: version,
            before_first_map: before,
            recorded: kept.is_some(),
            ..FeatureRelease::default()
        });
    }
    for (version, idx) in by_version {
        let map = match version {
            Some(n) => match history.map(n) {
                Some(m) => m,
                None => continue,
            },
            None => current,
        };
        let Some(feat) = key_in(map, key, def) else {
            for i in idx {
                out[i].not_in_map = true;
            }
            continue;
        };
        let names: Vec<String> = idx
            .iter()
            .filter_map(|i| crate::lit(&out[*i].release).ok())
            .collect();
        if names.is_empty() {
            continue;
        }
        let src = source(Some(map));
        let k = crate::lit(&feat.key)?;
        let ok = feat
            .success
            .as_deref()
            .and_then(|s| crate::lit(s).ok())
            .unwrap_or_else(|| "NULL".to_owned());
        let sql = format!(
            "SELECT coalesce(release, 'untagged') AS release, count(DISTINCT distinct_id) AS u, \
             count(*) AS n, count(DISTINCT CASE WHEN event = {ok} THEN distinct_id END) AS s \
             FROM {src} WHERE {w90} AND feature = {k} \
             AND coalesce(release, 'untagged') IN ({}) GROUP BY 1",
            names.join(", ")
        );
        for r in &q.sql(project, &sql).await?.rows {
            let Some(name) = text(r, "release") else {
                continue;
            };
            for i in &idx {
                if out[*i].release == name {
                    out[*i].users = int(r, "u");
                    out[*i].uses = int(r, "n");
                    out[*i].success_users = int(r, "s");
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use chrono::Duration;

    #[test]
    fn the_version_in_force_is_the_latest_saved_before() {
        let now = Utc::now();
        let m = openlens_featuremap::parse("features: []\n").map.unwrap();
        let at = |version, days| MapAt {
            version,
            saved: now - Duration::days(days),
            map: m.clone(),
        };
        let h = vec![at(1, 30), at(2, 10), at(3, 1)];
        let t = |days: i64| (now - Duration::days(days)).timestamp() as f64;
        assert_eq!(
            in_force(&h, t(5)).map(|(v, b)| (v.version, b)),
            Some((2, false))
        );
        assert_eq!(
            in_force(&h, t(0)).map(|(v, b)| (v.version, b)),
            Some((3, false))
        );
        assert_eq!(
            in_force(&h, t(40)).map(|(v, b)| (v.version, b)),
            Some((1, true))
        );
        assert!(in_force(&[], t(5)).is_none());
    }
}

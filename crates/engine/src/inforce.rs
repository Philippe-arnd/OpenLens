//! A feature's numbers per release, each counted with the map version in
//! force when the release came out (#283, decision B of #240). The version
//! in force is the latest one saved before the release's first event, so
//! pushing a new map does not change the numbers of earlier releases: they
//! keep the routes and success event they shipped with. A release older
//! than every stored version is counted with the oldest one, and says so.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use openlens_featuremap::{Feature, FeatureMap};
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
/// map version in force. `current` is used when `history` is empty.
pub(crate) async fn releases(
    q: &QueryEngine,
    project: &str,
    current: &FeatureMap,
    history: &[MapAt],
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
        let (version, before) = match in_force(history, t) {
            Some((v, b)) => (Some(v.version), b),
            None => (None, false),
        };
        by_version.entry(version).or_default().push(out.len());
        out.push(FeatureRelease {
            release,
            first_seen: text(r, "first_seen").unwrap_or_default(),
            map_version: version,
            before_first_map: before,
            ..FeatureRelease::default()
        });
    }
    for (version, idx) in by_version {
        let map = match version {
            Some(n) => match history.iter().find(|v| v.version == n) {
                Some(v) => &v.map,
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

//! Drift between the pushed map and what the app does (#283): routes that
//! appear with the newest release and that no feature claims. The newest
//! release is the one whose first event is the latest, in the last 90
//! days; a route is new when no page of it was seen before that first
//! event. A map health finding (`drift`) with the feature to add.

use std::collections::{BTreeMap, HashSet};

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;

use crate::maphealth::{Finding, key_for, route_for};
use crate::{Filters, Result, int, source, text};

/// Paths of the newest release read at most.
const MAX_PATHS: usize = 2000;
/// Earlier paths read at most: past it, nothing is called new.
const MAX_EARLIER: usize = 20_000;
/// Drift findings kept.
const MAX_FINDINGS: usize = 10;
/// A route must weigh this share of the release's pages and clicks.
const MIN_SHARE: f64 = 0.01;

/// What was observed for the newest release.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    /// The newest release, if any was tagged.
    pub release: Option<String>,
    /// Its pages and clicks: (path, events, claimed by a feature).
    pub paths: Vec<(String, i64, bool)>,
    /// Route patterns seen before its first event, or None when there were
    /// too many to read them all (then nothing is called new).
    pub earlier: Option<HashSet<String>>,
}

/// Read the newest release's paths and the routes seen before it.
pub(crate) async fn load(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    f: &Filters,
) -> Result<Observed> {
    // The newest release is a project fact: a release filter does not apply.
    let all = Filters {
        release: None,
        ..f.clone()
    };
    let w90 = all.clause(90)?;
    let src = source(Some(m));
    let newest = format!(
        "SELECT release, date_part('epoch', min(timestamp)) AS t FROM {src} \
         WHERE {w90} AND release IS NOT NULL GROUP BY release ORDER BY t DESC LIMIT 1"
    );
    let rows = q.sql(project, &newest).await?.rows;
    let Some(row) = rows.first() else {
        return Ok(Observed::default());
    };
    let (Some(release), Some(t)) = (
        text(row, "release"),
        row.get("t").and_then(serde_json::Value::as_f64),
    ) else {
        return Ok(Observed::default());
    };
    let Ok(rel) = crate::lit(&release) else {
        return Ok(Observed::default());
    };
    let paths = format!(
        "SELECT path, feature IS NOT NULL AS claimed, count(*) AS n FROM {src} \
         WHERE {w90} AND release = {rel} AND event IN ('$pageview', '$click') AND path IS NOT NULL \
         GROUP BY 1, 2 ORDER BY n DESC LIMIT {MAX_PATHS}"
    );
    let earlier = format!(
        "SELECT DISTINCT path FROM {src} WHERE {w90} AND path IS NOT NULL \
         AND date_part('epoch', timestamp) < {t} LIMIT {MAX_EARLIER}"
    );
    let (p, e) = futures::join!(q.sql(project, &paths), q.sql(project, &earlier));
    let paths = p?
        .rows
        .iter()
        .filter_map(|r| {
            let claimed = r
                .get("claimed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Some((text(r, "path")?, int(r, "n"), claimed))
        })
        .collect();
    let e = e?.rows;
    let earlier = (e.len() < MAX_EARLIER).then(|| {
        e.iter()
            .filter_map(|r| text(r, "path"))
            .map(|p| route_for(&p))
            .collect()
    });
    Ok(Observed {
        release: Some(release),
        paths,
        earlier,
    })
}

/// The drift findings of `o`, each with its route (pure).
#[must_use]
pub fn findings(o: &Observed) -> Vec<(String, Finding)> {
    let (Some(release), Some(earlier)) = (&o.release, &o.earlier) else {
        return Vec::new();
    };
    let total: i64 = o.paths.iter().map(|(_, n, _)| n).sum();
    let mut new: BTreeMap<String, i64> = BTreeMap::new();
    for (path, n, claimed) in &o.paths {
        let route = route_for(path);
        if !claimed && !earlier.contains(&route) {
            *new.entry(route).or_default() += n;
        }
    }
    let mut new: Vec<(String, i64)> = new.into_iter().collect();
    new.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    new.into_iter()
        .filter(|(_, n)| total > 0 && *n as f64 >= MIN_SHARE * total as f64)
        .take(MAX_FINDINGS)
        .map(|(route, n)| {
            let key = key_for(&route);
            let f = Finding {
                kind: "drift",
                level: "warning",
                feature: None,
                message: format!("`{route}` is new in release `{release}` and no feature claims it"),
                evidence: format!(
                    "{n} pages and clicks in `{release}` ({:.0}% of the release), none before it",
                    100.0 * n as f64 / total as f64
                ),
                fix: Some(format!(
                    "# A new feature of this release, or a new route of an existing one:\n- key: {key}\n  name: {}\n  entry:\n    routes: [\"{route}\"]",
                    key.replace('-', " ")
                )),
                impact: n,
            };
            (route, f)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_unclaimed_routes_drift() {
        let o = Observed {
            release: Some("v2".into()),
            paths: vec![
                ("/reports/1".into(), 40, false),
                ("/reports/2".into(), 30, false),
                ("/old".into(), 50, false),
                ("/docs".into(), 80, true),
            ],
            earlier: Some(["/old".to_owned(), "/docs".to_owned()].into()),
        };
        let f = findings(&o);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].0, "/reports/:id");
        assert!(
            f[0].1
                .fix
                .as_deref()
                .is_some_and(|x| x.contains("/reports/:id"))
        );
        // Without the full list of earlier routes, nothing is called new.
        assert!(findings(&Observed { earlier: None, ..o }).is_empty());
    }
}

//! What each session did with the features of the map, last 30 days
//! (#283): the input of the split, merge and step order suggestions (the
//! relations are added up in SQL, `mapdays`, #483). One bounded read, like
//! `flows.rs`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;

use crate::{Filters, Result, source, text};

/// Rows read at most (one per session, feature and page or event).
const MAX_ROWS: usize = 300_000;

/// One feature inside one session.
#[derive(Debug, Clone, Default)]
pub struct Visit {
    /// When the session first used it (seconds since the epoch).
    pub first: f64,
    /// Pages (as route patterns, starting with `/`) and the app's own
    /// events (names) seen with it, each with its first time.
    pub units: BTreeMap<String, f64>,
}

/// One session.
#[derive(Debug, Clone, Default)]
pub struct Session {
    /// Who.
    pub user: String,
    /// Feature key → its visit.
    pub features: BTreeMap<String, Visit>,
    /// Success events (of any feature) that fired in the session.
    pub successes: BTreeSet<String>,
}

/// Session id → what it did.
pub type Sessions = HashMap<String, Session>;

/// Read the sessions of the last 30 days that used a feature.
pub(crate) async fn load(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    f: &Filters,
) -> Result<Sessions> {
    let src = source(Some(m));
    let w30 = f.clause(30)?;
    let units = format!(
        "SELECT session_id, min(distinct_id) AS who, feature, \
         CASE WHEN event = '$pageview' THEN path WHEN event NOT LIKE '$%' THEN event END AS unit, \
         date_part('epoch', min(timestamp)) AS t \
         FROM {src} WHERE {w30} AND feature IS NOT NULL AND session_id IS NOT NULL \
         GROUP BY session_id, feature, 4 LIMIT {MAX_ROWS}"
    );
    let successes: Vec<String> = m
        .features
        .iter()
        .filter_map(|x| x.success.as_deref())
        .filter_map(|s| crate::lit(s).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut out = Sessions::new();
    for r in &q.sql(project, &units).await?.rows {
        let (Some(sid), Some(feat)) = (text(r, "session_id"), text(r, "feature")) else {
            continue;
        };
        let t = r
            .get("t")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let s = out.entry(sid).or_default();
        if s.user.is_empty() {
            s.user = text(r, "who").unwrap_or_default();
        }
        let v = s.features.entry(feat).or_insert_with(|| Visit {
            first: t,
            units: BTreeMap::new(),
        });
        v.first = v.first.min(t);
        if let Some(u) = text(r, "unit") {
            let u = if u.starts_with('/') {
                crate::maphealth::route_for(&u)
            } else {
                u
            };
            let e = v.units.entry(u).or_insert(t);
            *e = e.min(t);
        }
    }
    if !successes.is_empty() && !out.is_empty() {
        let sql = format!(
            "SELECT session_id, event FROM {src} WHERE {w30} AND session_id IS NOT NULL \
             AND event IN ({}) GROUP BY 1, 2 LIMIT {MAX_ROWS}",
            successes.join(", ")
        );
        for r in &q.sql(project, &sql).await?.rows {
            if let (Some(sid), Some(e)) = (text(r, "session_id"), text(r, "event"))
                && let Some(s) = out.get_mut(&sid)
            {
                s.successes.insert(e);
            }
        }
    }
    Ok(out)
}

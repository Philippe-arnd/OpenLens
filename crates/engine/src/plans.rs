//! Plans (#712, D-103): upgrades and downgrades per period and per
//! release, and the raw plan values no plan of the map matches (plan gaps).
//!
//! Everything here needs a `plans:` section in the feature map: an app that
//! sends no plan (an internal tool) or declares no plans sees nothing new.
//! A move is read from the `$identify` events, the rows the plan history
//! (`plan_history` in the metadata store) is written from: each one carries
//! the plan the user had then, and the release they were on ("plan at event
//! time").

use std::collections::BTreeMap;

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, int, text};

/// Releases listed with their moves, newest first.
const MAX_RELEASES: usize = 5;

/// Upgrades and downgrades of one release.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanRelease {
    /// Release name.
    pub release: String,
    /// Moves to a higher plan (or free to paid) on it, last 90 days.
    pub upgrades: i64,
    /// Moves to a lower plan (or paid to free) on it, last 90 days.
    pub downgrades: i64,
}

/// Plan moves (#712): shown only when the map declares plans.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanMoves {
    /// Upgrades in the last 30 days: a move to a higher plan in the map's
    /// order, or from a free plan to a paid one.
    pub upgrades: i64,
    /// Downgrades in the last 30 days: the reverse.
    pub downgrades: i64,
    /// Upgrades in the 30 days before.
    pub prev_upgrades: i64,
    /// Downgrades in the 30 days before.
    pub prev_downgrades: i64,
    /// Per release (the release the user was on when the new plan was
    /// sent), last 90 days, newest first, at most 5.
    pub releases: Vec<PlanRelease>,
    /// Raw plan values no plan of the map matches, with their users (last
    /// 30 days): plan gaps, kept as `unmatched:<value>` and never counted
    /// as a move.
    pub gaps: Vec<(String, i64)>,
}

/// Upgrades and downgrades over the last 90 days, from each user's
/// successive plans, and the plan gaps of the last 30 days. Every
/// environment filter applies; the role and release filters do not (a
/// move is a person's, and the release is its own column here).
pub(crate) async fn moves(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    m: &FeatureMap,
) -> Result<PlanMoves> {
    let q = &q.with_params(&f.params()?);
    let all = Filters {
        role: None,
        release: None,
        ..f.clone()
    };
    let (w90, w30) = (all.clause(90)?, all.clause(30)?);
    let changes = format!(
        "WITH ids AS (SELECT distinct_id, timestamp, plan, release FROM events \
                      WHERE {w90} AND event = '$identify' AND plan IS NOT NULL), \
         ch AS (SELECT timestamp, release, plan, \
                lag(plan) OVER (PARTITION BY distinct_id ORDER BY timestamp) AS prev FROM ids), \
         moved AS (SELECT prev, plan, release, timestamp, \
                   CASE WHEN timestamp >= now() - INTERVAL '30 days' THEN 0 \
                        WHEN timestamp >= now() - INTERVAL '60 days' THEN 1 ELSE 2 END AS bucket \
                   FROM ch WHERE prev IS NOT NULL AND prev <> plan) \
         SELECT prev, plan, release, bucket, count(*) AS n, CAST(min(timestamp) AS VARCHAR) AS first \
         FROM moved GROUP BY prev, plan, release, bucket LIMIT 5000"
    );
    let gaps = format!(
        "SELECT plan, count(DISTINCT distinct_id) AS users FROM events \
         WHERE {w30} AND plan IS NOT NULL GROUP BY plan ORDER BY users DESC LIMIT 200"
    );
    let (changes, gaps) = futures::join!(q.sql(project, &changes), q.sql(project, &gaps));
    let mut out = PlanMoves::default();
    // release → (first move seen, upgrades, downgrades)
    let mut by_release: BTreeMap<String, (String, i64, i64)> = BTreeMap::new();
    for r in &changes?.rows {
        let (Some(from), Some(to)) = (text(r, "prev"), text(r, "plan")) else {
            continue;
        };
        let Some(up) = m.plan_move(&from, &to) else {
            continue;
        };
        let n = int(r, "n");
        match (int(r, "bucket"), up) {
            (0, true) => out.upgrades += n,
            (0, false) => out.downgrades += n,
            (1, true) => out.prev_upgrades += n,
            (1, false) => out.prev_downgrades += n,
            _ => {}
        }
        if let Some(release) = text(r, "release") {
            let first = text(r, "first").unwrap_or_default();
            let e = by_release
                .entry(release)
                .or_insert_with(|| (first.clone(), 0, 0));
            if first < e.0 {
                e.0 = first;
            }
            if up {
                e.1 += n;
            } else {
                e.2 += n;
            }
        }
    }
    let mut releases: Vec<(String, (String, i64, i64))> = by_release.into_iter().collect();
    releases.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    out.releases = releases
        .into_iter()
        .take(MAX_RELEASES)
        .map(|(release, (_, upgrades, downgrades))| PlanRelease {
            release,
            upgrades,
            downgrades,
        })
        .collect();
    for r in &gaps?.rows {
        if let Some(raw) = text(r, "plan")
            && m.resolve_plan(&raw).is_none()
        {
            out.gaps.push((raw, int(r, "users")));
        }
    }
    Ok(out)
}

/// The board's plan block: the moves, or nothing without plans in the
/// map.
pub(crate) async fn read(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
) -> Result<Option<PlanMoves>> {
    match map.filter(|m| !m.plans.is_empty()) {
        Some(m) => Ok(Some(moves(q, project, f, m).await?)),
        None => Ok(None),
    }
}

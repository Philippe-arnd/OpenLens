//! Plans (#712, #713, D-103): upgrades and downgrades per period and per
//! release, the raw plan values no plan of the map matches (plan gaps),
//! and the conversion drivers card.
//!
//! Everything here needs a `plans:` section in the feature map, and the
//! card also a free plan and a paid one: an app that sends no plan (an
//! internal tool) or declares no plans sees nothing new. A move is read
//! from the `$identify` events, the rows the plan history (`plan_history`
//! in the metadata store) is written from: each one carries the plan the
//! user had then, and the release they were on ("plan at event time").

use std::collections::BTreeMap;

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::insights::{Insight, MIN_GROUP};
use crate::{Filters, Result, int, text};

/// Releases listed with their moves, newest first.
const MAX_RELEASES: usize = 5;
/// Days after a free user's first visit within which an upgrade counts as
/// a conversion (#713).
const CONVERSION_DAYS: i64 = 30;

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

/// The board's plan blocks: the moves and the conversion drivers cards,
/// or nothing without plans in the map.
pub(crate) async fn read(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
    people90: &str,
    ident: &str,
) -> Result<(Option<PlanMoves>, Vec<Insight>)> {
    let Some(m) = map.filter(|m| !m.plans.is_empty()) else {
        return Ok((None, Vec::new()));
    };
    let (moved, cards) = futures::try_join!(
        moves(q, project, f, m),
        conversion_drivers(q, project, f, m, people90, ident)
    )?;
    Ok((Some(moved), cards))
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

/// Conversion drivers (#713): features used in the first week of free
/// users that lift the share who move to a paid plan within 30 days of
/// their first visit. The stick drivers' cohort and minimums (8 users per
/// group), with a two-proportion test; shown only when the map tells free
/// from paid plans and at least 8 free users converted.
pub(crate) async fn conversion_drivers(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    m: &FeatureMap,
    people90: &str,
    ident: &str,
) -> Result<Vec<Insight>> {
    let Some(paid) = m.plan_paid_sql() else {
        return Ok(Vec::new());
    };
    let q = &q.with_params(&f.params()?);
    let w = Filters {
        role: None,
        release: None,
        ..f.clone()
    }
    .clause(90)?;
    // Users first seen 30 to 89 days ago: their 30 days are over.
    let base = crate::insights::rolled_cohort_base(people90, ident, CONVERSION_DAYS);
    let free = format!(
        "{base}, plan_marks AS (SELECT distinct_id, timestamp, {paid} AS paid FROM events \
                                WHERE {w} AND event = '$identify' AND plan IS NOT NULL), \
         plan_times AS (SELECT distinct_id, min(CASE WHEN paid = 0 THEN timestamp END) AS free_at, \
                        min(CASE WHEN paid = 1 THEN timestamp END) AS paid_at \
                        FROM plan_marks GROUP BY distinct_id), \
         free_users AS (SELECT c.distinct_id, \
                        CASE WHEN p.paid_at IS NOT NULL \
                             AND p.paid_at <= c.first_seen + INTERVAL '{CONVERSION_DAYS} days' \
                             THEN 1 ELSE 0 END AS converted \
                        FROM cohort c JOIN plan_times p ON c.distinct_id = p.distinct_id \
                        WHERE p.free_at IS NOT NULL AND p.free_at <= c.first_seen + INTERVAL '7 days' \
                        AND (p.paid_at IS NULL OR p.paid_at > p.free_at))"
    );
    let totals = format!("{free} SELECT count(*) AS n, sum(converted) AS conv FROM free_users");
    let early = format!(
        "{free}, early AS (SELECT e.distinct_id, e.feature FROM ev e JOIN cohort c ON e.distinct_id = c.distinct_id \
           WHERE e.feature IS NOT NULL AND e.first_ts <= c.first_seen + INTERVAL '7 days' \
           GROUP BY e.distinct_id, e.feature) \
         SELECT early.feature AS feature, count(*) AS users, sum(fu.converted) AS conv \
         FROM early JOIN free_users fu ON early.distinct_id = fu.distinct_id GROUP BY early.feature"
    );
    let (totals, early) = futures::join!(q.sql(project, &totals), q.sql(project, &early));
    let t = totals?.rows;
    let n_all = t.first().map_or(0, |r| int(r, "n"));
    let conv_all = t.first().map_or(0, |r| int(r, "conv"));
    Ok(conversion_cards(
        n_all,
        conv_all,
        &early?
            .rows
            .iter()
            .filter_map(|r| Some((text(r, "feature")?, int(r, "users"), int(r, "conv"))))
            .collect::<Vec<_>>(),
    ))
}

fn pct(r: f64) -> String {
    format!("{:.0}%", r * 100.0)
}

/// The conversion drivers cards from the free users' numbers: `n_all`
/// free users, `conv_all` of whom converted, and per feature used in the
/// first week `(feature, users, converted)`. Pure, so it is tested
/// without data files.
pub(crate) fn conversion_cards(
    n_all: i64,
    conv_all: i64,
    early: &[(String, i64, i64)],
) -> Vec<Insight> {
    let mut cards = Vec::new();
    if conv_all < MIN_GROUP || n_all - conv_all < MIN_GROUP {
        return cards;
    }
    for (feat, with, conv_with) in early {
        let (with, conv_with) = (*with, *conv_with);
        let (without, conv_without) = (n_all - with, conv_all - conv_with);
        if with < MIN_GROUP || without < MIN_GROUP {
            continue;
        }
        let (ra, rb) = (
            conv_with as f64 / with as f64,
            conv_without as f64 / without as f64,
        );
        if ra <= rb {
            continue;
        }
        let Some(p) = crate::stats::two_proportions_min(ra, with, rb, without, MIN_GROUP)
            .filter(|p| *p < crate::stats::ALPHA)
        else {
            continue;
        };
        let lift = if rb > 0.0 {
            format!("{:.1}× as often", ra / rb)
        } else {
            "far more often".to_owned()
        };
        cards.push(Insight {
            kind: "conversion_driver",
            title: format!(
                "Free users who use {feat} in their first week move to a paid plan {lift}"
            ),
            evidence: format!(
                "{} of {with} free users who used it in week 1 upgraded within {CONVERSION_DAYS} days, vs {} of {without} who did not (p = {p:.3}); {conv_all} of {n_all} free users upgraded",
                pct(ra),
                pct(rb)
            ),
            next_step: format!(
                "Bring new free users to {feat} in their first week (onboarding, empty states), then check whether more of them upgrade."
            ),
            score: (ra - rb) * (with.min(without) as f64).sqrt() * 2.0,
            subject: feat.clone(),
            figure: (rb > 0.0).then(|| ra / rb),
        });
    }
    cards.sort_by(|a, b| b.score.total_cmp(&a.score));
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_feature_that_lifts_conversion_ranks_first_and_chance_does_not() {
        // 200 free users, 40 converted. `invite`: 30 of 60 users converted
        // (50% vs 7%); `search`: everyone's rate; `tiny`: too few users.
        let early = vec![
            ("search".to_owned(), 100, 20),
            ("invite".to_owned(), 60, 30),
            ("tiny".to_owned(), 5, 5),
        ];
        let cards = conversion_cards(200, 40, &early);
        assert_eq!(
            cards.first().map(|c| c.subject.as_str()),
            Some("invite"),
            "{cards:?}"
        );
        assert!(
            cards
                .iter()
                .all(|c| c.subject != "search" && c.subject != "tiny")
        );
        // Fewer than 8 converted: no card at all.
        assert!(conversion_cards(200, 7, &early).is_empty());
    }
}

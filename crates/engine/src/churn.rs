//! Churn as one section (#247), around the PM's questions: how many am I
//! losing, after how long, at which moment, with what friction, and who is
//! at risk now. A user has churned after `churn_after_days` without an
//! event (docs/kpis.md). Last 90 days, identified users.

use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, INTERACTION, Result, any_person, identified, int, source, text};

/// A user at risk: active within the window, quiet for over half of it,
/// and less active than in the window before.
#[derive(Debug, Clone, Serialize)]
pub struct AtRisk {
    /// User id.
    pub distinct_id: String,
    /// Raw role, if any.
    pub role: Option<String>,
    /// Last active day (YYYY-MM-DD).
    pub last_day: String,
    /// Days since then.
    pub idle_days: i64,
    /// Active days in the last window vs the one before.
    pub recent_days: i64,
    /// Active days in the window before.
    pub before_days: i64,
}

/// The churn section's numbers.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChurnReport {
    /// The churn window used (days).
    pub window: i64,
    /// A window suggested by the data (the 90th percentile of gaps between
    /// active days, rounded to 7, 14, 21, 30, 45 or 60), when enough data.
    pub suggested: Option<i64>,
    /// Users active at the start of the last window.
    pub active_at_start: i64,
    /// Of them, churned since.
    pub churned_recent: i64,
    /// New users in the last window.
    pub new_recent: i64,
    /// Users who churned and came back in the last 30 days.
    pub resurrected: i64,
    /// (week start YYYY-MM-DD, users whose last active day fell that week
    /// and who have churned), oldest first.
    pub by_week: Vec<(String, i64)>,
    /// Survival: (days since first visit, share still active), Kaplan–Meier.
    pub survival: Vec<(i64, f64)>,
    /// Median lifetime in days, when the curve crosses one half.
    pub median_days: Option<i64>,
    /// Churned users by week of life at departure (0 = first week).
    pub leave_week: Vec<(i64, i64)>,
    /// Last place before leaving: (feature or page, churned users, share
    /// of churned users, share of retained users' last place).
    pub last_place: Vec<(String, i64, f64, f64)>,
    /// Share of churned users with an error in their last active day, and
    /// the same for retained users.
    pub errors_at_end: (f64, f64),
    /// Users at risk now, quietest first.
    pub at_risk: Vec<AtRisk>,
    /// Users counted.
    pub users: i64,
}

fn day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// Kaplan–Meier survival from (lifetime in days, churned?) pairs, at the
/// given days.
#[must_use]
pub fn survival(lives: &[(i64, bool)], at: &[i64]) -> Vec<(i64, f64)> {
    let mut times: Vec<i64> = lives.iter().filter(|l| l.1).map(|l| l.0).collect();
    times.sort_unstable();
    times.dedup();
    let mut s = 1.0;
    let mut curve: Vec<(i64, f64)> = vec![(0, 1.0)];
    for t in times {
        let at_risk = lives.iter().filter(|l| l.0 >= t).count() as f64;
        let died = lives.iter().filter(|l| l.1 && l.0 == t).count() as f64;
        if at_risk > 0.0 {
            s *= 1.0 - died / at_risk;
        }
        curve.push((t, s));
    }
    at.iter()
        .map(|&d| {
            let v = curve
                .iter()
                .rev()
                .find(|(t, _)| *t <= d)
                .map_or(1.0, |c| c.1);
            (d, v)
        })
        .collect()
}

/// The churn report of a project.
pub async fn churn(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
) -> Result<ChurnReport> {
    let src = source(map);
    let w = f.clause(90)?;
    let ident = identified(any_person(q, project).await?);
    let c = map.map_or(14, |m| i64::from(m.churn_days()));
    let users_sql = format!(
        "SELECT distinct_id, max(role) AS role, CAST(min(timestamp) AS VARCHAR) AS first, \
         CAST(max(timestamp) AS VARCHAR) AS last, \
         count(DISTINCT CASE WHEN timestamp >= now() - INTERVAL '{c} days' THEN date END) AS recent, \
         count(DISTINCT CASE WHEN timestamp < now() - INTERVAL '{c} days' \
               AND timestamp >= now() - INTERVAL '{c2} days' THEN date END) AS before \
         FROM {src} WHERE {w} AND {ident} AND {INTERACTION} GROUP BY distinct_id",
        c2 = 2 * c
    );
    let days_sql = format!(
        "SELECT distinct_id, date FROM {src} WHERE {w} AND {ident} AND {INTERACTION} \
         GROUP BY distinct_id, date ORDER BY distinct_id, date LIMIT 500000"
    );
    let last_sql = format!(
        "WITH l AS (SELECT distinct_id, max(timestamp) AS t FROM {src} WHERE {w} AND {ident} AND {INTERACTION} GROUP BY distinct_id) \
         SELECT e.distinct_id, max(coalesce(e.feature, e.path)) AS place, \
                max(CASE WHEN ev.event = '$error' THEN 1 ELSE 0 END) AS err \
         FROM {src} e JOIN l ON e.distinct_id = l.distinct_id AND e.timestamp = l.t \
         LEFT JOIN (SELECT distinct_id, event, date FROM {src} WHERE {w} AND event = '$error') ev \
           ON ev.distinct_id = e.distinct_id AND ev.date = e.date \
         GROUP BY e.distinct_id"
    );
    let (u, d, l, resurrected) = futures::join!(
        q.sql(project, &users_sql),
        q.sql(project, &days_sql),
        q.sql(project, &last_sql),
        crate::tasks::resurrected(q, project, &src, f, c),
    );
    let today = Utc::now().date_naive();
    let mut out = ChurnReport {
        window: c,
        resurrected: resurrected.unwrap_or(0),
        ..ChurnReport::default()
    };
    let mut churned: HashMap<String, bool> = HashMap::new();
    let mut lives: Vec<(i64, bool)> = Vec::new();
    let mut weeks: HashMap<String, i64> = HashMap::new();
    let mut leave: HashMap<i64, i64> = HashMap::new();
    let start = today - chrono::Duration::days(c);
    for r in &u?.rows {
        let (Some(id), Some(first), Some(last)) = (
            text(r, "distinct_id"),
            text(r, "first").as_deref().and_then(day),
            text(r, "last").as_deref().and_then(day),
        ) else {
            continue;
        };
        out.users += 1;
        let idle = (today - last).num_days();
        let gone = idle > c;
        churned.insert(id.clone(), gone);
        let life = (last - first).num_days();
        lives.push((life, gone));
        if first >= start {
            out.new_recent += 1;
        }
        if first < start && last >= start - chrono::Duration::days(c) {
            // Active in the window before the last one: counted at its start.
            out.active_at_start += 1;
            if gone {
                out.churned_recent += 1;
            }
        }
        if gone {
            let monday = last
                - chrono::Duration::days(i64::from(
                    chrono::Datelike::weekday(&last).num_days_from_monday(),
                ));
            *weeks
                .entry(monday.format("%Y-%m-%d").to_string())
                .or_default() += 1;
            *leave.entry((life / 7).min(12)).or_default() += 1;
        } else {
            let recent = int(r, "recent");
            let before = int(r, "before");
            if idle * 2 > c && recent < before {
                out.at_risk.push(AtRisk {
                    distinct_id: id,
                    role: text(r, "role"),
                    last_day: last.format("%Y-%m-%d").to_string(),
                    idle_days: idle,
                    recent_days: recent,
                    before_days: before,
                });
            }
        }
    }
    out.at_risk.sort_by(|a, b| {
        b.idle_days
            .cmp(&a.idle_days)
            .then(a.distinct_id.cmp(&b.distinct_id))
    });
    out.at_risk.truncate(100);
    let mut by_week: Vec<(String, i64)> = weeks.into_iter().collect();
    by_week.sort();
    out.by_week = by_week;
    let mut lw: Vec<(i64, i64)> = leave.into_iter().collect();
    lw.sort_unstable();
    out.leave_week = lw;
    let points: Vec<i64> = [0, 1, 2, 3, 7, 14, 21, 30, 45, 60, 75, 90].to_vec();
    out.survival = survival(&lives, &points);
    out.median_days = {
        let fine: Vec<i64> = (0..=90).collect();
        survival(&lives, &fine)
            .into_iter()
            .find(|(_, s)| *s < 0.5)
            .map(|(d, _)| d)
    };
    // Gaps between active days → a suggested window.
    let mut gaps: Vec<i64> = Vec::new();
    let mut prev: Option<(String, NaiveDate)> = None;
    for r in &d?.rows {
        let (Some(id), Some(dt)) = (
            text(r, "distinct_id"),
            text(r, "date").as_deref().and_then(day),
        ) else {
            continue;
        };
        if let Some((pid, pd)) = &prev
            && *pid == id
        {
            gaps.push((dt - *pd).num_days());
        }
        prev = Some((id, dt));
    }
    if gaps.len() >= 50 {
        gaps.sort_unstable();
        let p90 = gaps[(gaps.len() - 1) * 9 / 10];
        out.suggested = [7, 14, 21, 30, 45, 60]
            .into_iter()
            .find(|&s| s >= p90)
            .or(Some(60));
    }
    // Last place and errors at the end, churned vs retained.
    let mut place_c: HashMap<String, i64> = HashMap::new();
    let mut place_r: HashMap<String, i64> = HashMap::new();
    let (mut n_c, mut n_r, mut err_c, mut err_r) = (0i64, 0i64, 0i64, 0i64);
    for r in &l?.rows {
        let Some(id) = text(r, "distinct_id") else {
            continue;
        };
        let Some(&gone) = churned.get(&id) else {
            continue;
        };
        let place = text(r, "place").unwrap_or_else(|| "(none)".into());
        let err = int(r, "err") > 0;
        if gone {
            n_c += 1;
            err_c += i64::from(err);
            *place_c.entry(place).or_default() += 1;
        } else {
            n_r += 1;
            err_r += i64::from(err);
            *place_r.entry(place).or_default() += 1;
        }
    }
    let mut lp: Vec<(String, i64, f64, f64)> = place_c
        .into_iter()
        .map(|(p, n)| {
            let base = place_r.get(&p).copied().unwrap_or(0) as f64 / n_r.max(1) as f64;
            (p, n, n as f64 / n_c.max(1) as f64, base)
        })
        .collect();
    lp.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    lp.truncate(10);
    out.last_place = lp;
    out.errors_at_end = (
        err_c as f64 / n_c.max(1) as f64,
        err_r as f64 / n_r.max(1) as f64,
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::survival;

    #[test]
    fn kaplan_meier_counts_recent_users_correctly() {
        // Two churned at day 5 and 10; two still active (censored) at day 3 and 20.
        let lives = [(5, true), (10, true), (3, false), (20, false)];
        let s = survival(&lives, &[0, 4, 5, 10, 30]);
        assert_eq!(s[0], (0, 1.0));
        assert!((s[1].1 - 1.0).abs() < 1e-9);
        // At day 5: 3 at risk (3 is censored before), 1 dies → 2/3.
        assert!((s[2].1 - 2.0 / 3.0).abs() < 1e-9);
        // At day 10: 2 at risk, 1 dies → 2/3 × 1/2.
        assert!((s[3].1 - 1.0 / 3.0).abs() < 1e-9);
        assert!((s[4].1 - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(survival(&[], &[7]), vec![(7, 1.0)]);
    }
}

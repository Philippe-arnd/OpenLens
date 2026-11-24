//! Goals on KPIs (#235): a target, a direction and a deadline on a board
//! KPI; OpenLens suggests the target, shows the gap and progress, and
//! projects the trend to the deadline (on track, at risk, off track).
//! Part 2 (#272): weekly series for the KPIs releases do not give one,
//! Web Vitals, guardrails, objectives with key results, deadlines in
//! releases, and what moved a goal.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Datelike, NaiveDate};
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::{Deserialize, Serialize};

use crate::{Board, Filters, Result, float, int, source, text};

/// KPIs a goal can target: (key, label, higher is better, is a share).
pub const KPIS: &[(&str, &str, bool, bool)] = &[
    ("error_rate", "Sessions with errors", false, true),
    ("bounce_rate", "Bounce rate", false, true),
    ("avg_secs", "Average session (s)", true, false),
    ("tasks_per_session", "Tasks per session", true, false),
    ("stickiness", "Stickiness (DAU ÷ MAU)", true, true),
    ("dau", "Daily active users", true, false),
    ("mau", "Monthly active users", true, false),
    ("lcp_p75", "Largest paint p75 (ms)", false, false),
    ("inp_p75", "Interaction delay p75 (ms)", false, false),
    ("cls_p75", "Layout shift p75", false, false),
];

/// A KPI that must not get worse while a goal is pursued (#272).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Guardrail {
    /// KPI key (as for a goal).
    pub kpi: String,
    /// The limit: worse than this is a breach.
    pub limit: f64,
    /// Higher is better for this KPI.
    pub up: bool,
}

impl Guardrail {
    /// The default limit: 5% worse than `baseline` (relative; a share
    /// moves by at least one point).
    #[must_use]
    pub fn suggested(baseline: f64, up: bool, share: bool) -> f64 {
        let step = (baseline.abs() * 0.05).max(if share { 0.01 } else { 0.0 });
        let v = if up { baseline - step } else { baseline + step };
        if share { v.clamp(0.0, 1.0) } else { v.max(0.0) }
    }

    /// Is `current` worse than the limit?
    #[must_use]
    pub fn breached(&self, current: f64) -> bool {
        if self.up {
            current < self.limit
        } else {
            current > self.limit
        }
    }
}

/// An objective (#272): a sentence, 1–3 goals as its key results, and a
/// period. Archived automatically once the period is over.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Objective {
    /// Id.
    pub id: String,
    /// What we want, in words.
    pub title: String,
    /// Goal ids (1–3).
    pub goals: Vec<String>,
    /// First day (YYYY-MM-DD).
    pub start: String,
    /// Last day (YYYY-MM-DD).
    pub end: String,
    /// Set when the period is over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<Archived>,
}

/// How an objective ended.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Archived {
    /// When (YYYY-MM-DD).
    pub at: String,
    /// Every key result reached.
    pub achieved: bool,
    /// (goal id, KPI label, final value, target, reached).
    pub results: Vec<(String, String, Option<f64>, f64, bool)>,
}

/// An objective's standing from its goals' statuses: the worst one.
#[must_use]
pub fn objective_status(statuses: &[Status]) -> Status {
    let rank = |s: &Status| match s {
        Status::OffTrack => 0,
        Status::AtRisk => 1,
        Status::NotEnoughData => 2,
        Status::OnTrack => 3,
        Status::Reached => 4,
    };
    statuses
        .iter()
        .copied()
        .min_by_key(rank)
        .unwrap_or(Status::NotEnoughData)
}

/// A deadline `n` releases from now: today plus `n` times the median gap
/// between the releases seen (None under two releases).
#[must_use]
pub fn deadline_in_releases(firsts: &[String], n: u32, today: NaiveDate) -> Option<NaiveDate> {
    let mut days: Vec<NaiveDate> = firsts
        .iter()
        .filter_map(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok())
        .collect();
    days.sort_unstable();
    days.dedup();
    let mut gaps: Vec<i64> = days.windows(2).map(|w| (w[1] - w[0]).num_days()).collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    let median = gaps[gaps.len() / 2].max(1);
    today.checked_add_days(chrono::Days::new(
        u64::try_from(median).ok()? * u64::from(n),
    ))
}

/// One release's effect on a goal's KPI (#272 "what moved it").
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Move {
    /// Release name.
    pub release: String,
    /// First seen (YYYY-MM-DD).
    pub day: String,
    /// Change from the release before (in KPI units).
    pub delta: f64,
    /// The change went the right way.
    pub good: bool,
}

/// Releases since the goal was set, ranked by how much they moved its KPI.
#[must_use]
pub fn movers(b: &Board, goal: &Goal) -> Vec<Move> {
    let (_, series) = kpi_values(b, &goal.kpi);
    let names: HashMap<String, String> = b
        .releases
        .iter()
        .map(|r| {
            (
                r.first_seen.get(..10).unwrap_or(&r.first_seen).to_owned(),
                r.release.clone(),
            )
        })
        .collect();
    let mut out: Vec<Move> = series
        .windows(2)
        .filter(|w| w[1].0 >= goal.created)
        .filter_map(|w| {
            let delta = w[1].1 - w[0].1;
            (delta.abs() > 1e-12).then(|| Move {
                release: names
                    .get(&w[1].0)
                    .cloned()
                    .unwrap_or_else(|| w[1].0.clone()),
                day: w[1].0.clone(),
                delta,
                good: (delta > 0.0) == goal.up,
            })
        })
        .collect();
    out.sort_by(|a, b| b.delta.abs().total_cmp(&a.delta.abs()));
    out.truncate(3);
    out
}

/// Weekly points (Monday, value), last 12 weeks, for the KPIs releases do
/// not give a series for (#272).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Weekly {
    /// Mean daily active users per week.
    pub dau: Vec<(String, f64)>,
    /// Distinct users over the 4 weeks ending each week.
    pub mau: Vec<(String, f64)>,
    /// DAU ÷ MAU per week.
    pub stickiness: Vec<(String, f64)>,
    /// Per feature: share of its users reaching its success event.
    pub success: BTreeMap<String, Vec<(String, f64)>>,
    /// Per Web Vital (LCP, INP, CLS): p75.
    pub vitals: BTreeMap<String, Vec<(String, f64)>>,
}

fn monday(d: NaiveDate) -> NaiveDate {
    d - chrono::Duration::days(i64::from(d.weekday().num_days_from_monday()))
}

/// Fold (user, day) pairs into weekly DAU, MAU and stickiness (pure).
#[must_use]
pub fn fold_users(pairs: &[(String, String)], today: NaiveDate) -> Weekly {
    let this = monday(today);
    let weeks: Vec<NaiveDate> = (0..12)
        .rev()
        .map(|i| this - chrono::Duration::weeks(i))
        .collect();
    let mut per_day: BTreeMap<NaiveDate, BTreeSet<&str>> = BTreeMap::new();
    let mut per_week: BTreeMap<NaiveDate, BTreeSet<&str>> = BTreeMap::new();
    for (u, d) in pairs {
        if let Ok(day) = NaiveDate::parse_from_str(d.get(..10).unwrap_or(d), "%Y-%m-%d") {
            per_day.entry(day).or_default().insert(u.as_str());
            per_week.entry(monday(day)).or_default().insert(u.as_str());
        }
    }
    let mut w = Weekly::default();
    for wk in weeks {
        let days: Vec<usize> = (0..7)
            .filter_map(|i| {
                let d = wk + chrono::Duration::days(i);
                (d <= today).then(|| per_day.get(&d).map_or(0, BTreeSet::len))
            })
            .collect();
        let mut month: BTreeSet<&str> = BTreeSet::new();
        for back in 0..4 {
            if let Some(us) = per_week.get(&(wk - chrono::Duration::weeks(back))) {
                month.extend(us.iter().copied());
            }
        }
        if month.is_empty() {
            continue;
        }
        let dau = days.iter().sum::<usize>() as f64 / days.len().max(1) as f64;
        let key = wk.format("%Y-%m-%d").to_string();
        w.dau.push((key.clone(), dau));
        w.mau.push((key.clone(), month.len() as f64));
        w.stickiness.push((key, dau / month.len() as f64));
    }
    w
}

/// Measure [`Weekly`] (three queries over the last 16 weeks).
pub async fn weekly(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
) -> Result<Weekly> {
    let q = &q.with_params(&f.params()?);
    let src = source(map);
    let w = f.clause(112)?;
    let day = f.day()?;
    let ident = crate::identified(crate::any_person(q, project).await?);
    let users = format!(
        "SELECT distinct_id, {day} AS d FROM {src} WHERE {w} AND {ident} GROUP BY 1, 2 LIMIT 1000000"
    );
    let successes: Vec<String> = map
        .map(|m| {
            m.features
                .iter()
                .filter_map(|x| x.success.clone())
                .collect()
        })
        .unwrap_or_default();
    let succ_list = successes
        .iter()
        .map(|s| crate::lit(s))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let success = (!successes.is_empty()).then(|| {
        format!(
            "SELECT {day} AS d, feature, distinct_id, max(CASE WHEN event IN ({succ_list}) THEN 1 ELSE 0 END) AS s              FROM {src} WHERE {w} AND feature IS NOT NULL GROUP BY 1, 2, 3 LIMIT 1000000"
        )
    });
    let vitals = format!(
        "SELECT d, name, approx_percentile_cont(v, 0.75) AS p75 FROM (            SELECT {day} AS d, regexp_match(properties, '\"\\$name\":\"([A-Z]+)\"')[1] AS name,                   CAST(regexp_match(properties, '\"\\$value\":([0-9.eE+-]+)')[1] AS DOUBLE) AS v            FROM {src} WHERE {w} AND event = '$vital') x          WHERE name IN ('LCP', 'INP', 'CLS') AND v IS NOT NULL GROUP BY d, name"
    );
    let (u, v) = futures::join!(q.sql(project, &users), q.sql(project, &vitals));
    let pairs: Vec<(String, String)> = u?
        .rows
        .iter()
        .filter_map(|r| Some((text(r, "distinct_id")?, text(r, "d")?)))
        .collect();
    let today = chrono::Utc::now().date_naive();
    let mut out = fold_users(&pairs, today);
    // Vitals: daily p75s, averaged per week (an approximation of the weekly p75).
    let mut vit: BTreeMap<(String, NaiveDate), (f64, usize)> = BTreeMap::new();
    for r in &v?.rows {
        if let (Some(n), Some(d)) = (text(r, "name"), text(r, "d"))
            && let Ok(day) = NaiveDate::parse_from_str(d.get(..10).unwrap_or(&d), "%Y-%m-%d")
        {
            let e = vit.entry((n, monday(day))).or_default();
            e.0 += float(r, "p75");
            e.1 += 1;
        }
    }
    for ((n, wk), (sum, k)) in vit {
        out.vitals
            .entry(n)
            .or_default()
            .push((wk.format("%Y-%m-%d").to_string(), sum / k as f64));
    }
    if let Some(sql) = success {
        let mut acc: BTreeMap<(String, NaiveDate), (BTreeSet<String>, BTreeSet<String>)> =
            BTreeMap::new();
        for r in &q.sql(project, &sql).await?.rows {
            if let (Some(d), Some(feat), Some(id)) =
                (text(r, "d"), text(r, "feature"), text(r, "distinct_id"))
                && let Ok(day) = NaiveDate::parse_from_str(d.get(..10).unwrap_or(&d), "%Y-%m-%d")
            {
                let e = acc.entry((feat, monday(day))).or_default();
                if int(r, "s") > 0 {
                    e.1.insert(id.clone());
                }
                e.0.insert(id);
            }
        }
        for ((feat, wk), (all, ok)) in acc {
            out.success.entry(feat).or_default().push((
                wk.format("%Y-%m-%d").to_string(),
                ok.len() as f64 / all.len().max(1) as f64,
            ));
        }
    }
    Ok(out)
}

/// [`kpi_values`], with weekly series where releases give none.
#[must_use]
pub fn kpi_values_with(b: &Board, w: &Weekly, kpi: &str) -> (Option<f64>, Vec<(String, f64)>) {
    let (current, series) = kpi_values(b, kpi);
    if !series.is_empty() {
        return (current, series);
    }
    let weekly = match kpi {
        "dau" => w.dau.clone(),
        "mau" => w.mau.clone(),
        "stickiness" => w.stickiness.clone(),
        "lcp_p75" => w.vitals.get("LCP").cloned().unwrap_or_default(),
        "inp_p75" => w.vitals.get("INP").cloned().unwrap_or_default(),
        "cls_p75" => w.vitals.get("CLS").cloned().unwrap_or_default(),
        k => k
            .strip_prefix("success:")
            .and_then(|f| w.success.get(f).cloned())
            .unwrap_or_default(),
    };
    (current.or_else(|| weekly.last().map(|x| x.1)), weekly)
}

/// A goal, as stored (`project:<id>:goals`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Goal {
    /// Id (stable).
    pub id: String,
    /// KPI key: one of [`KPIS`], or `success:<feature>`.
    pub kpi: String,
    /// Target value (a share as 0–1).
    pub target: f64,
    /// Higher is better.
    pub up: bool,
    /// Deadline (YYYY-MM-DD).
    pub deadline: String,
    /// Who owns it (free text).
    #[serde(default)]
    pub owner: String,
    /// Value when it was set.
    pub baseline: f64,
    /// When it was set (YYYY-MM-DD).
    pub created: String,
    /// KPIs that must not get worse meanwhile (#272).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guardrails: Vec<Guardrail>,
    /// Set when the deadline was given in releases (#272): how many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub releases: Option<u32>,
}

/// Where a goal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    /// Reached already.
    Reached,
    /// The trend reaches the target by the deadline.
    OnTrack,
    /// The trend misses by less than 10% of the target.
    AtRisk,
    /// The trend misses by more.
    OffTrack,
    /// Fewer than three points, or past the deadline without reaching it.
    NotEnoughData,
}

impl Status {
    /// Plain words.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Reached => "reached",
            Self::OnTrack => "on track",
            Self::AtRisk => "at risk",
            Self::OffTrack => "off track",
            Self::NotEnoughData => "not enough data",
        }
    }
}

/// Label, direction and share-ness of a KPI key.
#[must_use]
pub fn kpi_info(kpi: &str) -> (String, bool, bool) {
    if let Some(f) = kpi.strip_prefix("success:") {
        return (format!("Success rate of {f}"), true, true);
    }
    KPIS.iter().find(|k| k.0 == kpi).map_or_else(
        || (kpi.to_owned(), true, false),
        |k| (k.1.to_owned(), k.2, k.3),
    )
}

/// The KPI's value now, and per release (oldest first) as (first seen
/// day, value), from the board.
#[must_use]
pub fn kpi_values(b: &Board, kpi: &str) -> (Option<f64>, Vec<(String, f64)>) {
    let mut rel: Vec<&crate::ReleaseRow> = b.releases.iter().collect();
    rel.reverse();
    let series = |f: &dyn Fn(&crate::ReleaseRow) -> f64| -> Vec<(String, f64)> {
        rel.iter()
            .filter(|r| r.sessions > 0)
            .map(|r| {
                (
                    r.first_seen.get(..10).unwrap_or(&r.first_seen).to_owned(),
                    f(r),
                )
            })
            .collect()
    };
    match kpi {
        "error_rate" => (Some(b.sessions.error_rate), series(&|r| r.error_rate)),
        "bounce_rate" => (Some(b.sessions.bounce_rate), series(&|r| r.bounce_rate)),
        "avg_secs" => (Some(b.sessions.avg_secs), series(&|r| r.avg_secs)),
        "tasks_per_session" => (
            b.releases.first().map(|r| r.tasks_per_session),
            series(&|r| r.tasks_per_session),
        ),
        "stickiness" => (Some(b.users.stickiness), Vec::new()),
        "dau" => (Some(b.users.dau as f64), Vec::new()),
        "mau" => (Some(b.users.mau as f64), Vec::new()),
        "lcp_p75" | "inp_p75" | "cls_p75" => {
            let name = kpi.get(..3).unwrap_or("").to_uppercase();
            (
                b.vitals.iter().find(|v| v.name == name).map(|v| v.p75),
                Vec::new(),
            )
        }
        k => match k.strip_prefix("success:") {
            Some(f) => (
                b.features
                    .iter()
                    .find(|x| x.key == f && x.users > 0)
                    .map(|x| x.completed_users as f64 / x.users as f64),
                Vec::new(),
            ),
            None => (None, Vec::new()),
        },
    }
}

/// A reachable target: the best release so far or the baseline improved
/// by 10%, whichever is closer to the baseline (the median of the last
/// three releases, else the current value).
#[must_use]
pub fn suggest(current: f64, series: &[(String, f64)], up: bool, share: bool) -> (f64, f64) {
    let mut last: Vec<f64> = series.iter().rev().take(3).map(|x| x.1).collect();
    last.sort_by(f64::total_cmp);
    let baseline = if last.is_empty() {
        current
    } else {
        last[last.len() / 2]
    };
    let step = if up { baseline * 1.1 } else { baseline * 0.9 };
    let best = series
        .iter()
        .map(|x| x.1)
        .reduce(|a, b| if (b > a) == up { b } else { a });
    let mut target = match best {
        Some(b)
            if (b - baseline).abs() > f64::EPSILON
                && (b > baseline) == up
                && (b - baseline).abs() < (step - baseline).abs() =>
        {
            b
        }
        _ => step,
    };
    if share {
        target = target.clamp(0.0, 1.0);
    }
    if (target - baseline).abs() < 1e-9 {
        target = if up {
            baseline + if share { 0.01 } else { 1.0 }
        } else {
            (baseline - if share { 0.01 } else { 1.0 }).max(0.0)
        };
    }
    (baseline, target)
}

/// Where a goal stands today: a straight line through the series (day
/// numbers, values) projected to the deadline.
#[must_use]
pub fn status(
    goal: &Goal,
    current: Option<f64>,
    series: &[(String, f64)],
    today: NaiveDate,
) -> (Status, Option<f64>) {
    let reached = |v: f64| {
        if goal.up {
            v >= goal.target
        } else {
            v <= goal.target
        }
    };
    if current.is_some_and(reached) {
        return (Status::Reached, current);
    }
    let Ok(deadline) = NaiveDate::parse_from_str(&goal.deadline, "%Y-%m-%d") else {
        return (Status::NotEnoughData, None);
    };
    if deadline < today {
        return (Status::NotEnoughData, None);
    }
    let pts: Vec<(f64, f64)> = series
        .iter()
        .filter_map(|(d, v)| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .ok()
                .map(|d| ((d - today).num_days() as f64, *v))
        })
        .collect();
    if pts.len() < 3 {
        return (Status::NotEnoughData, None);
    }
    let n = pts.len() as f64;
    let (mx, my) = (
        pts.iter().map(|p| p.0).sum::<f64>() / n,
        pts.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let slope = if sxx > 0.0 {
        pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>() / sxx
    } else {
        0.0
    };
    let x = (deadline - today).num_days() as f64;
    let projected = my + slope * (x - mx);
    let st = if reached(projected) {
        Status::OnTrack
    } else if (projected - goal.target).abs() < 0.1 * goal.target.abs().max(f64::EPSILON) {
        Status::AtRisk
    } else {
        Status::OffTrack
    };
    (st, Some(projected))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(d, x)| ((*d).to_owned(), *x)).collect()
    }

    #[test]
    fn suggests_a_reachable_step() {
        // Errors (down is better): baseline = median of the last 3 = 0.10;
        // the step to 0.09 is closer than the best release seen (0.08).
        let (base, t) = suggest(
            0.12,
            &s(&[
                ("2026-01-01", 0.08),
                ("2026-02-01", 0.10),
                ("2026-03-01", 0.11),
                ("2026-04-01", 0.10),
            ]),
            false,
            true,
        );
        assert!((base - 0.10).abs() < 1e-9);
        assert!((t - 0.09).abs() < 1e-9);
        // Up: best seen 52 is closer than +10% (55) from 50.
        let (_, t) = suggest(
            50.0,
            &s(&[
                ("2026-01-01", 52.0),
                ("2026-02-01", 50.0),
                ("2026-03-01", 49.0),
                ("2026-04-01", 50.0),
            ]),
            true,
            false,
        );
        assert!((t - 52.0).abs() < 1e-9);
        // No history: +10% of the current value.
        assert!((suggest(0.5, &[], true, true).1 - 0.55).abs() < 1e-9);
    }

    #[test]
    fn projects_the_trend_to_the_deadline() {
        let today = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap_or_default();
        let g = Goal {
            id: "g".into(),
            kpi: "error_rate".into(),
            target: 0.05,
            up: false,
            deadline: "2026-06-01".into(),
            owner: String::new(),
            baseline: 0.12,
            created: "2026-03-01".into(),
            guardrails: Vec::new(),
            releases: None,
        };
        // Falling 0.02 a month: reaches 0.04 by June → on track.
        let falling = s(&[
            ("2026-01-01", 0.12),
            ("2026-02-01", 0.10),
            ("2026-03-01", 0.08),
        ]);
        assert_eq!(status(&g, Some(0.08), &falling, today).0, Status::OnTrack);
        // Flat at 0.08 → off track (60% above the target).
        let flat = s(&[
            ("2026-01-01", 0.08),
            ("2026-02-01", 0.08),
            ("2026-03-01", 0.08),
        ]);
        assert_eq!(status(&g, Some(0.08), &flat, today).0, Status::OffTrack);
        // Close: projected 0.054 → at risk.
        let close = s(&[
            ("2026-01-01", 0.058),
            ("2026-02-01", 0.057),
            ("2026-03-01", 0.056),
        ]);
        assert_eq!(status(&g, Some(0.056), &close, today).0, Status::AtRisk);
        assert_eq!(status(&g, Some(0.04), &flat, today).0, Status::Reached);
        assert_eq!(
            status(&g, Some(0.08), &flat[..2], today).0,
            Status::NotEnoughData
        );
    }

    #[test]
    fn guardrails_breach_when_worse_than_the_limit() {
        let g = Guardrail {
            kpi: "bounce_rate".into(),
            limit: Guardrail::suggested(0.10, false, true),
            up: false,
        };
        assert!(
            (g.limit - 0.11).abs() < 1e-9,
            "a share moves by a point at least"
        );
        assert!(g.breached(0.12) && !g.breached(0.10));
        let up = Guardrail {
            kpi: "dau".into(),
            limit: Guardrail::suggested(200.0, true, false),
            up: true,
        };
        assert!((up.limit - 190.0).abs() < 1e-9);
        assert!(up.breached(180.0) && !up.breached(195.0));
    }

    #[test]
    fn objectives_take_their_worst_key_result() {
        assert_eq!(
            objective_status(&[Status::Reached, Status::AtRisk, Status::OnTrack]),
            Status::AtRisk
        );
        assert_eq!(objective_status(&[]), Status::NotEnoughData);
    }

    #[test]
    fn deadlines_in_releases_use_the_median_gap() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap_or_default();
        let firsts: Vec<String> = [
            "2026-10-01T10:00:00Z",
            "2026-10-15",
            "2026-10-29",
            "2026-11-26",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        // Gaps 14, 14, 28: median 14 → 3 releases = 42 days.
        assert_eq!(
            deadline_in_releases(&firsts, 3, today),
            NaiveDate::from_ymd_opt(2027, 1, 4)
        );
        assert_eq!(deadline_in_releases(&firsts[..1], 3, today), None);
    }

    #[test]
    fn weekly_users_fold_into_dau_mau_and_stickiness() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap_or_default(); // a Wednesday
        let p = |u: &str, d: &str| (u.to_owned(), d.to_owned());
        let pairs = vec![
            p("a", "2026-11-23"),
            p("b", "2026-11-23"),
            p("a", "2026-11-24"),
            p("c", "2026-11-10"),
        ];
        let w = fold_users(&pairs, today);
        let empty = (String::new(), 0.0);
        let last = w.dau.last().unwrap_or(&empty);
        assert_eq!(last.0, "2026-11-23");
        // Mon 2 users, Tue 1, Wed 0 → 1.0 per day over 3 days.
        assert!((last.1 - 1.0).abs() < 1e-9);
        assert_eq!(
            w.mau.last().map(|x| x.1),
            Some(3.0),
            "c was active 2 weeks ago"
        );
        assert!((w.stickiness.last().map_or(0.0, |x| x.1) - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(
            w.dau.first().map(|x| x.0.as_str()),
            Some("2026-11-09"),
            "empty weeks skipped"
        );
    }
}

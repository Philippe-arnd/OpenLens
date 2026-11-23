//! Goals on KPIs (#235): a target, a direction and a deadline on a board
//! KPI; OpenLens suggests the target, shows the gap and progress, and
//! projects the trend to the deadline (on track, at risk, off track).

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::Board;

/// KPIs a goal can target: (key, label, higher is better, is a share).
pub const KPIS: &[(&str, &str, bool, bool)] = &[
    ("error_rate", "Sessions with errors", false, true),
    ("bounce_rate", "Bounce rate", false, true),
    ("avg_secs", "Average session (s)", true, false),
    ("tasks_per_session", "Tasks per session", true, false),
    ("stickiness", "Stickiness (DAU ÷ MAU)", true, true),
    ("dau", "Daily active users", true, false),
    ("mau", "Monthly active users", true, false),
];

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
}

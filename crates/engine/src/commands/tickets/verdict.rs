//! Ticket verdicts (#704, D-108): the flag's cohorts when there is one,
//! else a control chart of the weekly series, else a before/after test.
//! Moved out of `tickets.rs` as is (file length, #307).

use super::*;
use crate::FlagCohort;
use chrono::Datelike;

/// A flag's sessions against the others on a ticket's metric (#704).
#[derive(Debug, Clone, PartialEq)]
pub struct Cohort {
    /// Flag key.
    pub flag: String,
    /// The metric over the sessions with the flag on.
    pub with: f64,
    /// Those sessions.
    pub with_n: i64,
    /// The metric over the other sessions.
    pub without: f64,
    /// Those sessions.
    pub without_n: i64,
    /// p-value of the difference (`None`: under 30 sessions on a side).
    pub p: Option<f64>,
}

/// The cohorts of a flag on `metric`, for the metrics they measure with a
/// test: sessions with errors and tasks per session (pure).
#[must_use]
pub fn cohort_for(c: &FlagCohort, metric: &str) -> Option<Cohort> {
    let (w, o) = (&c.with, &c.without);
    let (with, without, p) = match metric {
        "error_rate" => (w.error_rate, o.error_rate, c.p_errors),
        "tasks_per_session" => (w.tasks_per_session, o.tasks_per_session, c.p_tasks),
        _ => return None,
    };
    Some(Cohort {
        flag: c.flag.clone(),
        with,
        with_n: w.sessions,
        without,
        without_n: o.sessions,
        p,
    })
}

/// What a ticket's metric is judged on, once its check date has come.
#[derive(Debug, Clone)]
pub struct Measures<'a> {
    /// The metric now.
    pub after: f64,
    /// Sessions behind it.
    pub after_n: i64,
    /// The metric is a share (0–1).
    pub share: bool,
    /// The metric's weekly series (Monday, value), oldest first; empty
    /// when it has none.
    pub weekly: &'a [(String, f64)],
    /// The day the release that shipped the change was first seen.
    pub shipped: Option<NaiveDate>,
    /// Today.
    pub today: NaiveDate,
    /// The ticket's flag on the metric, when it has one and it measures it.
    pub cohort: Option<Cohort>,
}

/// A verdict on a change (D-108).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// [`IMPROVED`], [`WORSE`] or [`INCONCLUSIVE`].
    pub word: String,
    /// `flag`, `control_chart`, `before_after` or `none`.
    pub method: String,
    /// The method, the band or the p-value, and the sample sizes.
    pub evidence: String,
}

impl Verdict {
    fn new(word: &str, method: &str, evidence: String) -> Self {
        Self {
            word: word.to_owned(),
            method: method.to_owned(),
            evidence,
        }
    }
}

/// The word for a change `delta` that is clear (or not) (pure).
pub(super) fn word(clear: bool, delta: f64, up: bool) -> &'static str {
    if !clear || delta.abs() < 1e-12 {
        INCONCLUSIVE
    } else if (delta > 0.0) == up {
        IMPROVED
    } else {
        WORSE
    }
}

pub(super) fn day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok()
}

/// The values of the full weeks before shipping (the last
/// [`BEFORE_WEEKS`]) and of the full weeks after it, ended by `today`
/// (pure).
pub(super) fn split_weeks(
    weekly: &[(String, f64)],
    shipped: NaiveDate,
    today: NaiveDate,
) -> (Vec<f64>, Vec<f64>) {
    let mut weeks: Vec<(NaiveDate, f64)> = weekly
        .iter()
        .filter(|(_, v)| v.is_finite())
        .filter_map(|(d, v)| Some((day(d)?, *v)))
        .collect();
    weeks.sort_by_key(|w| w.0);
    let week = chrono::Duration::days(7);
    let before: Vec<f64> = weeks
        .iter()
        .filter(|(m, _)| *m + week <= shipped)
        .map(|w| w.1)
        .collect();
    let skip = before.len().saturating_sub(BEFORE_WEEKS);
    let before = before.into_iter().skip(skip).collect();
    let after = weeks
        .iter()
        .filter(|(m, _)| *m >= shipped && *m + week <= today)
        .map(|w| w.1)
        .collect();
    (before, after)
}

/// Full weeks (Monday to Sunday) between `shipped` and `today` (pure).
pub(super) fn full_weeks_after(shipped: NaiveDate, today: NaiveDate) -> usize {
    let to_monday = (7 - i64::from(shipped.weekday().num_days_from_monday())) % 7;
    let first = shipped + chrono::Duration::days(to_monday);
    usize::try_from((today - first).num_days().div_euclid(7)).unwrap_or(0)
}

/// A control chart can be drawn but its second full week after shipping
/// has not ended yet: the check waits for it (pure).
#[must_use]
pub fn waiting(weekly: &[(String, f64)], shipped: NaiveDate, today: NaiveDate) -> bool {
    split_weeks(weekly, shipped, today).0.len() >= MIN_BEFORE
        && full_weeks_after(shipped, today) < MIN_AFTER
}

pub(super) fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

/// Sample standard deviation.
pub(super) fn sd(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

pub(super) fn flag_verdict(t: &Ticket, c: &Cohort, share: bool) -> Verdict {
    let delta = c.with - c.without;
    let clear = c.p.is_some_and(|p| p < crate::stats::ALPHA);
    let ev = format!(
        "Flag cohorts: {} with `{}` ({} sessions) vs {} without it ({} sessions) since the release · {}",
        fmt(c.with, share),
        c.flag,
        c.with_n,
        fmt(c.without, share),
        c.without_n,
        c.p.map_or_else(
            || format!(
                "under {} sessions on a side, no test",
                crate::stats::MIN_SESSIONS
            ),
            |p| format!("p = {p:.3}")
        )
    );
    Verdict::new(word(clear, delta, t.up), "flag", ev)
}

pub(super) fn chart_verdict(
    t: &Ticket,
    before: &[f64],
    after: &[f64],
    share: bool,
    note: &str,
) -> Verdict {
    let (m, s) = (mean(before), sd(before));
    let flat = s <= 1e-12;
    let band = if flat {
        m.abs() * FLAT_BAND
    } else {
        SIGMAS * s
    };
    let a = mean(after);
    let delta = a - m;
    let ev = format!(
        "{note}Control chart: {} a week over the {} full weeks after the release vs {} over the {} weeks before; normal range {} to {} ({})",
        fmt(a, share),
        after.len(),
        fmt(m, share),
        before.len(),
        fmt(m - band, share),
        fmt(m + band, share),
        if flat {
            "± 5%: the weeks before were flat".to_owned()
        } else {
            format!("mean ± 3σ, σ = {}", fmt(s, share))
        }
    );
    Verdict::new(word(delta.abs() > band, delta, t.up), "control_chart", ev)
}

/// The verdict on a change (pure, D-108): the flag's cohorts when it
/// shipped behind one, else a control chart of the metric's weekly
/// series, else (not enough history) a before/after significance test
/// for shares; anything else is inconclusive.
#[must_use]
pub fn verdict(t: &Ticket, m: &Measures<'_>) -> Verdict {
    let mut note = String::new();
    match (&m.cohort, &t.flag) {
        (Some(c), _) if c.with_n > 0 && c.without_n > 0 => {
            return flag_verdict(t, c, m.share);
        }
        (Some(c), _) => {
            note = format!("Flag `{}` not seen on sessions since the release. ", c.flag);
        }
        (None, Some(f)) => {
            note = format!("Flag `{f}` has no cohort reading of this metric. ");
        }
        (None, None) => {}
    }
    let before = match m.shipped {
        Some(d) => {
            let (before, after) = split_weeks(m.weekly, d, m.today);
            if before.len() >= MIN_BEFORE && after.len() >= MIN_AFTER {
                return chart_verdict(t, &before, &after, m.share, &note);
            }
            before.len()
        }
        None => 0,
    };
    let history = if m.weekly.is_empty() {
        "no weekly series for this metric".to_owned()
    } else {
        format!("{before} full weeks before the release ({MIN_BEFORE} needed)")
    };
    let Some(b) = t.baseline else {
        return Verdict::new(
            INCONCLUSIVE,
            "none",
            format!("{note}No test: no baseline, and {history}"),
        );
    };
    let delta = m.after - b;
    let rel = if b.abs() > 1e-12 {
        delta / b.abs()
    } else {
        0.0
    };
    let moved = format!(
        "{} before ({} sessions) → {} after ({} sessions) ({:+.1}%)",
        fmt(b, m.share),
        t.baseline_n,
        fmt(m.after, m.share),
        m.after_n,
        rel * 100.0
    );
    if !m.share {
        return Verdict::new(
            INCONCLUSIVE,
            "none",
            format!("{note}No test: {moved}; {history}, and only shares get a before/after test"),
        );
    }
    let p = crate::stats::two_proportions(m.after, m.after_n, b, t.baseline_n);
    let ev = format!(
        "{note}Before/after test ({history}): {moved} · {}",
        p.map_or_else(
            || format!(
                "under {} sessions on a side, no test",
                crate::stats::MIN_SESSIONS
            ),
            |p| format!("p = {p:.3}")
        )
    );
    let clear = p.is_some_and(|p| p < crate::stats::ALPHA);
    Verdict::new(word(clear, delta, t.up), "before_after", ev)
}

/// How a method reads in the GitHub comment.
pub(super) fn method_label(method: &str) -> &'static str {
    match method {
        "flag" => "flag cohorts",
        "control_chart" => "control chart of the weekly series",
        "before_after" => "before/after test",
        _ => "no test",
    }
}

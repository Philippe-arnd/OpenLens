//! Availability over long windows (SLA) and an availability target with
//! its error budget (SLO) (#560, D-078).
//!
//! **Availability** is time up over time measured. A check stands for
//! [`PROBE_SECS`]; time with no check (OpenLens stopped, no probe yet) is
//! neither up nor down and is given as `unmeasured_seconds`. Down time is
//! the time inside a *down* incident that was measured. Planned
//! maintenance (#563) is left out of both down and measured time, and
//! `maintenance_seconds` says how much.
//!
//! The last [`RAW_DAYS`] UTC days are counted from the raw checks (to the
//! second); older days from the daily rows of #559, a day cut by a
//! window's start counted pro rata. p95 over a window that reaches older
//! days is the checks-weighted 95th percentile of the days' p95.
//!
//! **The SLO** is one target per environment over a rolling
//! [`SLO_DAYS`] days, stored as project data and changed only through
//! `uptime.set_target`. The error budget is `(1 − target) × 30 days`; the
//! burn rate over a period is its share of measured time down divided by
//! `1 − target` (1 spends the budget in exactly 30 days). Two alert rules
//! read it: *budget spent* and *budget burning fast* (at the last day's
//! rate, what is left is gone within [`FAST_BURN_SECS`]).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Datelike, Months, NaiveDate, NaiveTime, TimeZone, Utc};
use openlens_meta::uptime_history::{PROBE_SECS, RAW_DAYS};
use openlens_meta::{Meta, Project, UptimeDay};
use serde::Serialize;

use crate::commands::alerts::{Seen, Settings, track_at};
use crate::commands::{Error, Host};
use crate::environments::incident_rule;
use crate::maintenance::{PRODUCTION, covers};

/// The SLO's rolling window, in days.
pub const SLO_DAYS: i64 = 30;
/// The lowest target `uptime.set_target` takes.
pub const MIN_TARGET: f64 = 0.5;
/// The highest target `uptime.set_target` takes.
pub const MAX_TARGET: f64 = 0.99999;
/// *Budget burning fast*: what is left goes within this many seconds at
/// the last day's rate.
pub const FAST_BURN_SECS: i64 = 2 * DAY;
/// Most environments with a target in one project.
pub const MAX_TARGETS: usize = 20;
/// The project setting that keeps the targets.
const SETTING: &str = "uptime_targets";
const DAY: i64 = 86_400;

/// Availability over one window.
#[derive(Debug, Clone, Serialize)]
pub struct WindowFigure {
    /// `24h`, `7d`, `30d`, `90d`, `month` (this calendar month so far, in
    /// the project's zone), `last_month` or `365d`.
    pub label: &'static str,
    /// Time up over time measured (0 to 1); `None` when nothing was
    /// measured.
    pub availability: Option<f64>,
    /// Seconds measured, planned maintenance left out.
    pub measured_seconds: i64,
    /// Seconds of the window with no check and no maintenance: neither up
    /// nor down.
    pub unmeasured_seconds: i64,
    /// Seconds of planned maintenance left out.
    pub maintenance_seconds: i64,
    /// *Down* incidents overlapping the window, those that started during
    /// planned maintenance left out.
    pub incidents: u32,
    /// 95th percentile response time, ms.
    pub p95_ms: Option<i64>,
}

/// The availability target of an environment and its error budget.
#[derive(Debug, Clone, Serialize)]
pub struct Slo {
    /// The target, a share (0.999 for 99.9 %).
    pub target: f64,
    /// Availability over the rolling 30 days.
    pub availability: Option<f64>,
    /// Down time the target allows over 30 days, in seconds.
    pub budget_seconds: i64,
    /// Down time over the rolling 30 days.
    pub spent_seconds: i64,
    /// What is left of the budget (0 once spent).
    pub left_seconds: i64,
    /// Burn rate over the last hour (1 spends the budget in 30 days).
    pub burn_1h: Option<f64>,
    /// Burn rate over the last day.
    pub burn_1d: Option<f64>,
}

impl Slo {
    /// The budget is spent.
    #[must_use]
    pub fn spent(&self) -> bool {
        self.spent_seconds >= self.budget_seconds
    }

    /// Down seconds a day at the last day's burn rate.
    #[must_use]
    pub fn down_per_day(&self) -> f64 {
        self.burn_1d.unwrap_or(0.0) * (1.0 - self.target) * DAY as f64
    }

    /// At the last day's rate, what is left goes within
    /// [`FAST_BURN_SECS`] (already gone counts while it still burns).
    #[must_use]
    pub fn burning_fast(&self) -> bool {
        let per_day = self.down_per_day();
        per_day > 0.0 && per_day * (FAST_BURN_SECS as f64 / DAY as f64) >= self.left_seconds as f64
    }
}

/// An environment's figures.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Environment.
    pub environment: String,
    /// The windows, in the order of [`WindowFigure::label`].
    pub windows: Vec<WindowFigure>,
    /// The SLO, when the project set a target for this environment.
    pub slo: Option<Slo>,
}

// ----- Targets -----

/// The project's targets by environment.
#[must_use]
pub fn targets(meta: &Meta, project: &str) -> BTreeMap<String, f64> {
    meta.project_setting(project, SETTING)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<BTreeMap<String, f64>>(&j).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, t)| valid_target(*t))
        .collect()
}

/// The target of `env`, if set.
#[must_use]
pub fn target(meta: &Meta, project: &str, env: &str) -> Option<f64> {
    targets(meta, project).get(env).copied()
}

fn valid_target(t: f64) -> bool {
    t.is_finite() && (MIN_TARGET..=MAX_TARGET).contains(&t)
}

/// Set or clear (`None`) the target of `env`.
///
/// # Errors
/// A target out of [`MIN_TARGET`]..=[`MAX_TARGET`], too many targets, or
/// the store.
pub fn set_target(meta: &Meta, project: &str, env: &str, t: Option<f64>) -> Result<(), Error> {
    let mut all = targets(meta, project);
    match t {
        Some(t) if !valid_target(t) => {
            return Err(Error::Invalid(format!(
                "target: a share from {MIN_TARGET} to {MAX_TARGET} (0.999 for 99.9%)"
            )));
        }
        Some(t) => {
            if !all.contains_key(env) && all.len() >= MAX_TARGETS {
                return Err(Error::Invalid(format!(
                    "at most {MAX_TARGETS} environments with a target"
                )));
            }
            all.insert(env.to_owned(), t);
        }
        None => {
            all.remove(env);
        }
    }
    let j = if all.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&all).map_err(|e| Error::Storage(e.to_string()))?
    };
    meta.set_project_setting(project, SETTING, &j)
        .map_err(|e| Error::Storage(e.to_string()))
}

// ----- Spans of time -----

type Span = (i64, i64);

/// Sorted, non-overlapping, non-empty spans.
fn merge(mut v: Vec<Span>) -> Vec<Span> {
    v.retain(|(a, b)| a < b);
    v.sort_unstable();
    let mut out: Vec<Span> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn clip(v: &[Span], from: i64, to: i64) -> Vec<Span> {
    v.iter()
        .map(|&(a, b)| (a.max(from), b.min(to)))
        .filter(|(a, b)| a < b)
        .collect()
}

fn total(v: &[Span]) -> i64 {
    v.iter().map(|(a, b)| b - a).sum()
}

/// The overlap of two merged lists.
fn intersect(x: &[Span], y: &[Span]) -> Vec<Span> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while let (Some(&(a1, b1)), Some(&(a2, b2))) = (x.get(i), y.get(j)) {
        let (a, b) = (a1.max(a2), b1.min(b2));
        if a < b {
            out.push((a, b));
        }
        if b1 < b2 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// `x` without `y` (both merged).
fn subtract(x: &[Span], y: &[Span]) -> Vec<Span> {
    let mut out = Vec::new();
    for &(a, b) in x {
        let mut start = a;
        for &(c, d) in y.iter().filter(|(c, d)| *d > a && *c < b) {
            if c > start {
                out.push((start, c));
            }
            start = start.max(d);
        }
        if start < b {
            out.push((start, b));
        }
    }
    out
}

fn day_start(d: NaiveDate) -> i64 {
    d.and_time(NaiveTime::MIN).and_utc().timestamp()
}

// ----- The figures -----

/// What the figures of one environment are computed from.
struct Data {
    /// Raw checks are complete from here (the start of the oldest UTC day
    /// still within [`RAW_DAYS`]).
    raw_from: i64,
    /// Raw checks from `raw_from`: (at, ms).
    checks: Vec<(i64, Option<i64>)>,
    /// Daily rows before `raw_from`.
    rows: HashMap<NaiveDate, UptimeDay>,
    /// *Down* incidents (start, end so far).
    incidents: Vec<Span>,
    /// The same, merged.
    down: Vec<Span>,
    /// Planned maintenance of the environment, merged.
    maintenance: Vec<Span>,
}

fn storage(e: openlens_meta::MetaError) -> Error {
    Error::Storage(e.to_string())
}

impl Data {
    fn load(meta: &Meta, project: &str, env: &str, from: i64, now: i64) -> Result<Self, Error> {
        let today = DateTime::from_timestamp(now, 0)
            .ok_or_else(|| Error::Invalid("time out of range".into()))?
            .date_naive();
        let raw_day = today
            .checked_sub_days(chrono::Days::new(u64::try_from(RAW_DAYS - 1).unwrap_or(0)))
            .unwrap_or(today);
        let raw_from = day_start(raw_day);
        let checks = meta
            .checks_in(project, env, raw_from.max(from))
            .map_err(storage)?
            .into_iter()
            .filter(|c| c.0 < now)
            .map(|c| (c.0, c.3))
            .collect();
        let mut rows = HashMap::new();
        if from < raw_from
            && let (Some(first), Some(last)) = (
                DateTime::from_timestamp(from, 0).map(|d| d.date_naive()),
                raw_day.pred_opt(),
            )
        {
            for d in meta
                .uptime_days(project, env, first, last)
                .map_err(storage)?
            {
                if let Ok(day) = NaiveDate::parse_from_str(&d.day, "%Y-%m-%d") {
                    rows.insert(day, d);
                }
            }
        }
        let incidents: Vec<Span> = meta
            .down_spans(project, env, from, now)
            .map_err(storage)?
            .into_iter()
            .map(|(a, b)| (a, b.unwrap_or(now).min(now)))
            .collect();
        let maintenance = merge(
            meta.maintenance_windows(project, from, now)
                .map_err(storage)?
                .iter()
                .filter(|w| covers(w, env))
                .map(|w| (w.starts_at, w.ends_at))
                .collect(),
        );
        Ok(Self {
            raw_from,
            checks,
            rows,
            down: merge(incidents.clone()),
            incidents,
            maintenance,
        })
    }

    /// Seconds measured and down over `[from, to)`, and the response times
    /// as (value, weight) pairs.
    fn tally(&self, from: i64, to: i64) -> (i64, i64, Vec<(i64, f64)>) {
        let (mut measured, mut down, mut ms) = (0i64, 0i64, Vec::new());
        // The raw checks, to the second.
        let (rf, rt) = (from.max(self.raw_from), to);
        if rf < rt {
            let maint = clip(&self.maintenance, rf, rt);
            let mut covered = Vec::new();
            for &(at, v) in self.checks.iter().filter(|c| c.0 >= rf && c.0 < rt) {
                covered.push((at, (at + PROBE_SECS).min(rt)));
                if let Some(v) = v
                    && !maint.iter().any(|(a, b)| *a <= at && at < *b)
                {
                    ms.push((v, 1.0));
                }
            }
            let counted = subtract(&merge(covered), &maint);
            measured += total(&counted);
            down += total(&intersect(&clip(&self.down, rf, rt), &counted));
        }
        // The daily rows, a day cut by the window pro rata.
        let (sf, st) = (from, to.min(self.raw_from));
        if sf < st
            && let (Some(first), Some(last)) = (
                DateTime::from_timestamp(sf, 0).map(|d| d.date_naive()),
                DateTime::from_timestamp(st - 1, 0).map(|d| d.date_naive()),
            )
        {
            for day in first.iter_days().take_while(|d| *d <= last) {
                let Some(row) = self.rows.get(&day) else {
                    continue;
                };
                let start = day_start(day);
                let (a, b) = (start.max(sf), (start + DAY).min(st));
                let share = (b - a) as f64 / DAY as f64;
                let maint = clip(&self.maintenance, a, b);
                let m = ((row.measured_seconds as f64 * share) as i64 - total(&maint)).max(0);
                let d_in_maint = total(&intersect(&clip(&self.down, a, b), &maint));
                let d = ((row.down_seconds as f64 * share) as i64 - d_in_maint).clamp(0, m);
                measured += m;
                down += d;
                if let Some(p) = row.p95_ms {
                    ms.push((p, row.checks as f64 * share));
                }
            }
        }
        (measured, down.min(measured), ms)
    }

    fn figure(&self, label: &'static str, from: i64, to: i64) -> (WindowFigure, i64) {
        let (measured, down, mut ms) = self.tally(from, to);
        let maintenance = total(&clip(&self.maintenance, from, to));
        let incidents = self
            .incidents
            .iter()
            .filter(|(a, b)| *a < to && *b > from)
            .filter(|(a, _)| !self.maintenance.iter().any(|(c, d)| c <= a && a < d))
            .count();
        let fig = WindowFigure {
            label,
            availability: (measured > 0).then(|| (measured - down) as f64 / measured as f64),
            measured_seconds: measured,
            unmeasured_seconds: (to - from - measured - maintenance).max(0),
            maintenance_seconds: maintenance,
            incidents: u32::try_from(incidents).unwrap_or(u32::MAX),
            p95_ms: weighted_p95(&mut ms),
        };
        (fig, down)
    }

    /// The SLO of `target` at `now`.
    fn slo(&self, target: f64, now: i64) -> Slo {
        let (f30, spent) = self.figure("30d", now - SLO_DAYS * DAY, now);
        let burn = |secs: i64| {
            let (measured, down, _) = self.tally(now - secs, now);
            (measured > 0).then(|| down as f64 / measured as f64 / (1.0 - target))
        };
        let budget = ((1.0 - target) * (SLO_DAYS * DAY) as f64).round() as i64;
        Slo {
            target,
            availability: f30.availability,
            budget_seconds: budget,
            spent_seconds: spent,
            left_seconds: (budget - spent).max(0),
            burn_1h: burn(3600),
            burn_1d: burn(DAY),
        }
    }
}

/// The value at which the weights reach 95 %.
fn weighted_p95(v: &mut [(i64, f64)]) -> Option<i64> {
    v.sort_unstable_by_key(|x| x.0);
    let sum: f64 = v.iter().map(|x| x.1).sum();
    if sum <= 0.0 {
        return None;
    }
    let mut acc = 0.0;
    for &(value, w) in v.iter() {
        acc += w;
        if acc >= 0.95 * sum {
            return Some(value);
        }
    }
    v.last().map(|x| x.0)
}

/// The start of the calendar month of `now` in `tz`, and of the one
/// before (Unix seconds).
fn months(tz: &str, now: DateTime<Utc>) -> (i64, i64) {
    let tz: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz).date_naive();
    let first = NaiveDate::from_ymd_opt(local.year(), local.month(), 1).unwrap_or(local);
    let prev = first.checked_sub_months(Months::new(1)).unwrap_or(first);
    let at = |d: NaiveDate| {
        let midnight = d.and_time(NaiveTime::MIN);
        tz.from_local_datetime(&midnight)
            .earliest()
            .or_else(|| {
                // Midnight skipped by a clock change: the first hour that is.
                tz.from_local_datetime(&(midnight + chrono::Duration::hours(1)))
                    .earliest()
            })
            .map_or_else(|| midnight.and_utc().timestamp(), |t| t.timestamp())
    };
    (at(first), at(prev))
}

/// An environment's figures over every window, and its SLO when a target
/// is set. Months are calendar months in `tz` (the project's zone).
///
/// # Errors
/// An odd environment name, or the store.
pub fn report(
    meta: &Meta,
    project: &str,
    environment: &str,
    tz: &str,
    now: DateTime<Utc>,
) -> Result<Report, Error> {
    crate::commands::uptime::valid_env(environment)?;
    let t = now.timestamp();
    let (month, last_month) = months(tz, now);
    let from = (t - 365 * DAY).min(last_month);
    let data = Data::load(meta, project, environment, from, t)?;
    let windows = [
        ("24h", t - DAY, t),
        ("7d", t - 7 * DAY, t),
        ("30d", t - 30 * DAY, t),
        ("90d", t - 90 * DAY, t),
        ("month", month, t),
        ("last_month", last_month, month),
        ("365d", t - 365 * DAY, t),
    ]
    .into_iter()
    .map(|(label, a, b)| data.figure(label, a, b).0)
    .collect();
    Ok(Report {
        environment: environment.to_owned(),
        windows,
        slo: target(meta, project, environment).map(|target| data.slo(target, t)),
    })
}

/// The SLO of `env` for `target` at `now` (what the alerts read).
///
/// # Errors
/// The store.
pub fn slo(
    meta: &Meta,
    project: &str,
    env: &str,
    target: f64,
    now: DateTime<Utc>,
) -> Result<Slo, Error> {
    let t = now.timestamp();
    let data = Data::load(meta, project, env, t - SLO_DAYS * DAY, t)?;
    Ok(data.slo(target, t))
}

// ----- Alerts -----

/// A length in words, rounded to the minute.
fn minutes(secs: i64) -> String {
    crate::maintenance::length(secs)
}

/// The SLO rules of every environment with a target, at `now`: *budget
/// spent* and *budget burning fast*, each an incident announced once
/// through the alert channels (quiet hours apply, no hourly reminder),
/// with one message when it ends. Each can be switched off in the alert
/// settings. The server runs it every five minutes.
pub async fn alert_pass<H: Host>(host: &H, p: &Project, s: &Settings, now: DateTime<Utc>) {
    if !s.budget_spent && !s.budget_burn {
        return;
    }
    for (env, target) in targets(host.meta(), &p.id) {
        let Ok(slo) = slo(host.meta(), &p.id, &env, target, now) else {
            continue;
        };
        let rule = |check: &str| {
            if env == PRODUCTION {
                check.to_owned()
            } else {
                incident_rule(&env, check)
            }
        };
        let goal = format!("{:.3}%", target * 100.0);
        let avail = slo.availability.map_or_else(
            || "not measured".to_owned(),
            |a| format!("{:.3}%", a * 100.0),
        );
        if s.budget_spent {
            let cause = format!(
                "{env}: {} down over 30 days, the {goal} target allows {}; availability {avail}",
                minutes(slo.spent_seconds),
                minutes(slo.budget_seconds)
            );
            let r = rule("budget_spent");
            let seen = Seen {
                rule: &r,
                failing: slo.spent(),
                cause: &cause,
                wait: 0,
            };
            Box::pin(track_at(host, &p.id, &p.name, &seen, now.timestamp())).await;
        }
        if s.budget_burn {
            let cause = format!(
                "{env}: about {} down a day at the last day's rate, {} of the {goal} error budget left",
                minutes(slo.down_per_day().round() as i64),
                minutes(slo.left_seconds)
            );
            let r = rule("budget_burn");
            let seen = Seen {
                rule: &r,
                failing: slo.burning_fast(),
                cause: &cause,
                wait: 0,
            };
            Box::pin(track_at(host, &p.id, &p.name, &seen, now.timestamp())).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_add_up_without_counting_twice() {
        let x = merge(vec![(0, 10), (5, 20), (30, 40)]);
        assert_eq!(x, vec![(0, 20), (30, 40)]);
        let y = merge(vec![(15, 35)]);
        assert_eq!(intersect(&x, &y), vec![(15, 20), (30, 35)]);
        assert_eq!(subtract(&x, &y), vec![(0, 15), (35, 40)]);
        assert_eq!(
            total(&subtract(&x, &y)) + total(&intersect(&x, &y)),
            total(&x)
        );
    }

    #[test]
    fn burning_fast_means_the_budget_left_goes_within_two_days() {
        let s = |burn: f64, left: i64| Slo {
            target: 0.999,
            availability: None,
            budget_seconds: 2592,
            spent_seconds: 2592 - left,
            left_seconds: left,
            burn_1h: None,
            burn_1d: Some(burn),
        };
        // Burn 10: 864 s a day.
        assert!(s(10.0, 1000).burning_fast());
        assert!(!s(10.0, 2000).burning_fast());
        assert!(!s(0.0, 0).burning_fast(), "spent, not burning");
    }
}

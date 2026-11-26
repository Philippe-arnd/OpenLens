//! Uptime history as commands (#559, D-075): the days of a range, one row
//! per UTC day (`uptime.days`), and every incident, page by page
//! (`uptime.incidents`). The functions the server's minute job calls are
//! here too: recording a monitored environment's probe, and writing the
//! days as they close ([`close_days`]) and after a start ([`rebuild_recent`]).
//!
//! An SLO or SLA (#560) reads [`days`]: per day, the seconds up and
//! measured, the checks and failed checks, and p50/p95/max response time.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use openlens_meta::uptime_history::{PRODUCTION, RAW_DAYS};
use openlens_meta::{Meta, Probe, UptimeDay};
use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};

/// The longest range `uptime.days` answers, in days.
pub const MAX_DAYS: i64 = 400;
/// The range when none is given: the Uptime page's 90 days.
pub const DEFAULT_DAYS: i64 = 90;
/// A day with at least this many seconds down is red; less is partial.
pub const DOWN_DAY_SECS: i64 = 3600;
/// Incidents per page when the caller gives no limit.
pub const PAGE: u32 = 50;

/// One day of the range.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Day {
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// `up` (no down time), `partial` (down less than an hour), `down`
    /// (an hour or more) or `none` (no check that day: not measured).
    pub state: &'static str,
    /// Share of the day's checks that succeeded.
    pub availability: Option<f64>,
    /// Seconds measured and not down.
    pub up_seconds: i64,
    /// The day's numbers, when it had a check.
    pub stats: Option<UptimeDay>,
}

/// The days of a range, oldest first, every day listed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Days {
    /// Environment.
    pub environment: String,
    /// First day (UTC, `YYYY-MM-DD`).
    pub from: String,
    /// Last day.
    pub to: String,
    /// Share of the range's checks that succeeded.
    pub availability: Option<f64>,
    /// Seconds measured and not down over the range.
    pub up_seconds: i64,
    /// Seconds measured over the range.
    pub measured_seconds: i64,
    /// Every day of the range.
    pub days: Vec<Day>,
}

/// The colour of a day.
#[must_use]
pub fn state(d: Option<&UptimeDay>) -> &'static str {
    match d {
        None => "none",
        Some(d) if d.checks == 0 => "none",
        Some(d) if d.down_seconds >= DOWN_DAY_SECS => "down",
        Some(d) if d.down_seconds > 0 => "partial",
        Some(_) => "up",
    }
}

fn parse_day(s: &str, what: &str) -> Result<NaiveDate, Error> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| Error::Invalid(format!("{what}: a day as YYYY-MM-DD")))
}

/// The range asked for: `to` defaults to `today`, `from` to the
/// [`DEFAULT_DAYS`] ending on `to`; at most [`MAX_DAYS`].
///
/// # Errors
/// A day that does not parse, `from` after `to`, or too long a range.
pub fn range(
    from: Option<&str>,
    to: Option<&str>,
    today: NaiveDate,
) -> Result<(NaiveDate, NaiveDate), Error> {
    let to = to.map_or(Ok(today), |t| parse_day(t, "to"))?;
    let from = match from {
        Some(f) => parse_day(f, "from")?,
        None => to
            .checked_sub_days(chrono::Days::new(
                u64::try_from(DEFAULT_DAYS - 1).unwrap_or(0),
            ))
            .ok_or_else(|| Error::Invalid("to: a day as YYYY-MM-DD".into()))?,
    };
    if from > to {
        return Err(Error::Invalid("from is after to".into()));
    }
    if (to - from).num_days() + 1 > MAX_DAYS {
        return Err(Error::Invalid(format!("at most {MAX_DAYS} days")));
    }
    Ok((from, to))
}

/// An environment name as stored with checks.
///
/// # Errors
/// An empty, long or odd name.
pub fn valid_env(env: &str) -> Result<(), Error> {
    if env.is_empty()
        || env.len() > 64
        || !env
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Error::Invalid(
            "environment: 1 to 64 letters, digits, '-', '_' or '.'".into(),
        ));
    }
    Ok(())
}

fn storage(e: openlens_meta::MetaError) -> Error {
    Error::Storage(e.to_string())
}

/// The days of `env` from `from` to `to`, as of `now`: the stored rows,
/// with today and yesterday computed from the raw checks (not closed, or
/// closed less than an hour ago). A day with no check is `none`.
///
/// # Errors
/// The store.
pub fn days(
    meta: &Meta,
    project: &str,
    env: &str,
    from: NaiveDate,
    to: NaiveDate,
    now: DateTime<Utc>,
) -> Result<Days, Error> {
    let mut rows: HashMap<String, UptimeDay> = meta
        .uptime_days(project, env, from, to)
        .map_err(storage)?
        .into_iter()
        .map(|d| (d.day.clone(), d))
        .collect();
    let today = now.date_naive();
    for d in [today.pred_opt(), Some(today)].into_iter().flatten() {
        if d >= from
            && d <= to
            && let Some(live) = meta
                .uptime_day_live(project, env, d, now.timestamp())
                .map_err(storage)?
        {
            rows.insert(live.day.clone(), live);
        }
    }
    let n = usize::try_from((to - from).num_days() + 1).unwrap_or(0);
    let days: Vec<Day> = from
        .iter_days()
        .take(n)
        .map(|d| {
            let key = d.format("%Y-%m-%d").to_string();
            let stats = rows.remove(&key);
            Day {
                state: state(stats.as_ref()),
                availability: stats.as_ref().and_then(UptimeDay::availability),
                up_seconds: stats.as_ref().map_or(0, UptimeDay::up_seconds),
                day: key,
                stats,
            }
        })
        .collect();
    let (mut checks, mut failed, mut measured) = (0i64, 0i64, 0i64);
    for s in days.iter().filter_map(|d| d.stats.as_ref()) {
        checks += s.checks;
        failed += s.failed;
        measured += s.measured_seconds;
    }
    Ok(Days {
        environment: env.to_owned(),
        from: from.format("%Y-%m-%d").to_string(),
        to: to.format("%Y-%m-%d").to_string(),
        availability: (checks > 0).then(|| (checks - failed) as f64 / checks as f64),
        up_seconds: days.iter().map(|d| d.up_seconds).sum(),
        measured_seconds: measured,
        days,
    })
}

/// Write the last `back` days and today of every environment with a
/// recent check, from their raw checks. Running it again replaces the
/// rows: nothing is lost or counted twice.
fn rebuild(meta: &Meta, project: &str, back: u64, now: DateTime<Utc>) {
    let today = now.date_naive();
    let Some(first) = today.checked_sub_days(chrono::Days::new(back)) else {
        return;
    };
    let since = first.and_time(chrono::NaiveTime::MIN).and_utc().timestamp();
    let envs = meta.uptime_environments(project, since).unwrap_or_default();
    for env in envs {
        for d in first.iter_days().take_while(|d| *d <= today) {
            let _ = meta.rebuild_uptime_day(project, &env, d, now.timestamp());
        }
    }
}

/// After a start: write the days whose raw checks are all still there
/// (today and the [`RAW_DAYS`] − 1 before), so a restart loses no day.
pub fn rebuild_recent(meta: &Meta, project: &str, now: DateTime<Utc>) {
    rebuild(meta, project, u64::try_from(RAW_DAYS - 1).unwrap_or(0), now);
}

/// Write yesterday (closed) and today (so far): the server's job runs it
/// every hour, so a day is written within the hour after it ends.
pub fn close_days(meta: &Meta, project: &str, now: DateTime<Utc>) {
    rebuild(meta, project, 1, now);
}

/// Record a probe of a monitored non-production environment (#410), so
/// its days survive a restart.
pub fn record_env_probe(meta: &Meta, project: &str, env: &str, probe: &Probe<'_>) {
    if env == PRODUCTION || valid_env(env).is_err() {
        return;
    }
    let now = Utc::now().timestamp();
    let _ = meta.record_check_in(project, env, now, probe);
}

/// The input of `uptime.days`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaysInput {
    /// Project id.
    pub project: String,
    /// First day (`YYYY-MM-DD`, UTC); default: 89 days before `to`.
    #[serde(default)]
    pub from: Option<String>,
    /// Last day (`YYYY-MM-DD`, UTC); default: today.
    #[serde(default)]
    pub to: Option<String>,
    /// Environment; default `production`.
    #[serde(default)]
    pub environment: Option<String>,
}

/// `uptime.days`: one row per UTC day of a range (at most 400 days):
/// state, availability, seconds up and measured, checks, failed checks,
/// down incidents and response times.
pub struct GetDays;

impl Command for GetDays {
    const NAME: &'static str = "uptime.days";
    const ACCESS: Access = Access::Read;
    type Input = DaysInput;
    type Output = Days;

    async fn run<H: Host>(host: &H, _: &Caller, input: DaysInput) -> Result<Days, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let env = input.environment.as_deref().unwrap_or(PRODUCTION);
        valid_env(env)?;
        let now = Utc::now();
        let (from, to) = range(input.from.as_deref(), input.to.as_deref(), now.date_naive())?;
        days(host.meta(), &p.id, env, from, to, now)
    }
}

/// An incident of the paged list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PagedIncident {
    /// Id: pass the last one as `before` for the next page.
    pub id: i64,
    /// Its rule (`down`, `traffic`, `insight:…`, `env:<env>:down`…).
    pub rule: String,
    /// Start (Unix seconds).
    pub start: i64,
    /// End (Unix seconds), once closed.
    pub end: Option<i64>,
    /// How long it lasted, or has lasted so far, in minutes.
    pub minutes: i64,
    /// What happened.
    pub cause: String,
}

/// One page of incidents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncidentPage {
    /// Newest first.
    pub incidents: Vec<PagedIncident>,
    /// The `before` of the next page, when there may be one.
    pub next: Option<i64>,
}

/// The input of `uptime.incidents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentsInput {
    /// Project id.
    pub project: String,
    /// Only incidents with an id below it (the previous page's `next`).
    #[serde(default)]
    pub before: Option<i64>,
    /// Page size, 1 to 100 (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// A page of a project's incidents, newest first.
///
/// # Errors
/// The store.
pub fn incidents(
    meta: &Meta,
    project: &str,
    before: Option<i64>,
    limit: u32,
    now: i64,
) -> Result<IncidentPage, Error> {
    let limit = limit.clamp(1, 100);
    let rows = meta
        .incidents_page(project, before, limit)
        .map_err(storage)?;
    let full = rows.len() == usize::try_from(limit).unwrap_or(usize::MAX);
    let next = rows.last().map(|r| r.id).filter(|_| full);
    Ok(IncidentPage {
        incidents: rows
            .into_iter()
            .map(|r| PagedIncident {
                minutes: (r.ended_at.unwrap_or(now) - r.started_at).max(0) / 60,
                id: r.id,
                rule: r.rule,
                start: r.started_at,
                end: r.ended_at,
                cause: r.cause,
            })
            .collect(),
        next,
    })
}

/// `uptime.incidents`: every incident of a project, newest first, page by
/// page (incidents are kept as long as the project).
pub struct GetIncidents;

impl Command for GetIncidents {
    const NAME: &'static str = "uptime.incidents";
    const ACCESS: Access = Access::Read;
    type Input = IncidentsInput;
    type Output = IncidentPage;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: IncidentsInput,
    ) -> Result<IncidentPage, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        incidents(
            host.meta(),
            &p.id,
            input.before,
            input.limit.unwrap_or(PAGE),
            Utc::now().timestamp(),
        )
    }
}

//! Uptime history (#559, D-075): what is kept of the probe's checks, and
//! for how long.
//!
//! - Raw checks: [`RAW_DAYS`] days.
//! - Raw checks from [`MARGIN_SECS`] before an availability incident starts
//!   to [`MARGIN_SECS`] after it ends: marked `kept` and deleted only after
//!   [`KEPT_DAYS`] days, for an investigation.
//! - One row per project, environment and UTC day ([`UptimeDay`]), kept as
//!   long as the project: what the 90-day bars and an SLO read.
//!
//! Production's checks have the environment [`PRODUCTION`]; a monitored
//! non-production environment's (#410) are stored the same way under its
//! name, so its days survive a restart.

use chrono::{Days, NaiveDate, NaiveTime};

use super::*;

/// Raw checks are kept this many days.
pub const RAW_DAYS: i64 = 7;
/// Checks around an incident are kept this many days.
pub const KEPT_DAYS: i64 = 365;
/// Checks this many seconds before an incident starts and after it ends
/// are kept with it.
pub const MARGIN_SECS: i64 = 3600;
/// The probe runs once a minute: one check stands for this many seconds.
pub const PROBE_SECS: i64 = 60;
/// The environment of production's checks.
pub const PRODUCTION: &str = "production";
/// Most *down* incidents [`Meta::down_spans`] answers.
pub const MAX_SPANS: i64 = 10_000;
/// Longest environment name stored with a check.
const MAX_ENV: usize = 64;

/// The new table and columns (called by `migrate`).
pub(crate) fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS uptime_days (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  day TEXT NOT NULL,
  checks INTEGER NOT NULL,
  failed INTEGER NOT NULL,
  down_seconds INTEGER NOT NULL,
  measured_seconds INTEGER NOT NULL,
  incidents INTEGER NOT NULL,
  p50_ms INTEGER,
  p95_ms INTEGER,
  max_ms INTEGER,
  PRIMARY KEY (project, environment, day)
);",
    )?;
    for (col, ddl) in [
        (
            "environment",
            "ALTER TABLE uptime_checks ADD COLUMN environment TEXT NOT NULL DEFAULT 'production'",
        ),
        (
            "kept",
            "ALTER TABLE uptime_checks ADD COLUMN kept INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        let has: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('uptime_checks') WHERE name = ?1")?
            .exists([col])?;
        if !has {
            c.execute_batch(ddl)?;
        }
    }
    c.execute_batch(
        "CREATE INDEX IF NOT EXISTS uptime_checks_env ON uptime_checks (project, environment, at);
CREATE INDEX IF NOT EXISTS uptime_checks_kept ON uptime_checks (project, kept, at);",
    )
}

/// One UTC day of one environment's checks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UptimeDay {
    /// Environment (`production`, or a monitored one).
    pub environment: String,
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// Checks made.
    pub checks: i64,
    /// Checks that failed (no answer, or HTTP 5xx).
    pub failed: i64,
    /// Seconds of the day inside a *down* incident.
    pub down_seconds: i64,
    /// Seconds of the day the probe covered (one check stands for
    /// [`PROBE_SECS`]; at most a day).
    pub measured_seconds: i64,
    /// *Down* incidents that overlap the day.
    pub incidents: i64,
    /// Median response time, ms.
    pub p50_ms: Option<i64>,
    /// 95th percentile response time, ms.
    pub p95_ms: Option<i64>,
    /// Slowest response, ms.
    pub max_ms: Option<i64>,
}

impl UptimeDay {
    /// Seconds measured and not down.
    #[must_use]
    pub fn up_seconds(&self) -> i64 {
        (self.measured_seconds - self.down_seconds).max(0)
    }

    /// Share of checks that succeeded, when there were any.
    #[must_use]
    pub fn availability(&self) -> Option<f64> {
        (self.checks > 0).then(|| (self.checks - self.failed) as f64 / self.checks as f64)
    }
}

/// What one probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe<'a> {
    /// Up: an answer below HTTP 500.
    pub ok: bool,
    /// The HTTP status, when there was an answer.
    pub status: Option<u16>,
    /// Response time, ms, when there was an answer.
    pub ms: Option<i64>,
    /// Why it failed.
    pub error: Option<&'a str>,
}

/// An incident with its id, for a paged list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IncidentRow {
    /// Id (the cursor of the next page).
    pub id: i64,
    /// Rule (`down`, `traffic`, `env:<env>:down`…).
    pub rule: String,
    /// Start (Unix seconds).
    pub started_at: i64,
    /// End (Unix seconds), once closed.
    pub ended_at: Option<i64>,
    /// What happened.
    pub cause: String,
    /// Whether it was announced.
    pub notified: bool,
}

/// The incident rules whose window keeps an environment's checks: the
/// app's availability (production's *down*, *traffic stopped*, *failing
/// requests*; an environment's `env:<env>:down`, the engine's
/// `incident_rule` format).
fn kept_rules(env: &str) -> [String; 3] {
    if env == PRODUCTION {
        ["down".into(), "traffic".into(), "failing".into()]
    } else {
        let r = format!("env:{env}:down");
        [r.clone(), r.clone(), r]
    }
}

/// The rule whose incidents count as down time for `env`.
fn down_rule(env: &str) -> String {
    if env == PRODUCTION {
        "down".into()
    } else {
        format!("env:{env}:down")
    }
}

/// The environment whose checks an incident of `rule` keeps, if any.
fn env_of(rule: &str) -> Option<String> {
    match rule {
        "down" | "traffic" | "failing" => Some(PRODUCTION.into()),
        r => r
            .strip_prefix("env:")
            .and_then(|r| r.strip_suffix(":down"))
            .map(str::to_owned),
    }
}

/// Mark the checks around incident `id` as kept (called when it starts
/// and when it ends; the checks made later are marked as they come).
pub(crate) fn mark_incident(c: &Connection, id: i64) -> rusqlite::Result<()> {
    let row: Option<(String, String, i64, Option<i64>)> = c
        .query_row(
            "SELECT project, rule, started_at, ended_at FROM incidents WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((project, rule, start, end)) = row else {
        return Ok(());
    };
    let Some(env) = env_of(&rule) else {
        return Ok(());
    };
    c.execute(
        "UPDATE uptime_checks SET kept = 1 WHERE project = ?1 AND environment = ?2 \
         AND at >= ?3 AND at <= ?4 AND kept = 0",
        params![
            project,
            env,
            start - MARGIN_SECS,
            end.unwrap_or(start) + MARGIN_SECS
        ],
    )?;
    Ok(())
}

/// Seconds since the Unix epoch of a UTC day's start, and of the next.
fn bounds(day: NaiveDate) -> Option<(i64, i64)> {
    let start = day.and_time(NaiveTime::MIN).and_utc().timestamp();
    let end = day
        .checked_add_days(Days::new(1))?
        .and_time(NaiveTime::MIN)
        .and_utc()
        .timestamp();
    Some((start, end))
}

/// The value at quantile `q` of sorted values (nearest rank, as the
/// Uptime page's 24-hour figures).
fn pct(sorted: &[i64], q: f64) -> Option<i64> {
    let last = sorted.len().checked_sub(1)?;
    sorted.get((last as f64 * q).round() as usize).copied()
}

/// One day of `env` from its raw checks and incidents, as of `now`; `None`
/// when it has no check.
fn compute(
    c: &Connection,
    project: &str,
    env: &str,
    day: NaiveDate,
    now: i64,
) -> Result<Option<UptimeDay>> {
    let (start, end) = bounds(day).ok_or(MetaError::Invalid("day out of range"))?;
    let mut st = c.prepare(
        "SELECT ok, ms FROM uptime_checks WHERE project = ?1 AND environment = ?2 AND at >= ?3 AND at < ?4",
    )?;
    let rows = st
        .query_map(params![project, env, start, end], |r| {
            Ok((r.get::<_, i64>(0)? != 0, r.get::<_, Option<i64>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(None);
    }
    let checks = i64::try_from(rows.len()).unwrap_or(i64::MAX);
    let failed = i64::try_from(rows.iter().filter(|r| !r.0).count()).unwrap_or(i64::MAX);
    let mut ms: Vec<i64> = rows.iter().filter_map(|r| r.1).collect();
    ms.sort_unstable();
    let mut st = c.prepare(
        "SELECT started_at, ended_at FROM incidents WHERE project = ?1 AND rule = ?2 \
         AND started_at < ?3 AND (ended_at IS NULL OR ended_at > ?4)",
    )?;
    let spans = st
        .query_map(params![project, down_rule(env), end, start], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let down: i64 = spans
        .iter()
        .map(|(s, e)| (e.unwrap_or(now).min(end) - (*s).max(start)).max(0))
        .sum();
    let day_secs = end - start;
    Ok(Some(UptimeDay {
        environment: env.to_owned(),
        day: day.format("%Y-%m-%d").to_string(),
        checks,
        failed,
        down_seconds: down.min(day_secs),
        measured_seconds: checks.saturating_mul(PROBE_SECS).min(day_secs),
        incidents: i64::try_from(spans.len()).unwrap_or(i64::MAX),
        p50_ms: pct(&ms, 0.5),
        p95_ms: pct(&ms, 0.95),
        max_ms: ms.last().copied(),
    }))
}

fn valid_env(env: &str) -> Result<()> {
    if env.is_empty() || env.len() > MAX_ENV {
        return Err(MetaError::Invalid("environment: 1 to 64 characters"));
    }
    Ok(())
}

fn day_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<UptimeDay> {
    Ok(UptimeDay {
        environment: r.get(0)?,
        day: r.get(1)?,
        checks: r.get(2)?,
        failed: r.get(3)?,
        down_seconds: r.get(4)?,
        measured_seconds: r.get(5)?,
        incidents: r.get(6)?,
        p50_ms: r.get(7)?,
        p95_ms: r.get(8)?,
        max_ms: r.get(9)?,
    })
}

impl Meta {
    /// Record one probe of `env` at `at` (Unix seconds). It is kept with an
    /// incident when one of the environment's availability rules is open,
    /// or ended within [`MARGIN_SECS`]; the project's checks older than
    /// [`RAW_DAYS`] that are not kept, and kept ones older than
    /// [`KEPT_DAYS`], are deleted.
    ///
    /// # Errors
    /// An environment name that is empty or too long, or the store.
    pub fn record_check_in(
        &self,
        project: &str,
        env: &str,
        at: i64,
        probe: &Probe<'_>,
    ) -> Result<()> {
        valid_env(env)?;
        let [r1, r2, r3] = kept_rules(env);
        self.with(|c| {
            let kept: bool = c.query_row(
                "SELECT EXISTS (SELECT 1 FROM incidents WHERE project = ?1 AND rule IN (?2, ?3, ?4) \
                 AND started_at <= ?5 + ?6 AND (ended_at IS NULL OR ended_at >= ?5 - ?6))",
                params![project, r1, r2, r3, at, MARGIN_SECS],
                |r| r.get(0),
            )?;
            c.execute(
                "INSERT INTO uptime_checks (project, environment, at, ok, status, ms, error, kept) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    project,
                    env,
                    at,
                    i64::from(probe.ok),
                    probe.status,
                    probe.ms,
                    probe.error,
                    i64::from(kept)
                ],
            )?;
            c.execute(
                "DELETE FROM uptime_checks WHERE project = ?1 AND kept = 0 AND at < ?2",
                params![project, at - RAW_DAYS * 86400],
            )?;
            c.execute(
                "DELETE FROM uptime_checks WHERE project = ?1 AND kept = 1 AND at < ?2",
                params![project, at - KEPT_DAYS * 86400],
            )?;
            Ok(())
        })
    }

    /// Checks of `env` from `since` (Unix seconds), oldest first:
    /// `(at, ok, status, ms, error)`.
    ///
    /// # Errors
    /// The store.
    pub fn checks_in(&self, project: &str, env: &str, since: i64) -> Result<Vec<Check>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT at, ok, status, ms, error FROM uptime_checks \
                 WHERE project = ?1 AND environment = ?2 AND at >= ?3 ORDER BY at",
            )?;
            let rows = st
                .query_map(params![project, env, since], |r| {
                    Ok((
                        r.get(0)?,
                        r.get::<_, i64>(1)? != 0,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The environments with a check since `since` (Unix seconds).
    ///
    /// # Errors
    /// The store.
    pub fn uptime_environments(&self, project: &str, since: i64) -> Result<Vec<String>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT DISTINCT environment FROM uptime_checks WHERE project = ?1 AND at >= ?2 ORDER BY environment",
            )?;
            let rows = st
                .query_map(params![project, since], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One day of `env` computed from its raw checks now, without storing
    /// it (a day not closed yet). `None` with no check.
    ///
    /// # Errors
    /// The store.
    pub fn uptime_day_live(
        &self,
        project: &str,
        env: &str,
        day: NaiveDate,
        now: i64,
    ) -> Result<Option<UptimeDay>> {
        self.read(|c| compute(c, project, env, day, now))
    }

    /// Compute one day of `env` from its raw checks and store it, replacing
    /// the row it had: running it again (a restart) neither loses nor
    /// doubles the day. Only call it for a day whose raw checks are all
    /// still there (within [`RAW_DAYS`]). Nothing is stored for a day with
    /// no check.
    ///
    /// # Errors
    /// The store.
    pub fn rebuild_uptime_day(
        &self,
        project: &str,
        env: &str,
        day: NaiveDate,
        now: i64,
    ) -> Result<Option<UptimeDay>> {
        self.with(|c| {
            let d = compute(c, project, env, day, now)?;
            if let Some(d) = &d {
                c.execute(
                    "INSERT OR REPLACE INTO uptime_days (project, environment, day, checks, failed, \
                     down_seconds, measured_seconds, incidents, p50_ms, p95_ms, max_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        project,
                        d.environment,
                        d.day,
                        d.checks,
                        d.failed,
                        d.down_seconds,
                        d.measured_seconds,
                        d.incidents,
                        d.p50_ms,
                        d.p95_ms,
                        d.max_ms
                    ],
                )?;
            }
            Ok(d)
        })
    }

    /// The *down* incidents of `env` that overlap `[from, to)` (Unix
    /// seconds): `(start, end)`, the end `None` while open; oldest first,
    /// at most [`MAX_SPANS`] (#560).
    ///
    /// # Errors
    /// The store.
    pub fn down_spans(
        &self,
        project: &str,
        env: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<(i64, Option<i64>)>> {
        let rule = down_rule(env);
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT started_at, ended_at FROM incidents WHERE project = ?1 AND rule = ?2 \
                 AND started_at < ?3 AND (ended_at IS NULL OR ended_at > ?4) \
                 ORDER BY started_at LIMIT ?5",
            )?;
            let rows = st
                .query_map(params![project, rule, to, from, MAX_SPANS], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The stored days of `env` from `from` to `to` (inclusive), oldest
    /// first. A day with no check has no row.
    ///
    /// # Errors
    /// The store.
    pub fn uptime_days(
        &self,
        project: &str,
        env: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<UptimeDay>> {
        let (from, to) = (
            from.format("%Y-%m-%d").to_string(),
            to.format("%Y-%m-%d").to_string(),
        );
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT environment, day, checks, failed, down_seconds, measured_seconds, incidents, \
                 p50_ms, p95_ms, max_ms FROM uptime_days \
                 WHERE project = ?1 AND environment = ?2 AND day >= ?3 AND day <= ?4 ORDER BY day",
            )?;
            let rows = st
                .query_map(params![project, env, from, to], day_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One page of a project's incidents, newest first: at most `limit`
    /// (1 to 100), with an id below `before` when given.
    ///
    /// # Errors
    /// The store.
    pub fn incidents_page(
        &self,
        project: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<IncidentRow>> {
        let limit = limit.clamp(1, 100);
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, rule, started_at, ended_at, cause, notified_at IS NOT NULL FROM incidents \
                 WHERE project = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
            )?;
            let rows = st
                .query_map(params![project, before.unwrap_or(i64::MAX), limit], |r| {
                    Ok(IncidentRow {
                        id: r.get(0)?,
                        rule: r.get(1)?,
                        started_at: r.get(2)?,
                        ended_at: r.get(3)?,
                        cause: r.get(4)?,
                        notified: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}

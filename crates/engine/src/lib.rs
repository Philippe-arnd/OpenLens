//! Commands shared by the dashboard, the CLI and the MCP server.
//!
//! Every user-visible number is computed here, once, from the KPI
//! definitions in `docs/kpis.md`. Front ends only render what these
//! commands return.
//!
//! Layer: L3 (see `docs/architecture.md`).

use chrono::{Duration, Utc};
use openlens_featuremap::FeatureMap;
use openlens_query::{QueryEngine, QueryError};
use serde::Serialize;
use serde_json::{Map, Value};

mod acquisition;
mod board;
mod board_people;
mod board_sessions;
mod board_window;
pub mod churn;
pub mod commands;
pub mod deploys;
mod digest;
mod drift;
pub mod environments;
mod errors;
mod events;
mod explore;
pub mod flows;
pub mod forms;
pub mod goals;
pub mod inforce;
mod insights;
mod mapdays;
pub mod maphealth;
pub mod mapsessions;
pub mod mapshape;
mod pages;
mod people;
pub mod reads;
pub mod relations;
pub mod releases;
pub mod replay_rate;
mod revenue;
pub mod rollups;
pub mod stats;
mod tasks;
mod variants;
pub mod vital_parts;
pub mod vitals;
pub use acquisition::{AcquisitionRow, Touch, acquisition};
pub use board::{board, filter_options};
pub use board_window::{BoardWindow, Period};
pub use churn::{AtRisk, ChurnReport, churn};
pub use digest::{Delta, Digest, digest, digest_markdown};
pub use errors::{ErrorGroup, error_groups};
pub use events::*;
pub use explore::*;
pub use flows::{Flow, FlowLink, FlowQuery, flows};
pub use inforce::MapAt;
pub use insights::{CohortStats, FlagCohort, Funnel, FunnelStep, Insight, Journey, Readiness};
pub use mapdays::feature_relations;
pub use maphealth::{FeatureContext, MapHealth, feature_context, map_health, map_health_in};
pub use pages::{
    ClickMap, Hotspot, MAP_COLUMNS, MAP_ROW, PageStats, SurveyResults, click_map, pages,
    sessions_on_path, survey_results,
};
pub use people::*;
pub use reads::MapLive;
pub use relations::Relation;
pub use replay_rate::{ReplayRate, replay_rate};
pub use revenue::{Money, Revenue, revenue};
pub use tasks::{RetentionRow, TaskTime};
pub use variants::*;
pub use vital_parts::{Culprit, DeviceVital};
pub use vitals::{PageVital, VitalGuide, VitalReport, vitals, vitals_guide};

/// Engine errors.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Query failed.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// A filter value is not acceptable.
    #[error("invalid filter: {0}")]
    InvalidFilter(&'static str),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, EngineError>;

/// The time window of the board's period blocks (D-055).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    /// Each block keeps its fixed window (7, 14 or 30 days): what alerts,
    /// goals, the digest, Ask and shared boards read.
    #[default]
    Fixed,
    /// Since the current release of the environment (90 days at most),
    /// against the same length before it; the last 14 days against the 14
    /// before when the environment has no release: the board pages.
    Release,
}

/// Filters every board number honours. No other knobs (D-014).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Filters {
    /// Environment (default `production`).
    pub environment: String,
    /// Raw role value, or all roles.
    pub role: Option<String>,
    /// Release, or all releases.
    pub release: Option<String>,
    /// Include the team's own users (identified with `internal: true`).
    /// Off by default: on a small app the team's clicks would weigh a lot.
    pub internal: bool,
    /// The project's time zone (IANA name): where a day and a week start
    /// (D-017). Set from the project, not from the request.
    pub tz: String,
    /// The window of the board's period blocks (D-055): fixed unless a
    /// board page asks for the release window.
    pub window: Window,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            environment: "production".into(),
            role: None,
            release: None,
            internal: false,
            tz: "UTC".into(),
            window: Window::Fixed,
        }
    }
}

/// Quote a user-supplied value as a SQL string literal. Values are bounded
/// and restricted to printable characters, then single quotes are doubled.
/// Bind a value from a request to a placeholder of `p` (#310).
pub(crate) fn bind(p: &mut openlens_sqlgen::Params, v: &str) -> Result<String> {
    p.bind(v).map_err(|_| EngineError::InvalidFilter("value"))
}

fn lit(v: &str) -> Result<String> {
    openlens_sqlgen::lit(v).map_err(|_| EngineError::InvalidFilter("value"))
}

impl Filters {
    /// The values of `clause`'s placeholders (#310).
    pub(crate) fn params(&self) -> Result<openlens_sqlgen::Params> {
        let mut p = openlens_sqlgen::Params::new();
        let bad = |_| EngineError::InvalidFilter("value");
        p.set("env", &self.environment).map_err(bad)?;
        if let Some(r) = &self.role {
            p.set("role", r).map_err(bad)?;
        }
        if let Some(r) = &self.release {
            p.set("release", r).map_err(bad)?;
        }
        Ok(p)
    }

    /// WHERE clause for these filters plus a time window of `days`.
    /// `expr` (a UTC timestamp) as wall-clock time in the project's zone,
    /// for `date_trunc` and casts to a date.
    pub(crate) fn local(&self, expr: &str) -> Result<String> {
        if self.tz == "UTC" {
            return Ok(expr.to_owned());
        }
        if self.tz.is_empty()
            || self.tz.len() > 64
            || !self
                .tz
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
        {
            return Err(EngineError::InvalidFilter("timezone"));
        }
        Ok(format!(
            "to_local_time(arrow_cast({expr}, 'Timestamp(Microsecond, Some(\"{}\"))'))",
            self.tz
        ))
    }

    /// The local day of the `timestamp` column, as `YYYY-MM-DD`.
    pub(crate) fn day(&self) -> Result<String> {
        if self.tz == "UTC" {
            return Ok("date".to_owned());
        }
        Ok(format!(
            "CAST(CAST({} AS DATE) AS VARCHAR)",
            self.local("timestamp")?
        ))
    }

    fn clause(&self, days: i64) -> Result<String> {
        self.clause_at(days, Utc::now())
    }

    /// [`Filters::clause`], with the window's first day taken at `now`.
    fn clause_at(&self, days: i64, now: chrono::DateTime<Utc>) -> Result<String> {
        self.clause_span(Span::days(days), now)
    }

    /// WHERE clause for these filters over the span `s`, its days taken
    /// at `now` (D-055).
    fn clause_span(&self, s: Span, now: chrono::DateTime<Utc>) -> Result<String> {
        let since = (now - Duration::seconds(s.start)).format("%Y-%m-%d");
        // The values are placeholders, filled from `params()` (#310): the
        // engine runs every query of a function on `q.with_params`.
        let mut w = format!(
            "date >= '{since}' AND timestamp >= now() - INTERVAL '{}'",
            interval(s.start)
        );
        if s.end > 0 {
            let until = (now - Duration::seconds(s.end)).format("%Y-%m-%d");
            w.push_str(&format!(
                " AND date <= '{until}' AND timestamp < now() - INTERVAL '{}'",
                interval(s.end)
            ));
        }
        w.push_str(" AND environment = $env");
        if self.role.is_some() {
            w.push_str(" AND role = $role");
        }
        if self.release.is_some() {
            w.push_str(" AND release = $release");
        }
        if !self.internal {
            // A lookup, not `NOT IN (SELECT … FROM persons)`: that anti join
            // was built on the events and returned them as one batch (#444).
            w.push_str(" AND NOT is_internal(distinct_id)");
        }
        Ok(w)
    }
}

/// A span of time before now: from `start` seconds ago to `end` seconds
/// ago (`end` 0: until now). The fixed windows are spans of whole days
/// ending now; the release window and its comparison are any span (D-055).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    /// Seconds before now the span starts.
    pub start: i64,
    /// Seconds before now the span ends.
    pub end: i64,
}

impl Span {
    /// The last `days` days.
    pub(crate) fn days(days: i64) -> Self {
        Self {
            start: days * 86_400,
            end: 0,
        }
    }
}

/// An SQL interval of `secs` seconds, in days when it is whole days (the
/// fixed windows' clause is unchanged).
fn interval(secs: i64) -> String {
    if secs % 86_400 == 0 {
        format!("{} days", secs / 86_400)
    } else {
        format!("{secs} seconds")
    }
}

/// Headline user numbers.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Users {
    /// Distinct identified users, last 24 h.
    pub dau: i64,
    /// Distinct users, last 7 days.
    pub wau: i64,
    /// Distinct users, last 30 days.
    pub mau: i64,
    /// DAU / MAU.
    pub stickiness: f64,
    /// Users back in the last 30 days after being inactive for longer
    /// than the churn window.
    pub resurrected: i64,
    /// Visitors who were never identified, last 30 days (not in the user
    /// counts above).
    pub anonymous: i64,
    /// Known users: identified users seen in the last 90 days, the
    /// denominator of the active shares (docs/kpis.md).
    pub known: i64,
}

impl Users {
    /// Share of known users active in the last 24 h.
    #[must_use]
    pub fn dau_share(&self) -> f64 {
        share(self.dau, self.known)
    }

    /// Share of known users active in the last 30 days.
    #[must_use]
    pub fn mau_share(&self) -> f64 {
        share(self.mau, self.known)
    }
}

fn share(n: i64, of: i64) -> f64 {
    if of > 0 {
        (n as f64 / of as f64).min(1.0)
    } else {
        0.0
    }
}

/// Session numbers, last 7 days.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Sessions {
    /// Number of sessions.
    pub count: i64,
    /// Mean duration of non-bounce sessions, seconds.
    pub avg_secs: f64,
    /// Share of single-event sessions.
    pub bounce_rate: f64,
    /// Share of sessions with at least one `$error`.
    pub error_rate: f64,
    /// Completed tasks (success events; feature uses without a map) per
    /// active user-day.
    pub tasks_per_user_day: f64,
}

/// One Web Vital at p75.
#[derive(Debug, Clone, Serialize)]
pub struct Vital {
    /// LCP, INP, CLS, FCP or TTFB.
    pub name: String,
    /// p75 value (ms, or unitless for CLS).
    pub p75: f64,
    /// good / needs-improvement / poor.
    pub rating: String,
    /// Samples.
    pub samples: i64,
}

/// Usage of one feature.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureUse {
    /// Feature key.
    pub key: String,
    /// Display name from the feature map.
    pub name: Option<String>,
    /// Uses (events carrying the feature).
    pub uses: i64,
    /// Distinct users.
    pub users: i64,
    /// Completed tasks: success events of this feature.
    pub completed: i64,
    /// Distinct users who reached the feature's success event.
    pub completed_users: i64,
    /// Sum over users of the distinct days they used the feature (so
    /// `user_days / users` is the active days per user, a stickiness cue).
    pub user_days: i64,
    /// Declared in the feature map.
    pub declared: bool,
}

/// Users per role.
#[derive(Debug, Clone, Serialize)]
pub struct RoleUsers {
    /// Raw role value (`None` for unidentified).
    pub role: Option<String>,
    /// Distinct users, last 30 days.
    pub users: i64,
}

/// KPIs of one release.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ReleaseRow {
    /// Release name.
    pub release: String,
    /// First time seen.
    pub first_seen: String,
    /// Distinct users.
    pub users: i64,
    /// Sessions.
    pub sessions: i64,
    /// Share of sessions with an error.
    pub error_rate: f64,
    /// Mean duration of non-bounce sessions, seconds.
    pub avg_secs: f64,
    /// Share of single-event sessions.
    pub bounce_rate: f64,
    /// Feature uses per session.
    pub tasks_per_session: f64,
}

/// Everything the Focus board shows (M1 subset; insight cards come in M3).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Board {
    /// Filters applied.
    pub filters: Option<Filters>,
    /// All events ever, all environments (drives the empty state).
    pub total_events: i64,
    /// User KPIs.
    pub users: Users,
    /// Session KPIs.
    pub sessions: Sessions,
    /// Daily active users, last 14 days, oldest first.
    pub dau_series: Vec<(String, i64)>,
    /// Web Vitals at p75, last 7 days.
    pub vitals: Vec<Vital>,
    /// The vitals that are not good, explained: worst pages and what to do.
    pub vitals_why: Vec<VitalReport>,
    /// Most used features, last 30 days.
    pub features: Vec<FeatureUse>,
    /// Users per role, last 30 days.
    pub roles: Vec<RoleUsers>,
    /// Weekly retention cohorts (first-seen week × weeks later).
    pub retention: Vec<RetentionRow>,
    /// Task durations per declared feature with a success event.
    pub task_times: Vec<TaskTime>,
    /// Who uses what: distinct users per (role, feature), last 30 days,
    /// ignoring the role filter (the roles × features grid).
    pub role_features: Vec<(String, String, i64)>,
    /// Latest releases, newest first.
    pub releases: Vec<ReleaseRow>,
    /// Set by the server when the app is down right now (#266): what and
    /// since when, for a banner at the top of the board.
    #[serde(skip)]
    pub app_down: Option<String>,
    /// Set by the server (#272): open objectives at risk or off track,
    /// for a strip at the top of the board.
    #[serde(skip)]
    pub objectives_at_risk: Vec<String>,
    /// Required setup steps (done, total), for admins while incomplete
    /// (#403): a one-line strip at the top of the board.
    #[serde(skip)]
    pub setup: Option<(usize, usize)>,
    /// Environments seen.
    pub environments: Vec<String>,
    /// Raw role values no rule of the feature map matches, with user counts.
    pub role_gaps: Vec<(String, i64)>,
    /// Version of the feature map in use, if any.
    pub map_version: Option<i64>,
    /// Ranked insight cards (at most 7).
    pub insights: Vec<Insight>,
    /// How close the project is to retention insights.
    pub readiness: Readiness,
    /// Frequent paths into each feature's success event.
    pub journeys: Vec<Journey>,
    /// Each recent flag's sessions against everyone else.
    pub flag_cohorts: Vec<FlagCohort>,
    /// Funnels from the feature map's declared steps.
    pub funnels: Vec<Funnel>,
    /// The release window of the period blocks and its comparison, when
    /// the board was asked for it (D-055); none: the fixed windows.
    pub window: Option<BoardWindow>,
}

fn int(row: &Map<String, Value>, k: &str) -> i64 {
    row.get(k).and_then(Value::as_i64).unwrap_or(0)
}
fn float(row: &Map<String, Value>, k: &str) -> f64 {
    row.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}
fn text(row: &Map<String, Value>, k: &str) -> Option<String> {
    row.get(k).and_then(Value::as_str).map(str::to_owned)
}

use vitals::rating;

/// The event source: the raw `events` table, or, with a feature map, a
/// projection where `role` is the canonical role and `feature` includes
/// route-matched features. Every query reads from this, so the map applies
/// everywhere without rewriting stored data.
pub fn source(map: Option<&FeatureMap>) -> String {
    match map {
        None => "events".into(),
        Some(m) => {
            let (role, feature) = lookups(m);
            format!(
                "(SELECT uuid, event, distinct_id, session_id, timestamp, received_at, url, path, \
                 account, plan, environment, release, flags, properties, date, {VITAL_COLUMNS}, {FACT_COLUMNS}, \
                 {role} AS role, {feature} AS feature FROM events) AS events"
            )
        }
    }
}

/// The role and feature expressions of `map` (#446): calls to functions
/// that apply its rules in Rust, defined once per set of rules, or the
/// reference `CASE` expressions when the rules cannot be applied in Rust
/// with the same result.
pub(crate) fn lookups(m: &FeatureMap) -> (String, String) {
    use std::hash::{Hash, Hasher};
    let (role_sql, feature_sql) = (m.role_sql(), m.feature_sql());
    // The SQL is the rules, written out: same SQL, same functions.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&role_sql, &feature_sql).hash(&mut h);
    let id = h.finish();
    let (role_fn, feature_fn) = (
        format!("role_of_{id:016x}"),
        format!("feature_of_{id:016x}"),
    );
    let defined = openlens_query::functions::is_defined(&role_fn)
        && openlens_query::functions::is_defined(&feature_fn);
    if !defined {
        let Some(r) = openlens_featuremap::Resolver::new(m) else {
            return (role_sql, feature_sql);
        };
        let r = std::sync::Arc::new(r);
        let rr = r.clone();
        openlens_query::functions::define(
            &role_fn,
            1,
            std::sync::Arc::new(move |a: &[Option<&str>], out: &mut String| {
                match rr.role(a.first().copied().flatten()) {
                    Some(v) => {
                        out.push_str(&v);
                        true
                    }
                    None => false,
                }
            }),
        );
        openlens_query::functions::define(
            &feature_fn,
            3,
            std::sync::Arc::new(move |a: &[Option<&str>], out: &mut String| {
                let arg = |i: usize| a.get(i).copied().flatten();
                match r.feature(arg(0), arg(1), arg(2)) {
                    Some(v) => {
                        out.push_str(v);
                        true
                    }
                    None => false,
                }
            }),
        );
    }
    (
        format!("{role_fn}(role)"),
        format!("{feature_fn}(event, path, feature)"),
    )
}

/// Only identified users (`persons`), unless the project never identified
/// anyone, in which case every visitor counts. User KPIs (DAU/WAU/MAU,
/// retention, cohorts) use it, so a visitor who never logged in is not a
/// user (docs/kpis.md); anonymous visitors are counted on their own.
///
/// Decided here, from the `persons` count, rather than in SQL: DataFusion
/// cannot plan `IN (…) OR NOT EXISTS (…)` as a semi-join (20× slower), and
/// a scalar-subquery form returned rows out of `ORDER BY` order (#159).
pub(crate) fn identified(any_person: bool) -> &'static str {
    if any_person {
        "distinct_id IN (SELECT person FROM persons)"
    } else {
        "TRUE"
    }
}

/// Whether the project identified anyone (the `persons` table is cached).
pub(crate) async fn any_person(q: &QueryEngine, project: &str) -> Result<bool> {
    let rows = q
        .sql(project, "SELECT count(*) AS n FROM persons")
        .await?
        .rows;
    Ok(rows.first().map_or(0, |r| int(r, "n")) > 0)
}

/// Events a person does on purpose: the rule `session_day` is rolled up
/// with, so the board's sessions read from the events and from the rollup
/// agree on bounces (D-041, D-047).
pub(crate) use openlens_query::INTERACTION;

/// The typed columns of a `$vital` (#449), written at ingestion.
const VITAL_COLUMNS: &str = "vital_name, vital_value, vital_device, vital_el, vital_kind, \
     vital_res, vital_ttfb, vital_load_delay, vital_load_time, vital_render_delay, \
     vital_input_delay, vital_processing, vital_presentation";

/// The typed error, acquisition and friction facts (#482), written at ingestion.
const FACT_COLUMNS: &str = "vital_rating, error_message, error_source, error_stack, acq_channel, \
     acq_utm_source, acq_utm_campaign, acq_ref_host, click_rage, facts_version";

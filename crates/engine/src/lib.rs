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
pub mod churn;
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
pub mod maphealth;
pub mod mapsessions;
pub mod mapshape;
mod pages;
mod people;
pub mod relations;
pub mod replay_rate;
mod revenue;
pub mod stats;
mod tasks;
mod variants;
pub mod vital_parts;
pub mod vitals;
pub use acquisition::{AcquisitionRow, Touch, acquisition};
pub use churn::{AtRisk, ChurnReport, churn};
pub use digest::{Delta, Digest, digest, digest_markdown};
pub use errors::{ErrorGroup, error_groups};
pub use events::*;
pub use explore::*;
pub use flows::{Flow, FlowLink, FlowQuery, flows};
pub use inforce::MapAt;
pub use insights::{CohortStats, FlagCohort, Funnel, FunnelStep, Insight, Journey, Readiness};
pub use maphealth::{FeatureContext, MapHealth, feature_context, map_health};
pub use pages::{
    ClickMap, Hotspot, MAP_COLUMNS, MAP_ROW, PageStats, SurveyResults, click_map, pages,
    sessions_on_path, survey_results,
};
pub use people::*;
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

/// Filters every board number honours. No other knobs (D-014).
#[derive(Debug, Clone, Serialize)]
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
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            environment: "production".into(),
            role: None,
            release: None,
            internal: false,
            tz: "UTC".into(),
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
        let since = (Utc::now() - Duration::days(days))
            .format("%Y-%m-%d")
            .to_string();
        // The values are placeholders, filled from `params()` (#310): the
        // engine runs every query of a function on `q.with_params`.
        let mut w = format!(
            "date >= '{since}' AND timestamp >= now() - INTERVAL '{days} days' AND environment = $env"
        );
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
                 account, plan, environment, release, flags, properties, date, {VITAL_COLUMNS}, \
                 {role} AS role, {feature} AS feature FROM events) AS events"
            )
        }
    }
}

/// The role and feature expressions of `map` (#446): calls to functions
/// that apply its rules in Rust, defined once per set of rules, or the
/// reference `CASE` expressions when the rules cannot be applied in Rust
/// with the same result.
fn lookups(m: &FeatureMap) -> (String, String) {
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

/// Events a person does on purpose. A session with one of them at most is a
/// bounce; `$scroll`, `$vital`, `$identify` or `$error` alone are not activity.
pub(crate) const INTERACTION: &str =
    "(event IN ('$pageview', '$click', '$submit', '$feature', '$survey') OR event NOT LIKE '$%')";

/// Largest 90-day window the board keeps in memory; beyond it, each query
/// reads the files. The snapshot holds only the columns the board reads,
/// and no `properties` (#449), so a row costs less than 160 bytes: 4 M rows
/// stay under ~650 MB (#190). The snapshot also reserves its bytes in the
/// query memory pool (768 MB, shared with every query): when the pool
/// cannot hold it, the board reads the files instead (#329).
const BOARD_SNAPSHOT_ROWS: usize = 4_000_000;

/// The columns of the board snapshot. `uuid`, `url`, `received_at`,
/// `account` and `plan` are not read by any board query, and `properties`
/// (most of an event's bytes) by none since Web Vitals have their own
/// columns and snapshot (#449).
const BOARD_COLUMNS: &str = "event, distinct_id, session_id, timestamp, path, feature, role, \
     environment, release, flags, date";

/// The typed columns of a `$vital` (#449), written at ingestion.
const VITAL_COLUMNS: &str = "vital_name, vital_value, vital_device, vital_el, vital_kind, \
     vital_res, vital_ttfb, vital_load_delay, vital_load_time, vital_render_delay, \
     vital_input_delay, vital_processing, vital_presentation";

/// The columns of the board's Web Vitals snapshot: what the 7-day filters
/// read, the page, and the typed facts.
fn vital_snapshot_columns() -> String {
    format!(
        "event, distinct_id, timestamp, path, role, environment, release, date, {VITAL_COLUMNS}"
    )
}

/// What the filter bar of a page needs, without the board (#329): the
/// environments, the roles (with the unmatched ones apart, as on the
/// board) and the latest releases. The Errors, Pages and Events pages call
/// this instead of computing the whole board.
pub async fn filter_options(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
) -> Result<Board> {
    let q = &q.with_params(&f.params()?);
    let src = source(map);
    let roles_sql = format!(
        "SELECT role, count(DISTINCT distinct_id) AS users FROM {src} WHERE {} \
         GROUP BY role ORDER BY users DESC LIMIT 12",
        Filters {
            role: None,
            ..f.clone()
        }
        .clause(30)?
    );
    let releases_sql = format!(
        "SELECT release, CAST(min(timestamp) AS VARCHAR) AS first_seen, \
         count(DISTINCT distinct_id) AS users FROM {src} WHERE {} AND release IS NOT NULL \
         GROUP BY release ORDER BY min(timestamp) DESC LIMIT 5",
        Filters {
            release: None,
            ..f.clone()
        }
        .clause(90)?
    );
    let (roles, releases, stored) = futures::join!(
        q.sql(project, &roles_sql),
        q.sql(project, &releases_sql),
        q.raw_summary(project),
    );
    let mut b = Board {
        filters: Some(f.clone()),
        ..Board::default()
    };
    for r in roles?.rows.iter() {
        let role = text(r, "role");
        let users = int(r, "users");
        match role.as_deref().and_then(|v| v.strip_prefix("unmatched:")) {
            Some(raw) => b.role_gaps.push((raw.to_owned(), users)),
            None => b.roles.push(RoleUsers { role, users }),
        }
    }
    b.releases = releases?
        .rows
        .iter()
        .map(|r| ReleaseRow {
            release: text(r, "release").unwrap_or_default(),
            first_seen: text(r, "first_seen").unwrap_or_default(),
            users: int(r, "users"),
            ..ReleaseRow::default()
        })
        .collect();
    b.environments = stored?.environments;
    Ok(b)
}

/// Compute the board for a project, with its feature map when it has one.
/// Queries run concurrently.
pub async fn board(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> Result<Board> {
    board_with(q, project, f, map, None).await
}

/// [`board`], sharing its 90-day snapshot with the boards computed at the
/// same time for other roles or releases of the same data `generation`
/// (#418): four filtered boards build one snapshot, not four.
pub async fn board_shared(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
    generation: u64,
) -> Result<Board> {
    board_with(q, project, f, map, Some(generation)).await
}

async fn board_with(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
    share: Option<u64>,
) -> Result<Board> {
    let q = &q.with_params(&f.params()?);
    // Every board query reads the same projection (roles and features
    // resolved from the map) over at most 90 days: compute it once, in
    // memory, unless it is too large, then each query only filters it.
    let window = Filters {
        role: None,
        release: None,
        ..f.clone()
    }
    .clause(90)?;
    let projected = source(map.map(|(m, _)| m));
    let snap_sql = format!("SELECT {BOARD_COLUMNS} FROM {projected} WHERE {window}");
    let snap = match share {
        Some(g) => {
            q.shared_snapshot(project, "board_events", &snap_sql, BOARD_SNAPSHOT_ROWS, g)
                .await?
        }
        None => {
            q.snapshot(project, "board_events", &snap_sql, BOARD_SNAPSHOT_ROWS)
                .await?
        }
    };
    let (q, src) = match &snap {
        Some(s) => (s, "board_events".to_owned()),
        None => (q, projected.clone()),
    };
    // Web Vitals look at 7 days of `$vital` rows only: their own small
    // snapshot of the typed columns, so the 90-day one carries no
    // `properties` (#449).
    let vital_window = Filters {
        role: None,
        release: None,
        ..f.clone()
    }
    .clause(7)?;
    let vital_sql = format!(
        "SELECT {} FROM {projected} WHERE {vital_window} AND event = '$vital'",
        vital_snapshot_columns()
    );
    let vsnap = match share {
        Some(g) => {
            q.shared_snapshot(project, "board_vitals", &vital_sql, BOARD_SNAPSHOT_ROWS, g)
                .await?
        }
        None => {
            q.snapshot(project, "board_vitals", &vital_sql, BOARD_SNAPSHOT_ROWS)
                .await?
        }
    };
    let (q, vsrc) = match &vsnap {
        Some(s) => (s, "board_vitals".to_owned()),
        None => (q, projected),
    };
    let ident = identified(any_person(q, project).await?);
    let w30 = f.clause(30)?;
    let w14 = f.clause(14)?;
    // Days are the project's days (D-017).
    let day = f.day()?;
    let w7 = f.clause(7)?;
    let all_releases = Filters {
        release: None,
        ..f.clone()
    }
    .clause(90)?;

    // Completed tasks are success events when the map declares them;
    // without a map, every feature use counts.
    let successes: Vec<&str> = map
        .map(|(m, _)| {
            m.features
                .iter()
                .filter_map(|x| x.success.as_deref())
                .collect()
        })
        .unwrap_or_default();
    let success_list = successes
        .iter()
        .map(|e| lit(e))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let task = if successes.is_empty() {
        "feature IS NOT NULL".to_owned()
    } else {
        format!("event IN ({success_list})")
    };
    let users_sql = format!(
        "SELECT count(DISTINCT CASE WHEN timestamp >= now() - INTERVAL '1 day' THEN distinct_id END) AS dau, \
         count(DISTINCT CASE WHEN timestamp >= now() - INTERVAL '7 days' THEN distinct_id END) AS wau, \
         count(DISTINCT distinct_id) AS mau FROM {src} WHERE {w30} AND {ident}"
    );
    let w90 = f.clause(90)?;
    let known_sql =
        format!("SELECT count(DISTINCT distinct_id) AS n FROM {src} WHERE {w90} AND {ident}");
    let anonymous_sql =
        format!("SELECT count(DISTINCT distinct_id) AS n FROM {src} WHERE {w30} AND NOT {ident}");
    let sessions_sql = format!(
        "SELECT count(*) AS sessions, \
         avg(CASE WHEN n > 1 THEN secs END) AS avg_secs, \
         avg(CASE WHEN n <= 1 THEN 1.0 ELSE 0.0 END) AS bounce_rate, \
         avg(CASE WHEN errors > 0 THEN 1.0 ELSE 0.0 END) AS error_rate, \
         sum(tasks) AS tasks, count(DISTINCT concat(uid, '|', day)) AS user_days \
         FROM (SELECT session_id, min(distinct_id) AS uid, min({day}) AS day, \
               sum(CASE WHEN {INTERACTION} THEN 1 ELSE 0 END) AS n, \
               date_part('epoch', max(timestamp)) - date_part('epoch', min(timestamp)) AS secs, \
               sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors, \
               sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS tasks \
               FROM {src} WHERE {w7} AND session_id IS NOT NULL GROUP BY session_id) s"
    );
    let dau_sql = format!(
        "SELECT {day} AS date, count(DISTINCT distinct_id) AS n FROM {src} WHERE {w14} AND {ident} GROUP BY 1 ORDER BY 1"
    );
    let vitals_sql = vitals::p75_sql(&vsrc, &w7);
    let features_sql = format!(
        "SELECT feature AS key, count(*) AS uses, count(DISTINCT distinct_id) AS users, \
         count(DISTINCT concat(distinct_id, '|', {day})) AS user_days \
         FROM {src} WHERE {w30} AND feature IS NOT NULL GROUP BY feature ORDER BY users DESC, uses DESC LIMIT 12"
    );
    let roles_sql = format!(
        "SELECT role, count(DISTINCT distinct_id) AS users FROM {src} WHERE {} \
         GROUP BY role ORDER BY users DESC LIMIT 12",
        Filters {
            role: None,
            ..f.clone()
        }
        .clause(30)?
    );
    let role_features_sql = format!(
        "SELECT role, feature, count(DISTINCT distinct_id) AS users FROM {src} WHERE {} \
         AND role IS NOT NULL AND feature IS NOT NULL GROUP BY role, feature",
        Filters {
            role: None,
            ..f.clone()
        }
        .clause(30)?
    );
    let releases_sql = format!(
        "SELECT release, CAST(min(first) AS VARCHAR) AS first_seen, count(DISTINCT uid) AS users, \
         count(*) AS sessions, avg(CASE WHEN n > 1 THEN secs END) AS avg_secs, \
         avg(CASE WHEN n <= 1 THEN 1.0 ELSE 0.0 END) AS bounce_rate, \
         avg(CASE WHEN errors > 0 THEN 1.0 ELSE 0.0 END) AS error_rate, \
         avg(CAST(tasks AS DOUBLE)) AS tasks_per_session \
         FROM (SELECT release, session_id, min(distinct_id) AS uid, min(timestamp) AS first, \
               sum(CASE WHEN {INTERACTION} THEN 1 ELSE 0 END) AS n, \
               date_part('epoch', max(timestamp)) - date_part('epoch', min(timestamp)) AS secs, \
               sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors, \
               sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS tasks \
               FROM {src} WHERE {all_releases} AND release IS NOT NULL AND session_id IS NOT NULL \
               GROUP BY release, session_id) s \
         GROUP BY release ORDER BY min(first) DESC LIMIT 5"
    );
    // Neither depends on who did what: they come from the segments, which
    // are read once (#447), not from a query.

    let success_sql = if successes.is_empty() {
        "SELECT CAST(NULL AS VARCHAR) AS event, 0 AS n, 0 AS users WHERE false".to_owned()
    } else {
        format!(
            "SELECT event, count(*) AS n, count(DISTINCT distinct_id) AS users FROM {src} WHERE {w30} AND event IN ({success_list}) GROUP BY event"
        )
    };
    let (
        users,
        sessions,
        dau,
        vitals,
        features,
        roles,
        releases,
        stored,
        success,
        role_features,
        anonymous,
        known,
    ) = futures::join!(
        q.sql(project, &users_sql),
        q.sql(project, &sessions_sql),
        q.sql(project, &dau_sql),
        q.sql(project, &vitals_sql),
        q.sql(project, &features_sql),
        q.sql(project, &roles_sql),
        q.sql(project, &releases_sql),
        q.raw_summary(project),
        q.sql(project, &success_sql),
        q.sql(project, &role_features_sql),
        q.sql(project, &anonymous_sql),
        q.sql(project, &known_sql),
    );

    let mut b = Board {
        filters: Some(f.clone()),
        ..Board::default()
    };
    if let Some(r) = users?.rows.first() {
        b.users = Users {
            dau: int(r, "dau"),
            wau: int(r, "wau"),
            mau: int(r, "mau"),
            stickiness: if int(r, "mau") > 0 {
                int(r, "dau") as f64 / int(r, "mau") as f64
            } else {
                0.0
            },
            resurrected: 0,
            anonymous: anonymous?.rows.first().map_or(0, |r| int(r, "n")),
            known: known?.rows.first().map_or(0, |r| int(r, "n")),
        };
    }
    if let Some(r) = sessions?.rows.first() {
        let user_days = int(r, "user_days").max(1) as f64;
        b.sessions = Sessions {
            count: int(r, "sessions"),
            avg_secs: float(r, "avg_secs"),
            bounce_rate: float(r, "bounce_rate"),
            error_rate: float(r, "error_rate"),
            tasks_per_user_day: int(r, "tasks") as f64 / user_days,
        };
    }
    b.dau_series = dau?
        .rows
        .iter()
        .map(|r| (text(r, "date").unwrap_or_default(), int(r, "n")))
        .collect();
    b.vitals = vitals?
        .rows
        .iter()
        .filter_map(|r| {
            let name = text(r, "name")?;
            let p75 = float(r, "p75");
            Some(Vital {
                rating: rating(&name, p75).into(),
                name,
                p75,
                samples: int(r, "samples"),
            })
        })
        .collect();
    b.features = features?
        .rows
        .iter()
        .map(|r| FeatureUse {
            key: text(r, "key").unwrap_or_default(),
            name: None,
            uses: int(r, "uses"),
            users: int(r, "users"),
            completed: 0,
            completed_users: 0,
            user_days: int(r, "user_days"),
            declared: false,
        })
        .collect();
    b.role_features = role_features?
        .rows
        .iter()
        .filter_map(|r| Some((text(r, "role")?, text(r, "feature")?, int(r, "users"))))
        .collect();
    let success_rows = success?.rows;
    if let Some((m, version)) = map {
        b.map_version = Some(version);
        for decl in &m.features {
            let of = |col: &str| {
                decl.success.as_deref().map_or(0, |ev| {
                    success_rows
                        .iter()
                        .filter(|r| text(r, "event").as_deref() == Some(ev))
                        .map(|r| int(r, col))
                        .sum()
                })
            };
            let (completed, completed_users) = (of("n"), of("users"));
            match b.features.iter_mut().find(|x| x.key == decl.key) {
                Some(x) => {
                    x.name = Some(decl.name.clone());
                    x.completed = completed;
                    x.completed_users = completed_users;
                    x.declared = true;
                }
                None => b.features.push(FeatureUse {
                    key: decl.key.clone(),
                    name: Some(decl.name.clone()),
                    uses: 0,
                    users: 0,
                    completed,
                    completed_users,
                    user_days: 0,
                    declared: true,
                }),
            }
        }
    }
    for r in roles?.rows.iter() {
        let role = text(r, "role");
        let users = int(r, "users");
        match role.as_deref().and_then(|v| v.strip_prefix("unmatched:")) {
            Some(raw) => b.role_gaps.push((raw.to_owned(), users)),
            None => b.roles.push(RoleUsers { role, users }),
        }
    }
    b.releases = releases?
        .rows
        .iter()
        .map(|r| ReleaseRow {
            release: text(r, "release").unwrap_or_default(),
            first_seen: text(r, "first_seen").unwrap_or_default(),
            users: int(r, "users"),
            sessions: int(r, "sessions"),
            error_rate: float(r, "error_rate"),
            avg_secs: float(r, "avg_secs"),
            bounce_rate: float(r, "bounce_rate"),
            tasks_per_session: float(r, "tasks_per_session"),
        })
        .collect();
    let stored = stored?;
    b.environments = stored.environments;
    let churn = map.map_or(14, |(m, _)| i64::from(m.churn_days()));
    let successes: Vec<(String, String)> = map
        .map(|(m, _)| {
            m.features
                .iter()
                .filter_map(|x| Some((x.key.clone(), x.success.clone()?)))
                .collect()
        })
        .unwrap_or_default();
    let funnel_defs: Vec<(String, Vec<String>)> = map
        .map(|(m, _)| {
            m.features
                .iter()
                .filter(|x| !x.steps.is_empty())
                .map(|x| {
                    let mut steps = x.steps.clone();
                    if let Some(s) = &x.success
                        && steps.last() != Some(s)
                    {
                        steps.push(s.clone());
                    }
                    (x.key.clone(), steps)
                })
                .collect()
        })
        .unwrap_or_default();
    let ((mut cards, readiness), journeys, cohorts, funnels, task_times, resurrected, retention) =
        futures::try_join!(
            insights::compute(q, project, &src, f, ident, churn, &successes),
            insights::journeys(q, project, &src, f, &successes),
            insights::flag_cohorts(q, project, &src, f),
            insights::funnels(q, project, &src, f, &funnel_defs),
            tasks::task_times(q, project, &src, f, &successes),
            tasks::resurrected(q, project, &src, f, churn),
            tasks::retention(q, project, &src, f, ident),
        )?;
    b.retention = retention;
    b.task_times = task_times;
    b.users.resurrected = resurrected;
    cards.extend(insights::funnel_cards(&funnels));
    b.funnels = funnels;
    b.journeys = journeys;
    cards.extend(insights::flag_cards(&cohorts));
    b.flag_cohorts = cohorts;
    if b.vitals
        .iter()
        .any(|v| v.rating != "good" && vitals_guide(&v.name).is_some_and(|g| g.pages))
    {
        let (pages, attr) = futures::join!(
            vitals::pages(q, project, &vsrc, f),
            vital_parts::attribution(q, project, &vsrc, f)
        );
        b.vitals_why = vitals::explain(&b.vitals, &pages?, &attr?, true);
    } else {
        b.vitals_why = vitals::explain(&b.vitals, &[], &vital_parts::Attribution::default(), true);
    }
    if let Some(c) = insights::slow_pages(&b.vitals, &b.vitals_why) {
        cards.push(c);
        cards.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
    cards.truncate(7);
    b.insights = cards;
    b.readiness = readiness;
    b.total_events = i64::try_from(stored.rows).unwrap_or(i64::MAX);
    Ok(b)
}

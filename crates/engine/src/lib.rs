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
mod errors;
pub mod flows;
pub mod goals;
mod insights;
mod pages;
mod revenue;
pub mod stats;
mod tasks;
pub use acquisition::{AcquisitionRow, Touch, acquisition};
pub use churn::{AtRisk, ChurnReport, churn};
pub use digest::{Delta, Digest, digest, digest_markdown};
pub use errors::{ErrorGroup, error_groups};
pub use flows::{Flow, FlowLink, FlowQuery, flows};
pub use insights::{CohortStats, FlagCohort, Funnel, FunnelStep, Insight, Journey, Readiness};
pub use pages::{
    ClickMap, Hotspot, MAP_COLUMNS, MAP_ROW, PageStats, SurveyResults, click_map, pages,
    sessions_on_path, survey_results,
};
pub use revenue::{Money, Revenue, revenue};
pub use tasks::{RetentionRow, TaskTime};

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
fn lit(v: &str) -> Result<String> {
    if v.is_empty() || v.len() > 200 || v.chars().any(char::is_control) {
        return Err(EngineError::InvalidFilter("value"));
    }
    Ok(format!("'{}'", v.replace('\'', "''")))
}

impl Filters {
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
        let mut w = format!(
            "date >= '{since}' AND timestamp >= now() - INTERVAL '{days} days' AND environment = {}",
            lit(&self.environment)?
        );
        if let Some(r) = &self.role {
            w.push_str(&format!(" AND role = {}", lit(r)?));
        }
        if let Some(r) = &self.release {
            w.push_str(&format!(" AND release = {}", lit(r)?));
        }
        if !self.internal {
            w.push_str(" AND distinct_id NOT IN (SELECT person FROM persons WHERE internal)");
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
#[derive(Debug, Clone, Serialize)]
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

fn rating(name: &str, v: f64) -> &'static str {
    let (good, poor) = match name {
        "LCP" => (2500.0, 4000.0),
        "INP" => (200.0, 500.0),
        "CLS" => (0.1, 0.25),
        "FCP" => (1800.0, 3000.0),
        "TTFB" => (800.0, 1800.0),
        _ => return "unknown",
    };
    if v <= good {
        "good"
    } else if v <= poor {
        "needs-improvement"
    } else {
        "poor"
    }
}

/// The event source: the raw `events` table, or, with a feature map, a
/// projection where `role` is the canonical role and `feature` includes
/// route-matched features. Every query reads from this, so the map applies
/// everywhere without rewriting stored data.
pub fn source(map: Option<&FeatureMap>) -> String {
    match map {
        None => "events".into(),
        Some(m) => format!(
            "(SELECT uuid, event, distinct_id, session_id, timestamp, received_at, url, path, \
             account, plan, environment, release, flags, properties, date, \
             {} AS role, {} AS feature FROM events) AS events",
            m.role_sql(),
            m.feature_sql()
        ),
    }
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
/// reads the files. The snapshot holds only the columns the board reads
/// (properties only for `$vital`), so a row costs about 160 bytes: 4 M rows
/// stay under ~650 MB (#190).
const BOARD_SNAPSHOT_ROWS: usize = 4_000_000;

/// The columns of the board snapshot. `uuid`, `url`, `received_at`,
/// `account` and `plan` are not read by any board query, and `properties`
/// (most of an event's bytes) only for Web Vitals.
const BOARD_COLUMNS: &str = "event, distinct_id, session_id, timestamp, path, feature, role, \
     environment, release, flags, date, \
     CASE WHEN event = '$vital' THEN properties END AS properties";

/// Compute the board for a project, with its feature map when it has one.
/// Queries run concurrently.
pub async fn board(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> Result<Board> {
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
    let snap = q
        .snapshot(
            project,
            "board_events",
            &format!("SELECT {BOARD_COLUMNS} FROM {projected} WHERE {window}"),
            BOARD_SNAPSHOT_ROWS,
        )
        .await?;
    let (q, src) = match &snap {
        Some(s) => (s, "board_events".to_owned()),
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
    let vitals_sql = format!(
        "SELECT name, approx_percentile_cont(v, 0.75) AS p75, count(*) AS samples FROM ( \
           SELECT regexp_match(properties, '\"\\$name\":\"([A-Z]+)\"')[1] AS name, \
                  CAST(regexp_match(properties, '\"\\$value\":([0-9.eE+-]+)')[1] AS DOUBLE) AS v \
           FROM {src} WHERE {w7} AND event = '$vital') x \
         WHERE name IS NOT NULL AND v IS NOT NULL GROUP BY name ORDER BY name"
    );
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
    // Neither depends on who did what: read the stored rows, without the
    // identity join.
    let env_sql = "SELECT DISTINCT environment FROM raw_events ORDER BY environment".to_owned();
    let total_sql = "SELECT count(*) AS n FROM raw_events".to_owned();

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
        envs,
        total,
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
        q.sql(project, &env_sql),
        q.sql(project, &total_sql),
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
    b.environments = envs?
        .rows
        .iter()
        .filter_map(|r| text(r, "environment"))
        .collect();
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
    if let Some(c) = insights::slow_pages(&b.vitals) {
        cards.push(c);
        cards.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
    cards.truncate(7);
    b.insights = cards;
    b.readiness = readiness;
    b.total_events = total?.rows.first().map_or(0, |r| int(r, "n"));
    Ok(b)
}

/// One row of the recent-events view.
#[derive(Debug, Clone, Serialize)]
pub struct EventRow {
    /// When.
    pub timestamp: String,
    /// Event name.
    pub event: String,
    /// User.
    pub distinct_id: String,
    /// Raw role.
    pub role: Option<String>,
    /// Page path.
    pub path: Option<String>,
    /// Feature.
    pub feature: Option<String>,
    /// Release.
    pub release: Option<String>,
    /// Session (links to its replay).
    pub session_id: Option<String>,
}

/// The last sessions of churned users whose last event was at `place` (a
/// feature or page, as on the churn-point card), newest first, with the time
/// of that last event: what to watch to see why they left.
pub async fn churned_sessions(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    place: &str,
) -> Result<Vec<(String, String)>> {
    let src = source(map);
    let churn = map.map_or(14, |m| i64::from(m.churn_days()));
    let ident = identified(any_person(q, project).await?);
    let base = insights::cohort_base(&src, &f.clause(90)?, ident, churn);
    let sql = format!(
        "{base}, churned AS (SELECT distinct_id, last_seen FROM cohort WHERE retained = 0) \
         SELECT e.session_id AS sid, CAST(e.timestamp AS VARCHAR) AS ts FROM ev e \
         JOIN churned ch ON e.distinct_id = ch.distinct_id AND e.timestamp = ch.last_seen \
         WHERE coalesce(e.feature, e.path) = {} AND e.session_id IS NOT NULL \
         ORDER BY e.timestamp DESC LIMIT 200",
        lit(place)?
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .iter()
        .filter_map(|r| Some((text(r, "sid")?, text(r, "ts")?)))
        .collect())
}

/// One value of a KPI breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct BreakdownRow {
    /// The role, account, plan, page or feature (`null` when unset).
    pub value: Option<String>,
    /// Distinct users, last 30 days.
    pub users: i64,
    /// Distinct sessions.
    pub sessions: i64,
    /// Events.
    pub events: i64,
    /// Feature uses (tasks).
    pub tasks: i64,
    /// Error events.
    pub errors: i64,
}

/// KPIs split by one dimension (`role`, `account`, `plan`, `page` or
/// `feature`), last 30 days, most users first (at most 50 values). Roles
/// are the role each event was sent with ("role at event time").
pub async fn breakdown(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    by: &str,
) -> Result<Vec<BreakdownRow>> {
    let col = match by {
        "role" => "role",
        "account" => "account",
        "plan" => "plan",
        "page" => "path",
        "feature" => "feature",
        "environment" => "environment",
        "release" => "release",
        // Audience (#225), from page views (other events carry none).
        "country" => "regexp_match(properties, '\"\\$country\":\"([A-Z]{2})\"')[1]",
        "device" => "regexp_match(properties, '\"\\$device\":\"([a-z]+)\"')[1]",
        "browser" => "regexp_match(properties, '\"\\$browser\":\"([A-Za-z ]+)\"')[1]",
        "os" => "regexp_match(properties, '\"\\$os\":\"([A-Za-z]+)\"')[1]",
        "channel" => "regexp_match(properties, '\"\\$channel\":\"([a-z]+)\"')[1]",
        _ => return Err(EngineError::InvalidFilter("by")),
    };
    let src = source(map);
    let mut w = f.clause(30)?;
    // By environment: every environment, side by side.
    if col == "environment" {
        w = w.replacen(
            &format!(" AND environment = {}", lit(&f.environment)?),
            "",
            1,
        );
    }
    let sql = format!(
        "SELECT {col} AS value, count(DISTINCT distinct_id) AS users, count(DISTINCT session_id) AS sessions, \
         count(*) AS events, sum(CASE WHEN feature IS NOT NULL THEN 1 ELSE 0 END) AS tasks, \
         sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors \
         FROM {src} WHERE {w} GROUP BY {col} ORDER BY users DESC, events DESC LIMIT 50"
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .iter()
        .map(|r| BreakdownRow {
            value: text(r, "value"),
            users: int(r, "users"),
            sessions: int(r, "sessions"),
            events: int(r, "events"),
            tasks: int(r, "tasks"),
            errors: int(r, "errors"),
        })
        .collect())
}

/// Who is using the app right now (#228): users active in the last 5 and
/// 30 minutes, and over 30 minutes the roles, releases, features, pages
/// and errors, plus the latest events.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Live {
    /// Distinct users, last 5 minutes.
    pub users_5m: i64,
    /// Distinct users, last 30 minutes.
    pub users_30m: i64,
    /// Errors, last 30 minutes.
    pub errors_30m: i64,
    /// (role, users), last 30 minutes.
    pub roles: Vec<(String, i64)>,
    /// (release, users), last 30 minutes.
    pub releases: Vec<(String, i64)>,
    /// (feature, users), last 30 minutes.
    pub features: Vec<(String, i64)>,
    /// (page, users), last 30 minutes.
    pub pages: Vec<(String, i64)>,
    /// Latest events, newest first.
    pub events: Vec<EventRow>,
}

/// [`Live`] for a project (the environment and team filters apply).
pub async fn live(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
) -> Result<Live> {
    let src = source(map);
    let w = format!(
        "{} AND timestamp >= now() - INTERVAL '30 minutes'",
        f.clause(1)?
    );
    let head = format!(
        "SELECT count(DISTINCT CASE WHEN timestamp >= now() - INTERVAL '5 minutes' THEN distinct_id END) AS u5, \
         count(DISTINCT distinct_id) AS u30, sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors \
         FROM {src} WHERE {w}"
    );
    let by = |col: &str| {
        format!(
            "SELECT {col} AS v, count(DISTINCT distinct_id) AS n FROM {src} WHERE {w} AND {col} IS NOT NULL \
             GROUP BY {col} ORDER BY n DESC LIMIT 10"
        )
    };
    let (qr, ql, qf, qp) = (by("role"), by("release"), by("feature"), by("path"));
    let eq = EventQuery {
        days: Some(1),
        limit: 30,
        ..EventQuery::default()
    };
    let (h, r, l, fe, pa, ev) = futures::join!(
        q.sql(project, &head),
        q.sql(project, &qr),
        q.sql(project, &ql),
        q.sql(project, &qf),
        q.sql(project, &qp),
        event_list(q, project, f, map, &eq),
    );
    let rows = |r: openlens_query::QueryResult| -> Vec<(String, i64)> {
        r.rows
            .iter()
            .filter_map(|x| Some((text(x, "v")?, int(x, "n"))))
            .collect()
    };
    let h = h?;
    let top = h.rows.first();
    Ok(Live {
        users_5m: top.map_or(0, |r| int(r, "u5")),
        users_30m: top.map_or(0, |r| int(r, "u30")),
        errors_30m: top.map_or(0, |r| int(r, "errors")),
        roles: rows(r?),
        releases: rows(l?),
        features: rows(fe?),
        pages: rows(pa?),
        events: ev?,
    })
}

/// Who the users are, from page views of the last 30 days (#225): distinct
/// users per country, device class, browser, OS, language and screen
/// width bucket. A user seen in two countries counts in both.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Audience {
    /// (ISO country code, users).
    pub countries: Vec<(String, i64)>,
    /// (phone / tablet / desktop, users).
    pub devices: Vec<(String, i64)>,
    /// (browser, users).
    pub browsers: Vec<(String, i64)>,
    /// (OS, users).
    pub os: Vec<(String, i64)>,
    /// (language, users), the first part of `$lang` (`fr`, `en`).
    pub languages: Vec<(String, i64)>,
    /// (screen bucket, users).
    pub screens: Vec<(String, i64)>,
    /// Users with at least one page view in the window.
    pub users: i64,
}

/// [`Audience`] for a project.
pub async fn audience(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
) -> Result<Audience> {
    let src = source(map);
    let w = f.clause(30)?;
    let prop = |k: &str, re: &str| format!("regexp_match(properties, '\"\\${k}\":\"({re})\"')[1]");
    let one = |expr: String| {
        format!(
            "SELECT v, count(DISTINCT distinct_id) AS n FROM (SELECT {expr} AS v, distinct_id FROM {src} \
             WHERE {w} AND event = '$pageview') WHERE v IS NOT NULL GROUP BY v ORDER BY n DESC LIMIT 60"
        )
    };
    let screen = "CASE WHEN w < 600 THEN 'phone (< 600 px)' WHEN w < 1024 THEN 'tablet (600–1023)' \
                  WHEN w < 1440 THEN 'laptop (1024–1439)' ELSE 'wide (≥ 1440)' END";
    let screens = format!(
        "SELECT v, count(DISTINCT distinct_id) AS n FROM (SELECT {screen} AS v, distinct_id FROM \
         (SELECT CAST(regexp_match(properties, '\"\\$sw\":([0-9]+)')[1] AS BIGINT) AS w, distinct_id \
          FROM {src} WHERE {w} AND event = '$pageview') WHERE w IS NOT NULL) GROUP BY v ORDER BY n DESC"
    );
    let total = format!(
        "SELECT count(DISTINCT distinct_id) AS n FROM {src} WHERE {w} AND event = '$pageview'"
    );
    let langs = format!(
        "lower(substr({}, 1, 2))",
        prop("lang", "[A-Za-z]{2}[A-Za-z-]*")
    );
    let (qc, qd, qb, qo, ql) = (
        one(prop("country", "[A-Z]{2}")),
        one(prop("device", "[a-z]+")),
        one(prop("browser", "[A-Za-z ]+")),
        one(prop("os", "[A-Za-z]+")),
        one(langs),
    );
    let (c, d, b, o, l, sc, t) = futures::join!(
        q.sql(project, &qc),
        q.sql(project, &qd),
        q.sql(project, &qb),
        q.sql(project, &qo),
        q.sql(project, &ql),
        q.sql(project, &screens),
        q.sql(project, &total),
    );
    let rows = |r: openlens_query::QueryResult| -> Vec<(String, i64)> {
        r.rows
            .iter()
            .filter_map(|x| Some((text(x, "v")?, int(x, "n"))))
            .collect()
    };
    Ok(Audience {
        countries: rows(c?),
        devices: rows(d?),
        browsers: rows(b?),
        os: rows(o?),
        languages: rows(l?),
        screens: rows(sc?),
        users: t?.rows.first().map_or(0, |r| int(r, "n")),
    })
}

/// One person's latest events across every environment (at most 200),
/// newest first: the raw material of a user timeline.
pub async fn person_events(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    distinct_id: &str,
    limit: usize,
) -> Result<Vec<EventRow>> {
    let src = source(map);
    let since = (Utc::now() - Duration::days(90)).format("%Y-%m-%d");
    let sql = format!(
        "SELECT CAST(timestamp AS VARCHAR) AS ts, event, distinct_id, role, path, feature, release, session_id \
         FROM {src} WHERE date >= '{since}' AND distinct_id = {} ORDER BY timestamp DESC LIMIT {}",
        lit(distinct_id)?,
        limit.min(200)
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .iter()
        .map(|r| EventRow {
            timestamp: text(r, "ts").unwrap_or_default(),
            event: text(r, "event").unwrap_or_default(),
            distinct_id: text(r, "distinct_id").unwrap_or_default(),
            role: text(r, "role"),
            path: text(r, "path"),
            feature: text(r, "feature"),
            release: text(r, "release"),
            session_id: text(r, "session_id"),
        })
        .collect())
}

/// The latest events, newest first (at most 200).
pub async fn recent_events(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
    event: Option<&str>,
    limit: usize,
) -> Result<Vec<EventRow>> {
    let eq = EventQuery {
        event: event.map(str::to_owned),
        limit,
        ..EventQuery::default()
    };
    event_list(q, project, f, map, &eq).await
}

/// Event families, read at a glance on the events page (#217): the key,
/// its label, and the event names in it. `custom` is every event the app
/// names itself (no `$`), `other` the remaining `$` events.
pub const FAMILIES: &[(&str, &str, &[&str])] = &[
    ("page", "Page views", &["$pageview", "$scroll"]),
    ("click", "Clicks", &["$click", "$rage"]),
    ("form", "Forms", &["$submit"]),
    ("feature", "Features", &["$feature"]),
    ("identify", "Logins", &["$identify"]),
    ("error", "Errors", &["$error", "$request_failed"]),
    ("vital", "Web vitals", &["$vital"]),
    ("survey", "Surveys", &["$survey"]),
    ("custom", "App events", &[]),
    ("other", "Other", &[]),
];

/// The family of an event name (a key of [`FAMILIES`]).
#[must_use]
pub fn family(event: &str) -> &'static str {
    if !event.starts_with('$') {
        return "custom";
    }
    FAMILIES
        .iter()
        .find(|(_, _, names)| names.contains(&event))
        .map_or("other", |(k, _, _)| k)
}

/// How the events page lists events: filters, sort and position.
#[derive(Debug, Clone, Default)]
pub struct EventQuery {
    /// One event name.
    pub event: Option<String>,
    /// A family key of [`FAMILIES`].
    pub family: Option<String>,
    /// One user (`distinct_id`).
    pub person: Option<String>,
    /// Text the page path contains.
    pub path: Option<String>,
    /// Days back (1 to 90; default 30).
    pub days: Option<i64>,
    /// `time` (newest first, default), `event` or `person`.
    pub sort: Option<String>,
    /// Newest-first cursor: only events strictly older than this timestamp.
    pub before: Option<String>,
    /// Rows to skip (event and person sorts; at most 2000).
    pub skip: usize,
    /// Rows to return (at most 200).
    pub limit: usize,
}

/// Events matching an [`EventQuery`].
pub async fn event_list(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
    eq: &EventQuery,
) -> Result<Vec<EventRow>> {
    let src = source(map);
    let mut w = f.clause(eq.days.unwrap_or(30).clamp(1, 90))?;
    if let Some(e) = &eq.event {
        w.push_str(&format!(" AND event = {}", lit(e)?));
    }
    if let Some(p) = &eq.person {
        w.push_str(&format!(" AND distinct_id = {}", lit(p)?));
    }
    if let Some(p) = &eq.path {
        w.push_str(&format!(" AND strpos(path, {}) > 0", lit(p)?));
    }
    if let Some(fam) = eq.family.as_deref() {
        let known = |names: &[&str]| -> Result<String> {
            Ok(names
                .iter()
                .map(|n| lit(n))
                .collect::<Result<Vec<_>>>()?
                .join(", "))
        };
        match fam {
            "custom" => w.push_str(" AND event NOT LIKE '$%'"),
            "other" => {
                let all: Vec<&str> = FAMILIES
                    .iter()
                    .flat_map(|(_, _, n)| n.iter().copied())
                    .collect();
                w.push_str(&format!(
                    " AND event LIKE '$%' AND event NOT IN ({})",
                    known(&all)?
                ));
            }
            k => match FAMILIES.iter().find(|(key, _, _)| *key == k) {
                Some((_, _, names)) => w.push_str(&format!(" AND event IN ({})", known(names)?)),
                None => return Err(EngineError::InvalidFilter("unknown event family")),
            },
        }
    }
    let order = match eq.sort.as_deref() {
        Some("event") => "event, timestamp DESC",
        Some("person") => "distinct_id, timestamp DESC",
        _ => {
            if let Some(b) = &eq.before {
                w.push_str(&format!(" AND timestamp < CAST({} AS TIMESTAMP)", lit(b)?));
            }
            "timestamp DESC"
        }
    };
    let skip = if eq
        .sort
        .as_deref()
        .is_some_and(|s| s == "event" || s == "person")
    {
        eq.skip.min(2000)
    } else {
        0
    };
    let sql = format!(
        "SELECT CAST(timestamp AS VARCHAR) AS ts, event, distinct_id, role, path, feature, release, session_id \
         FROM {src} WHERE {w} ORDER BY {order} LIMIT {} OFFSET {skip}",
        eq.limit.clamp(1, 200)
    );
    let r = q.sql(project, &sql).await?;
    Ok(r.rows
        .iter()
        .map(|r| EventRow {
            timestamp: text(r, "ts").unwrap_or_default(),
            event: text(r, "event").unwrap_or_default(),
            distinct_id: text(r, "distinct_id").unwrap_or_default(),
            role: text(r, "role"),
            path: text(r, "path"),
            feature: text(r, "feature"),
            release: text(r, "release"),
            session_id: text(r, "session_id"),
        })
        .collect())
}

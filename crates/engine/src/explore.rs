//! Event rows, breakdowns and the live view. Moved out of lib.rs as is (#307).

use super::*;

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
    let q = &q.with_params(&f.params()?);
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
    let q = &q.with_params(&f.params()?);
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
    let q = &q.with_params(&f.params()?);
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

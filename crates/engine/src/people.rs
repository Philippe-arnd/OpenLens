//! Audiences and the events of one person. Moved out of lib.rs as is (#307).

use super::*;

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
    let q = &q.with_params(&f.params()?);
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
    let mut params = openlens_sqlgen::Params::new();
    let sql = format!(
        "SELECT CAST(timestamp AS VARCHAR) AS ts, event, distinct_id, role, path, feature, release, session_id \
         FROM {src} WHERE date >= '{since}' AND distinct_id = {} ORDER BY timestamp DESC LIMIT {}",
        crate::bind(&mut params, distinct_id)?,
        limit.min(200)
    );
    let q = &q.with_params(&params);
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

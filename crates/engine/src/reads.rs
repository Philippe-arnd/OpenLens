//! Small reads the server's pages and handlers need, moved out of the
//! server so it holds no SQL (#509, D-058). None of them is a user
//! action: each one feeds a page, an export or a check, and the adapter
//! keeps deciding what to do when it fails.

use openlens_query::QueryResult;

use super::*;

/// What the last 7 days say about a feature map (#221).
#[derive(Debug, Default)]
pub struct MapLive {
    /// (feature, uses, users).
    pub uses: Vec<(String, i64, i64)>,
    /// (success event, hits).
    pub success_hits: Vec<(String, i64)>,
    /// Raw role values no role of the map matches.
    pub unmapped_roles: Vec<String>,
    /// Raw plan values no plan of the map matches (#712); empty when the
    /// map declares no plans.
    pub unmapped_plans: Vec<String>,
}

/// [`MapLive`] for a project: uses and users per feature, hits per
/// success event, raw roles no role matches. A query that fails leaves
/// its part empty.
pub async fn map_live(q: &QueryEngine, project: &str, map: Option<&FeatureMap>) -> MapLive {
    let mut out = MapLive::default();
    let Some(m) = map else { return out };
    let since = (Utc::now() - Duration::days(7)).format("%Y-%m-%d");
    let src = source(Some(m));
    let uses = format!(
        "SELECT feature, count(*) AS n, count(DISTINCT distinct_id) AS u FROM {src} \
         WHERE date >= '{since}' AND feature IS NOT NULL GROUP BY feature"
    );
    let roles = format!(
        "SELECT DISTINCT role FROM raw_events WHERE date >= '{since}' AND role IS NOT NULL LIMIT 200"
    );
    // Bound, not spliced (#310).
    let mut params = openlens_sqlgen::Params::new();
    let successes: Vec<String> = m
        .features
        .iter()
        .filter_map(|f| f.success.as_deref())
        .filter_map(|s| params.bind(s).ok())
        .collect();
    let hits = (!successes.is_empty()).then(|| {
        format!(
            "SELECT event, count(*) AS n FROM events WHERE date >= '{since}' AND event IN ({}) GROUP BY event",
            successes.join(", ")
        )
    });
    let plans = format!(
        "SELECT DISTINCT plan FROM raw_events WHERE date >= '{since}' AND plan IS NOT NULL LIMIT 200"
    );
    let plans_fut = async {
        if m.plans.is_empty() {
            None
        } else {
            Some(q.sql(project, &plans).await)
        }
    };
    let (a, b, c) = futures::join!(q.sql(project, &uses), q.sql(project, &roles), plans_fut);
    if let Some(Ok(r)) = c {
        for row in &r.rows {
            if let Some(raw) = text(row, "plan")
                && m.resolve_plan(&raw).is_none()
            {
                out.unmapped_plans.push(raw);
            }
        }
    }
    if let Ok(r) = a {
        for row in &r.rows {
            if let Some(f) = text(row, "feature") {
                out.uses.push((f, int(row, "n"), int(row, "u")));
            }
        }
    }
    if let Ok(r) = b {
        for row in &r.rows {
            if let Some(raw) = text(row, "role")
                && m.resolve_role(&raw).is_none()
                && !m.roles.is_empty()
            {
                out.unmapped_roles.push(raw);
            }
        }
    }
    if let Some(sql) = hits
        && let Ok(r) = q.with_params(&params).sql(project, &sql).await
    {
        for row in &r.rows {
            if let Some(e) = text(row, "event") {
                out.success_hits.push((e, int(row, "n")));
            }
        }
    }
    out
}

/// The anonymous ids `$identify` linked to a person in the stored events
/// (the person's own id left out). Empty when the id cannot be bound or
/// the query fails.
pub async fn linked_anonymous_ids(q: &QueryEngine, project: &str, who: &str) -> Vec<String> {
    let mut params = openlens_sqlgen::Params::new();
    let Ok(person) = params.bind(who) else {
        return Vec::new();
    };
    let sql = format!(
        "SELECT DISTINCT regexp_match(properties, '\"\\$anon_distinct_id\":\"([^\"]+)\"')[1] AS anon \
         FROM raw_events WHERE event = '$identify' AND distinct_id = {person}"
    );
    q.with_params(&params)
        .sql(project, &sql)
        .await
        .map(|r| {
            r.rows
                .iter()
                .filter_map(|x| x.get("anon").and_then(|v| v.as_str()).map(str::to_owned))
                .filter(|a| a != who)
                .collect()
        })
        .unwrap_or_default()
}

/// Every stored event of these ids, oldest first (the right-of-access
/// export, #230). Ids that cannot be bound are left out; `q`'s limits
/// decide how many rows come back.
pub async fn person_events(
    q: &QueryEngine,
    project: &str,
    ids: &[String],
) -> std::result::Result<QueryResult, QueryError> {
    let mut params = openlens_sqlgen::Params::new();
    let list = ids
        .iter()
        .filter_map(|i| params.bind(i).ok())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT uuid, event, distinct_id, session_id, CAST(timestamp AS VARCHAR) AS timestamp, url, path, feature, \
         role, account, plan, environment, release, properties FROM raw_events WHERE distinct_id IN ({list}) ORDER BY timestamp"
    );
    q.with_params(&params).sql(project, &sql).await
}

/// True for a `YYYY-MM-DD` shaped day (digits and dashes in place).
#[must_use]
pub fn is_day(date: &str) -> bool {
    let d = date.as_bytes();
    d.len() == 10
        && d.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// One day of a project's events, oldest first (the NDJSON export).
pub async fn day_events(q: &QueryEngine, project: &str, date: &str) -> Result<QueryResult> {
    if !is_day(date) {
        return Err(EngineError::InvalidFilter("date"));
    }
    let sql = format!(
        "SELECT uuid, event, distinct_id, session_id, CAST(timestamp AS VARCHAR) AS timestamp, url, path, feature, \
         role, account, plan, environment, release, properties FROM events WHERE date = '{date}' ORDER BY timestamp"
    );
    Ok(q.sql(project, &sql).await?)
}

/// What arrived over the last 24 hours, for the settings checklist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DayCounts {
    /// Events.
    pub events: i64,
    /// `$identify` events.
    pub identified: i64,
    /// Events with a role.
    pub with_role: i64,
    /// Distinct releases.
    pub releases: i64,
}

/// [`DayCounts`] for a project; zeros when the query fails.
pub async fn day_counts(q: &QueryEngine, project: &str) -> DayCounts {
    let sql = "SELECT count(*) AS events, \
               count(CASE WHEN event = '$identify' THEN 1 END) AS identified, \
               count(role) AS with_role, count(DISTINCT release) AS releases \
               FROM events WHERE timestamp >= now() - INTERVAL '1 day'";
    let row = q
        .sql(project, sql)
        .await
        .ok()
        .and_then(|r| r.rows.into_iter().next());
    row.map_or_else(DayCounts::default, |r| DayCounts {
        events: int(&r, "events"),
        identified: int(&r, "identified"),
        with_role: int(&r, "with_role"),
        releases: int(&r, "releases"),
    })
}

/// Events over the last 7 days and the time of the latest one, for the
/// projects page; `(0, None)` when the query fails.
pub async fn recent_activity(q: &QueryEngine, project: &str) -> (i64, Option<String>) {
    let since = (Utc::now() - Duration::days(7)).format("%Y-%m-%d");
    let sql = format!(
        "SELECT count(*) AS n, CAST(max(timestamp) AS VARCHAR) AS last FROM events WHERE date >= '{since}'"
    );
    let row = q
        .sql(project, &sql)
        .await
        .ok()
        .and_then(|r| r.rows.into_iter().next());
    row.map_or((0, None), |r| (int(&r, "n"), text(&r, "last")))
}

/// `scheme://host` most page views of the last 7 days came from, as
/// stored (not checked: the caller parses it).
pub async fn top_page_origin(q: &QueryEngine, project: &str) -> Option<String> {
    let since = (Utc::now() - Duration::days(7)).format("%Y-%m-%d");
    let sql = format!(
        "SELECT split_part(url, '/', 1) || '//' || split_part(url, '/', 3) AS o, count(*) AS n \
         FROM events WHERE date >= '{since}' AND event = '$pageview' AND url LIKE 'http%' \
         GROUP BY 1 ORDER BY n DESC LIMIT 1"
    );
    let r = q.sql(project, &sql).await.ok()?;
    text(r.rows.first()?, "o")
}

/// A session's OpenLens events (page views, clicks, submits, errors,
/// success and custom events) for the markers on the replay player's
/// timeline: at most 1,000, oldest first.
pub async fn session_markers(
    q: &QueryEngine,
    project: &str,
    session: &str,
) -> Result<Vec<Map<String, Value>>> {
    let sql = "SELECT event, CAST(date_part('epoch', timestamp) * 1000 AS BIGINT) AS ms, feature, path, \
         regexp_match(properties, '\"\\$message\":\"([^\"]{0,120})')[1] AS message, \
         properties LIKE '%\"$rage\":true%' AS rage \
         FROM events WHERE session_id = $session \
         AND event NOT IN ('$scroll', '$vital', '$identify') ORDER BY timestamp LIMIT 1000";
    let mut params = openlens_sqlgen::Params::new();
    params
        .set("session", session)
        .map_err(|_| EngineError::InvalidFilter("session"))?;
    Ok(q.with_params(&params).sql(project, sql).await?.rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_day_shaped_date_is_a_day() {
        assert!(is_day("2026-11-26"));
        for bad in [
            "2026-1-26",
            "2026/11/26",
            "2026-11-2x",
            "26-11-2026",
            "2026-11-26'",
        ] {
            assert!(!is_day(bad), "{bad}");
        }
    }
}

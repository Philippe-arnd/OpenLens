//! Server routes (#594, D-083): the `$request_stats` the server SDKs send
//! (#593, D-082), one data point of OpenTelemetry's
//! `http.server.request.duration` histogram per (method, route, status
//! code) per stats window (#601, D-084), summed per route. The window is
//! 1, 5 or 10 minutes (#628, D-091), mixed within a day after a change:
//! every read sums histograms, so none depends on its length.
//!
//! - **Requests, 5xx and duration** per route, the duration as a merged
//!   histogram: p50 and p95 are the upper bound of the bucket that holds
//!   them (the last bucket reads the largest duration seen).
//! - **In the verdict** ([`checks`]): a route with at least
//!   [`MIN_REQUESTS`] requests on both releases is worse when its 5xx share
//!   goes up significantly by at least [`MIN_5XX_MOVE`], or when its p95 is
//!   at least [`P95_RATIO`] times the one before and [`P95_MIN_MS`] above
//!   it. A release with no stats keeps the verdict it had.
//! - **Hourly** ([`failing_routes`]): a route whose 5xx share over the
//!   last hour is far above its previous seven days.
//! - **Key actions:** a feature map's `server:` routes. Their regression is
//!   named as such, and their alert is urgent (quiet hours never hold it).
//! - **Exemplars** (#607, D-089): a point may carry real requests
//!   (`exemplars`, OpenTelemetry's shape). Per route, the slowest of the
//!   period and its first 5xx are returned ([`Exemplar`]), with their
//!   session when known, so a page can open the replay at that time; a
//!   slower route's p95 check carries its slowest.
//!
//! The stats are read from `raw_events`: the `events` view leaves them
//! out, so no user KPI ever counts them.

use std::collections::HashMap;

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::verdict::{Check, Outcome};
use crate::{EngineError, Result, int, stats, text};

/// Upper bounds of the histogram buckets as the SDKs send them: one more
/// bucket holds the rest.
/// OpenTelemetry's default explicit bounds of `http.server.request.duration`
/// (0.005 s to 10 s, #601), in ms.
pub const BOUNDS_MS: [f64; 14] = [
    5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0, 1000.0, 2500.0, 5000.0, 7500.0,
    10000.0,
];
/// Buckets per histogram.
pub const BUCKETS: usize = BOUNDS_MS.len() + 1;
/// Requests a route needs on both releases to be judged.
pub const MIN_REQUESTS: i64 = 50;
/// Smallest rise of a route's 5xx share that counts as worse.
pub const MIN_5XX_MOVE: f64 = 0.02;
/// Ratio of p95 from which a route counts as slower…
pub const P95_RATIO: f64 = 1.5;
/// …when it is also this many milliseconds slower.
pub const P95_MIN_MS: f64 = 200.0;
/// Routes read at most.
pub const MAX_ROUTES: usize = 500;
/// Days `routes.list` reads at most.
pub const MAX_DAYS: i64 = 90;
/// Days of each route's p95 trend (#622).
pub const TREND_DAYS: i64 = 10;

/// One route's numbers.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RouteStat {
    /// `GET`, `POST`…
    pub method: String,
    /// Route template, e.g. `/api/entries/:id`.
    pub route: String,
    /// Requests.
    pub requests: i64,
    /// Requests answered 5xx.
    pub errors: i64,
    /// Sum of the durations, ms.
    pub sum_ms: i64,
    /// Largest duration, ms.
    pub max_ms: i64,
    /// Requests per duration bucket ([`BOUNDS_MS`]).
    pub buckets: Vec<i64>,
    /// The feature whose key action this route is (`server:` in the map).
    pub key_of: Option<String>,
    /// The slowest request of the period with an exemplar (#607).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slowest: Option<Exemplar>,
    /// The first request of the period answered 5xx with an exemplar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_5xx: Option<Exemplar>,
}

/// One real request behind a route's numbers (#607, D-089): what an
/// exemplar holds once read, never a body, header, cookie or query string,
/// and never the user (the session leads to them, and is erased with them).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Exemplar {
    /// Its duration, ms.
    pub ms: f64,
    /// When it was measured (RFC 3339, UTC).
    pub time: String,
    /// The status code it was answered.
    pub status: i64,
    /// Its trace id (32 hex digits), when the app traces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// The OpenLens session it belongs to, when known: its replay at `time`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Exemplars read per data point at most.
pub const MAX_POINT_EXEMPLARS: usize = 4;
/// Largest exemplar duration kept, seconds (a day): a bound, not a measure.
pub const MAX_EXEMPLAR_S: f64 = 86_400.0;

/// A session id as the SDKs send it: 1 to 100 of `A-Za-z0-9_-`.
#[must_use]
pub fn valid_session(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A W3C trace id: 32 lowercase hex digits, not all zero.
#[must_use]
pub fn valid_trace_id(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && s.bytes().any(|b| b != b'0')
}

/// The exemplars of a stored data point (its `properties` JSON) answered
/// `status`: at most [`MAX_POINT_EXEMPLARS`], each bounded; one with no
/// valid duration or time is skipped, an invalid trace id or session is
/// left out. Any other attribute (a `user.id` included) is never read.
#[must_use]
pub fn exemplars_of(properties: &str, status: i64) -> Vec<Exemplar> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(properties) else {
        return Vec::new();
    };
    let Some(list) = v.get("exemplars").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .take(MAX_POINT_EXEMPLARS)
        .filter_map(|e| {
            let secs = e
                .get("value")
                .and_then(serde_json::Value::as_f64)
                .filter(|s| s.is_finite() && (0.0..=MAX_EXEMPLAR_S).contains(s))?;
            let time = e
                .get("time")
                .and_then(serde_json::Value::as_str)
                .filter(|t| t.len() <= 64)
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())?
                .with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let attrs = e.get("filtered_attributes");
            let attr = |k: &str| {
                attrs
                    .and_then(|a| a.get(k))
                    .and_then(serde_json::Value::as_str)
            };
            Some(Exemplar {
                ms: (secs * 1_000_000.0).round() / 1000.0,
                time,
                status,
                trace_id: e
                    .get("trace_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|t| valid_trace_id(t))
                    .map(str::to_owned),
                session_id: attr("session.id")
                    .filter(|s| valid_session(s))
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// A route's two exemplars.
#[derive(Debug, Clone, Default)]
struct Picked {
    slowest: Option<Exemplar>,
    first_5xx: Option<Exemplar>,
}

/// The SQL reading, per route (and per the `extra` expression, selected as
/// `part`) where `w` holds, the data point with exemplars and the slowest
/// request (`slow_rk = 1`) and the first data point answered 5xx with
/// exemplars (`err_rk = 1`), with their properties; `limit` rows at most.
fn exemplars_sql(w: &str, extra: Option<&str>, limit: usize) -> String {
    let route = attr_sql("http\\.route", "\"((?:[^\"\\\\]|\\\\.){1,300})\"");
    let method = attr_sql("http\\.request\\.method", "\"([A-Z]{1,10})\"");
    let status = int_sql("http\\.response\\.status_code");
    let max = secs_sql("max");
    let (sel, part) = extra.map_or((String::new(), String::new()), |e| {
        (format!("{e} AS part, "), "part, ".to_owned())
    });
    format!(
        "WITH s AS (SELECT {sel}{method} AS method, {route} AS route, {status} AS status, \
         CASE WHEN {status} >= 500 THEN 1 ELSE 0 END AS bad, {max} AS mx, timestamp AS ts, \
         properties AS props FROM raw_events WHERE event = '{event}' AND {w} \
         AND properties LIKE '%\"exemplars\":[{{%'), \
         r AS (SELECT *, row_number() OVER (PARTITION BY {part}method, route ORDER BY mx DESC, ts) AS slow_rk, \
         row_number() OVER (PARTITION BY {part}method, route, bad ORDER BY ts, mx DESC) AS err_rk FROM s \
         WHERE method IS NOT NULL AND route IS NOT NULL) \
         SELECT {part}method, route, status, bad, slow_rk, err_rk, props FROM r \
         WHERE slow_rk = 1 OR (bad = 1 AND err_rk = 1) LIMIT {limit}",
        event = openlens_event::names::REQUEST_STATS,
    )
}

/// The exemplars of [`exemplars_sql`]'s rows, by (`part`, route name).
fn picked(
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> HashMap<(Option<String>, String), Picked> {
    let mut out: HashMap<(Option<String>, String), Picked> = HashMap::new();
    for r in rows {
        let (Some(method), Some(route), Some(props)) =
            (text(r, "method"), text(r, "route"), text(r, "props"))
        else {
            continue;
        };
        let status = int(r, "status");
        let list = exemplars_of(&props, status);
        let e = out
            .entry((text(r, "part"), format!("{method} {route}")))
            .or_default();
        if int(r, "slow_rk") == 1
            && let Some(x) = list.iter().max_by(|a, b| a.ms.total_cmp(&b.ms))
            && e.slowest.as_ref().is_none_or(|s| x.ms > s.ms)
        {
            e.slowest = Some(x.clone());
        }
        if int(r, "bad") == 1
            && int(r, "err_rk") == 1
            && let Some(x) = list.iter().min_by(|a, b| a.time.cmp(&b.time))
            && e.first_5xx.as_ref().is_none_or(|s| x.time < s.time)
        {
            e.first_5xx = Some(x.clone());
        }
    }
    out
}

/// Put each route's exemplars of `part` on it.
fn attach(
    list: &mut [RouteStat],
    picked: &HashMap<(Option<String>, String), Picked>,
    part: Option<&str>,
) {
    for s in list {
        if let Some(p) = picked.get(&(part.map(str::to_owned), s.name())) {
            s.slowest.clone_from(&p.slowest);
            s.first_5xx.clone_from(&p.first_5xx);
        }
    }
}

impl RouteStat {
    /// `POST /api/login`.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{} {}", self.method, self.route)
    }
    /// Share of requests answered 5xx.
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        self.errors as f64 / self.requests.max(1) as f64
    }
    /// Mean duration, ms.
    #[must_use]
    pub fn mean_ms(&self) -> f64 {
        self.sum_ms as f64 / self.requests.max(1) as f64
    }
    /// The `q` quantile (0 to 1), ms: the upper bound of its bucket.
    #[must_use]
    pub fn quantile(&self, q: f64) -> f64 {
        let total: i64 = self.buckets.iter().sum();
        if total == 0 {
            return 0.0;
        }
        let rank = (q * total as f64).ceil().max(1.0) as i64;
        let mut seen = 0;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += n;
            if seen >= rank {
                return BOUNDS_MS
                    .get(i)
                    .copied()
                    .unwrap_or(self.max_ms as f64)
                    .min(self.max_ms.max(1) as f64);
            }
        }
        self.max_ms as f64
    }
    /// p50, ms.
    #[must_use]
    pub fn p50(&self) -> f64 {
        self.quantile(0.5)
    }
    /// p95, ms.
    #[must_use]
    pub fn p95(&self) -> f64 {
        self.quantile(0.95)
    }
    /// Add `other`'s requests and histogram to this one: the quantiles of
    /// the result are those of both together, never an average of theirs.
    pub fn merge(&mut self, other: &RouteStat) {
        self.requests = self.requests.saturating_add(other.requests);
        self.errors = self.errors.saturating_add(other.errors);
        self.sum_ms = self.sum_ms.saturating_add(other.sum_ms);
        self.max_ms = self.max_ms.max(other.max_ms);
        if self.buckets.len() < other.buckets.len() {
            self.buckets.resize(other.buckets.len(), 0);
        }
        for (a, b) in self.buckets.iter_mut().zip(&other.buckets) {
            *a = a.saturating_add(*b);
        }
    }
}

/// `GET /api/login` → the features whose key action it is, from the map's
/// `server:` entries.
#[must_use]
pub fn key_actions(map: Option<&FeatureMap>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for f in map.map(|m| m.features.as_slice()).unwrap_or_default() {
        for r in &f.server {
            out.insert(normalize(r), f.key.clone());
        }
    }
    out
}

/// `post  /api/x` → `POST /api/x`.
fn normalize(r: &str) -> String {
    let mut it = r.split_whitespace();
    match (it.next(), it.next()) {
        (Some(m), Some(p)) => format!("{} {p}", m.to_uppercase()),
        (Some(p), None) => p.to_owned(),
        _ => String::new(),
    }
}

/// The SQL of property `k` (a regex-escaped key) matching `pat`.
fn attr_sql(k: &str, pat: &str) -> String {
    format!("regexp_match(properties, '\"{k}\":{pat}')[1]")
}

/// The SQL of integer property `k`, 0 when absent.
fn int_sql(k: &str) -> String {
    format!(
        "COALESCE(CAST({} AS BIGINT), 0)",
        attr_sql(k, "([0-9]{1,18})")
    )
}

/// The SQL of property `k` in seconds, 0 when absent.
fn secs_sql(k: &str) -> String {
    format!(
        "COALESCE(CAST({} AS DOUBLE), 0)",
        attr_sql(k, "([0-9]{1,12}(?:\\.[0-9]{1,9})?(?:[eE][-+]?[0-9]{1,3})?)")
    )
}

/// The SQL summing the stats per route (and per the `extra` expression,
/// selected as `part`) where `w` holds, `limit` rows at most; each
/// histogram bucket is its own column.
fn stats_sql(w: &str, extra: Option<&str>, limit: usize) -> String {
    // One data point of `http.server.request.duration` (#601): OpenTelemetry
    // semantic-convention attributes, durations in seconds.
    let attr = attr_sql;
    let int_of = int_sql;
    let secs_of = secs_sql;
    let buckets: Vec<String> = (1..=BUCKETS)
        .map(|i| {
            format!(
                "sum(COALESCE(CAST(NULLIF(split_part({}, ',', {i}), '') AS BIGINT), 0)) AS b{i}",
                attr("bucket_counts", "\\[([0-9,]{0,400})\\]")
            )
        })
        .collect();
    let route = attr("http\\.route", "\"((?:[^\"\\\\]|\\\\.){1,300})\"");
    let method = attr("http\\.request\\.method", "\"([A-Z]{1,10})\"");
    let status = int_of("http\\.response\\.status_code");
    let count = int_of("count");
    let (sel, group) = extra.map_or((String::new(), String::new()), |e| {
        (format!("{e} AS part, "), format!("{e}, "))
    });
    format!(
        "SELECT {sel}{method} AS method, {route} AS route, sum({count}) AS requests, \
         sum(CASE WHEN {status} >= 500 THEN {count} ELSE 0 END) AS errors, \
         CAST(sum({sum}) * 1000 AS BIGINT) AS sum_ms, CAST(max({max}) * 1000 AS BIGINT) AS max_ms, {buckets} \
         FROM raw_events WHERE event = '{event}' AND {w} \
         GROUP BY {group}{method}, {route} ORDER BY requests DESC LIMIT {limit}",
        sum = secs_of("sum"),
        max = secs_of("max"),
        buckets = buckets.join(", "),
        event = openlens_event::names::REQUEST_STATS,
    )
}

fn row_stat(
    r: &serde_json::Map<String, serde_json::Value>,
    keys: &HashMap<String, String>,
) -> Option<RouteStat> {
    let mut s = RouteStat {
        method: text(r, "method")?,
        route: text(r, "route")?,
        requests: int(r, "requests"),
        errors: int(r, "errors"),
        sum_ms: int(r, "sum_ms"),
        max_ms: int(r, "max_ms"),
        buckets: (1..=BUCKETS).map(|i| int(r, &format!("b{i}"))).collect(),
        key_of: None,
        slowest: None,
        first_5xx: None,
    };
    s.key_of = keys.get(&s.name()).cloned();
    Some(s)
}

/// Each route of `environment` over the last `days`, most requests first.
///
/// # Errors
/// A bad environment or a failed query.
pub async fn list(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    days: i64,
    map: Option<&FeatureMap>,
) -> Result<Vec<RouteStat>> {
    let days = days.clamp(1, MAX_DAYS);
    let mut params = openlens_sqlgen::Params::new();
    let env = params
        .set("env", environment)
        .map_err(|_| EngineError::InvalidFilter("environment"))?;
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%d");
    let w = format!(
        "date >= '{since}' AND timestamp >= now() - INTERVAL '{days} days' AND environment = {env}"
    );
    let keys = key_actions(map);
    let qp = q.with_params(&params);
    let (sums, picks) = (
        stats_sql(&w, None, MAX_ROUTES),
        exemplars_sql(&w, None, MAX_ROUTES * 2),
    );
    let (r, ex) = futures::join!(qp.sql(project, &sums), qp.sql(project, &picks));
    let mut out: Vec<RouteStat> = r?.rows.iter().filter_map(|x| row_stat(x, &keys)).collect();
    attach(&mut out, &picked(&ex?.rows), None);
    Ok(out)
}

/// Each release's routes in `environment` since `since` (a `YYYY-MM-DD`
/// day), by release name: what the verdict compares.
///
/// # Errors
/// A bad environment or a failed query.
pub(crate) async fn by_release(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    since: &str,
    map: Option<&FeatureMap>,
) -> Result<HashMap<String, Vec<RouteStat>>> {
    let mut params = openlens_sqlgen::Params::new();
    let env = params
        .set("env", environment)
        .map_err(|_| EngineError::InvalidFilter("environment"))?;
    let w = format!(
        "date >= '{since}' AND environment = {env} AND release IS NOT NULL AND release <> ''"
    );
    let keys = key_actions(map);
    let qp = q.with_params(&params);
    let (sums, picks) = (
        stats_sql(&w, Some("release"), MAX_ROUTES),
        exemplars_sql(&w, Some("release"), MAX_ROUTES * 2),
    );
    let (r, ex) = futures::join!(qp.sql(project, &sums), qp.sql(project, &picks));
    let mut out: HashMap<String, Vec<RouteStat>> = HashMap::new();
    for x in &r?.rows {
        if let (Some(release), Some(s)) = (text(x, "part"), row_stat(x, &keys)) {
            out.entry(release).or_default().push(s);
        }
    }
    let ex = picked(&ex?.rows);
    for (release, list) in &mut out {
        attach(list, &ex, Some(release.as_str()));
    }
    Ok(out)
}

/// The days of the p95 trend (#622): the last [`TREND_DAYS`] UTC days,
/// oldest first, today last.
#[must_use]
pub fn trend_days(now: chrono::DateTime<chrono::Utc>) -> Vec<String> {
    let today = now.date_naive();
    (0..TREND_DAYS)
        .rev()
        .map(|i| {
            (today - chrono::Duration::days(i))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect()
}

/// Each route's p95 on each of `days` (`None` without requests that day),
/// by route name. A day's p95 comes from the merge of that day's
/// histograms, never from an average of p95s.
#[must_use]
pub fn daily_p95(
    rows: &[(String, RouteStat)],
    days: &[String],
) -> HashMap<String, Vec<Option<f64>>> {
    let mut merged: HashMap<(String, usize), RouteStat> = HashMap::new();
    for (day, s) in rows {
        let Some(i) = days.iter().position(|d| d == day) else {
            continue;
        };
        merged
            .entry((s.name(), i))
            .and_modify(|m| m.merge(s))
            .or_insert_with(|| s.clone());
    }
    let mut out: HashMap<String, Vec<Option<f64>>> = HashMap::new();
    for ((name, i), s) in merged {
        let series = out.entry(name).or_insert_with(|| vec![None; days.len()]);
        if let Some(slot) = series.get_mut(i) {
            *slot = (s.buckets.iter().sum::<i64>() > 0).then(|| s.p95());
        }
    }
    out
}

/// The p95 trend of each route of `environment` over [`trend_days`]: one
/// grouped query per day and route, bounded like the list.
///
/// # Errors
/// A bad environment or a failed query.
pub async fn trend(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    days: &[String],
) -> Result<HashMap<String, Vec<Option<f64>>>> {
    let Some(first) = days.first() else {
        return Ok(HashMap::new());
    };
    let mut params = openlens_sqlgen::Params::new();
    let env = params
        .set("env", environment)
        .map_err(|_| EngineError::InvalidFilter("environment"))?;
    let first = chrono::NaiveDate::parse_from_str(first, "%Y-%m-%d")
        .map_err(|_| EngineError::InvalidFilter("date"))?
        .format("%Y-%m-%d");
    let w = format!("date >= '{first}' AND environment = {env}");
    let r = q
        .with_params(&params)
        .sql(
            project,
            &stats_sql(&w, Some("date"), MAX_ROUTES * days.len()),
        )
        .await?;
    let none = HashMap::new();
    let rows: Vec<(String, RouteStat)> = r
        .rows
        .iter()
        .filter_map(|x| Some((text(x, "part")?, row_stat(x, &none)?)))
        .collect();
    Ok(daily_p95(&rows, days))
}

/// The latest release of an environment with route stats and the one
/// before it, each with its routes.
#[derive(Debug, Clone, Default)]
pub struct ReleasePair {
    /// The latest release's name.
    pub current: String,
    /// Its routes.
    pub current_routes: Vec<RouteStat>,
    /// The release before it, with its routes, when there is one.
    pub previous: Option<(String, Vec<RouteStat>)>,
}

/// The two latest releases of `environment` by their first route stats
/// over the releases' window ([`crate::releases::WINDOW_DAYS`]), with
/// their routes; `None` without a release in the stats.
///
/// # Errors
/// A bad environment or a failed query.
pub async fn latest_releases(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    map: Option<&FeatureMap>,
) -> Result<Option<ReleasePair>> {
    let since = (chrono::Utc::now() - chrono::Duration::days(crate::releases::WINDOW_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let mut params = openlens_sqlgen::Params::new();
    let env = params
        .set("env", environment)
        .map_err(|_| EngineError::InvalidFilter("environment"))?;
    let w = format!(
        "event = '{event}' AND date >= '{since}' AND environment = {env} \
         AND release IS NOT NULL AND release <> ''",
        event = openlens_event::names::REQUEST_STATS,
    );
    let r = q
        .with_params(&params)
        .sql(
            project,
            &format!(
                "SELECT release AS part, min(timestamp) AS first_ts FROM raw_events WHERE {w} \
                 GROUP BY release ORDER BY first_ts DESC LIMIT 2"
            ),
        )
        .await?;
    let names: Vec<String> = r.rows.iter().filter_map(|x| text(x, "part")).collect();
    let Some(current) = names.first().cloned() else {
        return Ok(None);
    };
    let previous = names.get(1).cloned();
    let mut list = vec![params.set("cur", &current)];
    if let Some(p) = &previous {
        list.push(params.set("prev", p));
    }
    // A release name the query cannot hold: no comparison.
    let Ok(list) = list.into_iter().collect::<std::result::Result<Vec<_>, _>>() else {
        return Ok(None);
    };
    let w = format!(
        "date >= '{since}' AND environment = {env} AND ({})",
        list.iter()
            .map(|r| format!("release = {r}"))
            .collect::<Vec<_>>()
            .join(" OR ")
    );
    let keys = key_actions(map);
    let r = q
        .with_params(&params)
        .sql(project, &stats_sql(&w, Some("release"), MAX_ROUTES * 2))
        .await?;
    let mut out = ReleasePair {
        current: current.clone(),
        ..ReleasePair::default()
    };
    let mut before = Vec::new();
    for x in &r.rows {
        let (Some(release), Some(s)) = (text(x, "part"), row_stat(x, &keys)) else {
            continue;
        };
        if release == current {
            out.current_routes.push(s);
        } else {
            before.push(s);
        }
    }
    out.previous = previous.map(|p| (p, before));
    Ok(Some(out))
}

/// How one route moved from the previous release to the latest one, by
/// the verdict's rules ([`checks`]).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Change {
    /// 5xx share in the latest release.
    pub error_rate: f64,
    /// 5xx share in the release before.
    pub previous_error_rate: f64,
    /// p95 in the latest release, ms.
    pub p95_ms: f64,
    /// p95 in the release before, ms.
    pub previous_p95_ms: f64,
    /// The 5xx share moved enough to count (worse or better).
    pub errors_moved: bool,
    /// The p95 moved enough to count.
    pub p95_moved: bool,
    /// Worse on either.
    pub worse: bool,
    /// Size of the larger move, in units of its threshold: 1 is a move
    /// just large enough to count.
    pub size: f64,
}

/// Each judged route's [`Change`] between `previous` and `current`, by
/// route name. Routes with too few requests on either side are left out.
#[must_use]
pub fn changes(current: &[RouteStat], previous: &[RouteStat]) -> HashMap<String, Change> {
    let mut out: HashMap<String, Change> = HashMap::new();
    for c in checks(current, previous) {
        let Some((kind, name)) = c.key.split_once(':') else {
            continue;
        };
        let moved = matches!(c.outcome, Outcome::Worse | Outcome::Better);
        let e = out.entry(name.to_owned()).or_default();
        e.worse |= c.outcome == Outcome::Worse;
        let before = c.previous.unwrap_or(0.0);
        if kind == "5xx" {
            e.error_rate = c.current;
            e.previous_error_rate = before;
            e.errors_moved = moved;
        } else {
            e.p95_ms = c.current;
            e.previous_p95_ms = before;
            e.p95_moved = moved;
        }
    }
    for e in out.values_mut() {
        let errors = (e.error_rate - e.previous_error_rate).abs() / MIN_5XX_MOVE;
        let p95 = (e.p95_ms.max(1.0) / e.previous_p95_ms.max(1.0)).ln().abs() / P95_RATIO.ln();
        e.size = errors.max(p95);
    }
    out
}

fn label(s: &RouteStat) -> String {
    match &s.key_of {
        Some(f) => format!("{} (key action of {f})", s.name()),
        None => s.name(),
    }
}

/// The verdict's backend checks: each route with enough requests on both
/// releases, its 5xx share and its p95.
#[must_use]
pub fn checks(current: &[RouteStat], previous: &[RouteStat]) -> Vec<Check> {
    let mut out = Vec::new();
    for c in current {
        let Some(p) = previous
            .iter()
            .find(|p| p.method == c.method && p.route == c.route)
        else {
            continue;
        };
        if c.requests < MIN_REQUESTS || p.requests < MIN_REQUESTS {
            continue;
        }
        let (rc, rp) = (c.error_rate(), p.error_rate());
        let outcome = match stats::two_proportions(rc, c.requests, rp, p.requests) {
            Some(pv) if pv < stats::ALPHA && (rc - rp).abs() >= MIN_5XX_MOVE => {
                if rc > rp {
                    Outcome::Worse
                } else {
                    Outcome::Better
                }
            }
            Some(_) => Outcome::Same,
            None => Outcome::TooFew,
        };
        out.push(Check {
            kind: "route",
            key: format!("5xx:{}", c.name()),
            label: format!("5xx of {}", label(c)),
            previous: Some(rp),
            current: rc,
            outcome,
            judged: true,
            exemplar: None,
            note: format!(
                "5xx of {}: {:.1}% → {:.1}% ({} requests)",
                label(c),
                rp * 100.0,
                rc * 100.0,
                c.requests
            ),
        });
        let (pc, pp) = (c.p95(), p.p95());
        let outcome = if pc >= pp * P95_RATIO && pc - pp >= P95_MIN_MS {
            Outcome::Worse
        } else if pp >= pc * P95_RATIO && pp - pc >= P95_MIN_MS {
            Outcome::Better
        } else {
            Outcome::Same
        };
        out.push(Check {
            kind: "route",
            key: format!("p95:{}", c.name()),
            label: format!("p95 of {}", label(c)),
            previous: Some(pp),
            current: pc,
            outcome,
            judged: true,
            note: format!("p95 of {}: {pp:.0} ms → {pc:.0} ms", label(c)),
            // The request to open for a slower route (#607): shown, the
            // outcome is the histogram's alone.
            exemplar: if outcome == Outcome::Worse {
                c.slowest.clone()
            } else {
                None
            },
        });
    }
    out
}

/// One route failing in the last hour.
#[derive(Debug, Clone, PartialEq)]
pub struct Failing {
    /// `POST /api/login`.
    pub route: String,
    /// The feature whose key action it is.
    pub key_of: Option<String>,
    /// In plain words.
    pub cause: String,
}

/// Requests a route needs in the last hour to raise an alert.
pub const MIN_HOUR_REQUESTS: i64 = 20;

/// Is a route failing now? Its 5xx share over the last hour is at least
/// 10 % and three times its previous seven days plus 5 points, on
/// [`MIN_HOUR_REQUESTS`] requests or more.
#[must_use]
pub fn failing(hour: &RouteStat, week: Option<&RouteStat>) -> Option<Failing> {
    if hour.requests < MIN_HOUR_REQUESTS {
        return None;
    }
    let now = hour.error_rate();
    let before = week.map_or(0.0, RouteStat::error_rate);
    (now >= 0.10 && now >= 3.0 * before + 0.05).then(|| Failing {
        route: hour.name(),
        key_of: hour.key_of.clone(),
        cause: format!(
            "{} answered 5xx to {:.0}% of {} requests in the last hour (usually {:.1}%)",
            label(hour),
            now * 100.0,
            hour.requests,
            before * 100.0
        ),
    })
}

/// The routes of production failing in the last hour (the closed windows
/// that started in the last 70 minutes, whatever their length), against
/// the seven days before.
///
/// # Errors
/// A failed query.
pub async fn failing_routes(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
) -> Result<Vec<Failing>> {
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::days(8)).format("%Y-%m-%d");
    let w = format!("date >= '{since}' AND environment = 'production'");
    // The last hour of closed windows (a window is stamped with its start
    // and sent when it ends, so 70 minutes back for 10-minute windows; with
    // finer ones a few more minutes, a share is a share), and the week
    // before.
    let part = "CASE WHEN timestamp >= now() - INTERVAL '70 minutes' THEN 'hour' ELSE 'week' END";
    let sql = stats_sql(&w, Some(part), MAX_ROUTES);
    let keys = key_actions(map);
    let r = q.sql(project, &sql).await?;
    let mut hour = Vec::new();
    let mut week = Vec::new();
    for x in &r.rows {
        let Some(s) = row_stat(x, &keys) else {
            continue;
        };
        if text(x, "part").as_deref() == Some("hour") {
            hour.push(s);
        } else {
            week.push(s);
        }
    }
    Ok(hour
        .iter()
        .filter_map(|h| {
            failing(
                h,
                week.iter()
                    .find(|w| w.method == h.method && w.route == h.route),
            )
        })
        .collect())
}

#[cfg(test)]
mod tests;

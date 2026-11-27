//! Server routes (#594, D-083): the `$request_stats` the server SDKs send
//! (#593, D-082), one data point of OpenTelemetry's
//! `http.server.request.duration` histogram per (method, route, status
//! code) per 10-minute window (#601, D-084), summed per route.
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

/// The SQL summing the stats per route (and per the `extra` expression,
/// selected as `part`) where `w` holds; each histogram bucket is its own
/// column.
fn stats_sql(w: &str, extra: Option<&str>) -> String {
    // One data point of `http.server.request.duration` (#601): OpenTelemetry
    // semantic-convention attributes, durations in seconds.
    let attr = |k: &str, pat: &str| format!("regexp_match(properties, '\"{k}\":{pat}')[1]");
    let int_of = |k: &str| {
        format!(
            "COALESCE(CAST({} AS BIGINT), 0)",
            attr(k, "([0-9]{1,18})")
        )
    };
    let secs_of = |k: &str| {
        format!(
            "COALESCE(CAST({} AS DOUBLE), 0)",
            attr(k, "([0-9]{1,12}(?:\\.[0-9]{1,9})?(?:[eE][-+]?[0-9]{1,3})?)")
        )
    };
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
         GROUP BY {group}{method}, {route} ORDER BY requests DESC LIMIT {MAX_ROUTES}",
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
    let r = q
        .with_params(&params)
        .sql(project, &stats_sql(&w, None))
        .await?;
    Ok(r.rows.iter().filter_map(|x| row_stat(x, &keys)).collect())
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
    let r = q
        .with_params(&params)
        .sql(project, &stats_sql(&w, Some("release")))
        .await?;
    let mut out: HashMap<String, Vec<RouteStat>> = HashMap::new();
    for x in &r.rows {
        if let (Some(release), Some(s)) = (text(x, "part"), row_stat(x, &keys)) {
            out.entry(release).or_default().push(s);
        }
    }
    Ok(out)
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

/// The routes of production failing in the last hour (closed 10-minute
/// windows), against the seven days before.
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
    // and sent when it ends, so 70 minutes back), and the week before.
    let part = "CASE WHEN timestamp >= now() - INTERVAL '70 minutes' THEN 'hour' ELSE 'week' END";
    let sql = stats_sql(&w, Some(part));
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
mod tests {
    use super::*;

    fn stat(requests: i64, errors: i64, fast: i64, slow_bucket: usize) -> RouteStat {
        let mut buckets = vec![0; BUCKETS];
        buckets[2] = fast;
        buckets[slow_bucket] += requests - fast;
        RouteStat {
            method: "POST".into(),
            route: "/api/login".into(),
            requests,
            errors,
            sum_ms: 0,
            max_ms: 20_000,
            buckets,
            key_of: None,
        }
    }

    #[test]
    fn p95_comes_from_the_histogram_and_never_passes_the_largest_duration() {
        let s = stat(100, 0, 96, 6);
        assert!(s.p95() <= BOUNDS_MS[2] && s.p50() <= s.p95());
        let slow = stat(100, 0, 50, 8);
        assert!(slow.p95() > s.p95());
        let mut tiny = stat(10, 0, 0, BUCKETS - 1);
        tiny.max_ms = 12_000;
        assert!(tiny.p95() <= 12_000.0);
    }

    #[test]
    fn a_route_that_starts_failing_or_slows_down_is_worse_and_too_little_traffic_is_not_judged() {
        let before = [stat(400, 0, 390, 4)];
        let broken = [stat(400, 400, 390, 4)];
        let worse = checks(&broken, &before);
        assert!(
            worse
                .iter()
                .any(|c| c.key.starts_with("5xx:") && c.outcome == Outcome::Worse)
        );
        let slow = [stat(400, 0, 100, 9)];
        assert!(
            checks(&slow, &before)
                .iter()
                .any(|c| c.key.starts_with("p95:") && c.outcome == Outcome::Worse)
        );
        assert!(
            checks(&before, &before)
                .iter()
                .all(|c| c.outcome != Outcome::Worse)
        );
        assert!(checks(&[stat(10, 10, 0, 9)], &before).is_empty());
    }

    #[test]
    fn a_failing_hour_alerts_only_well_above_the_usual_share() {
        let week = stat(10_000, 100, 9_000, 4);
        assert!(failing(&stat(100, 80, 90, 4), Some(&week)).is_some());
        assert!(failing(&stat(100, 2, 90, 4), Some(&week)).is_none());
        assert!(failing(&stat(5, 5, 0, 4), Some(&week)).is_none());
    }

    #[test]
    fn key_actions_match_whatever_the_case_and_spacing() {
        assert_eq!(normalize("post   /api/login"), "POST /api/login");
    }
}

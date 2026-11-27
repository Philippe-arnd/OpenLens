//! Outgoing dependencies (#608, D-090): the `$client_stats` the Node SDK
//! (`captureOutgoing()`) and OTLP (`http.client.request.duration`) send,
//! one data point of OpenTelemetry's `http.client.request.duration`
//! histogram per (method, `server.address`, status code) per 10-minute
//! window, summed per external host. Not an APM: no spans, no paths.
//!
//! - **Calls, failures and duration** per host, the duration as a merged
//!   histogram with the routes' bounds ([`crate::routes::BOUNDS_MS`]): p95
//!   is the upper bound of the bucket that holds it. A call **fails** when
//!   it got a 5xx or no response at all (status 0, with an `error.type`).
//! - **Shown, not judged** ([`changes`]): a host with at least
//!   [`MIN_CALLS`] calls on both releases that fails more (significantly,
//!   by at least [`crate::routes::MIN_5XX_MOVE`]) or got slower (p95
//!   [`crate::routes::P95_RATIO`] times and [`crate::routes::P95_MIN_MS`]
//!   above the one before). It is listed in the verdict's
//!   `dependencies`, never in its checks: a dependency's trouble is
//!   usually not the release's fault, so it never changes the outcome.
//! - **Alert** ([`failing_hosts`]): a host whose share of failed calls in
//!   the latest closed window is at least [`FAILING_SHARE`] and well above
//!   its previous seven days, on [`MIN_WINDOW_CALLS`] calls; announced
//!   once it has lasted [`FAILING_WAIT_SECS`].
//!
//! The stats are read from `raw_events`: the `events` view leaves them
//! out, so no user KPI ever counts them.

use std::collections::HashMap;

use openlens_query::QueryEngine;
use serde::Serialize;

use crate::routes::{BUCKETS, MIN_5XX_MOVE, P95_MIN_MS, P95_RATIO, RouteStat};
use crate::{EngineError, Result, int, stats, text};

/// Calls a host needs on both releases to be compared.
pub const MIN_CALLS: i64 = 50;
/// Hosts read at most (the SDK sends at most 50 per window and process).
pub const MAX_HOSTS: usize = 200;
/// Days `dependencies.list` reads at most.
pub const MAX_DAYS: i64 = 90;
/// Calls a host needs in the latest window to raise an alert.
pub const MIN_WINDOW_CALLS: i64 = 10;
/// Smallest share of failed calls in the latest window that alerts…
pub const FAILING_SHARE: f64 = 0.25;
/// Seconds a host must keep failing before its incident is announced.
pub const FAILING_WAIT_SECS: i64 = 15 * 60;

/// One external host's numbers.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DependencyStat {
    /// `server.address`: the host name only, never a path or a query.
    pub host: String,
    /// Calls.
    pub calls: i64,
    /// Calls that failed: a 5xx or no response.
    pub errors: i64,
    /// Of those, calls that got no response (network error, timeout).
    pub no_response: i64,
    /// Sum of the durations, ms.
    pub sum_ms: i64,
    /// Largest duration, ms.
    pub max_ms: i64,
    /// Calls per duration bucket ([`crate::routes::BOUNDS_MS`]).
    pub buckets: Vec<i64>,
}

impl DependencyStat {
    /// The histogram as a route's, whose quantile rule applies.
    fn histogram(&self) -> RouteStat {
        RouteStat {
            requests: self.calls,
            errors: self.errors,
            sum_ms: self.sum_ms,
            max_ms: self.max_ms,
            buckets: self.buckets.clone(),
            ..RouteStat::default()
        }
    }
    /// Share of calls that failed.
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        self.errors as f64 / self.calls.max(1) as f64
    }
    /// p95, ms: the upper bound of its bucket.
    #[must_use]
    pub fn p95(&self) -> f64 {
        self.histogram().p95()
    }
}

/// The SQL summing the stats per host (and per the `extra` expression,
/// selected as `part`) where `w` holds, `limit` rows at most.
fn stats_sql(w: &str, extra: Option<&str>, limit: usize) -> String {
    // One data point of `http.client.request.duration`: OpenTelemetry
    // semantic-convention attributes, durations in seconds.
    let attr = |k: &str, pat: &str| format!("regexp_match(properties, '\"{k}\":{pat}')[1]");
    let int_of = |k: &str| format!("COALESCE(CAST({} AS BIGINT), 0)", attr(k, "([0-9]{1,18})"));
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
    let host = attr("server\\.address", "\"([A-Za-z0-9._:\\[\\]-]{1,253})\"");
    let status = int_of("http\\.response\\.status_code");
    let count = int_of("count");
    let (sel, group) = extra.map_or((String::new(), String::new()), |e| {
        (format!("{e} AS part, "), format!("{e}, "))
    });
    format!(
        "SELECT {sel}{host} AS host, sum({count}) AS calls, \
         sum(CASE WHEN {status} >= 500 OR {status} < 100 THEN {count} ELSE 0 END) AS errors, \
         sum(CASE WHEN {status} < 100 THEN {count} ELSE 0 END) AS no_response, \
         CAST(sum({sum}) * 1000 AS BIGINT) AS sum_ms, CAST(max({max}) * 1000 AS BIGINT) AS max_ms, {buckets} \
         FROM raw_events WHERE event = '{event}' AND {w} \
         GROUP BY {group}{host} ORDER BY calls DESC LIMIT {limit}",
        sum = secs_of("sum"),
        max = secs_of("max"),
        buckets = buckets.join(", "),
        event = openlens_event::names::CLIENT_STATS,
    )
}

fn row_stat(r: &serde_json::Map<String, serde_json::Value>) -> Option<DependencyStat> {
    Some(DependencyStat {
        host: text(r, "host")?,
        calls: int(r, "calls"),
        errors: int(r, "errors"),
        no_response: int(r, "no_response"),
        sum_ms: int(r, "sum_ms"),
        max_ms: int(r, "max_ms"),
        buckets: (1..=BUCKETS).map(|i| int(r, &format!("b{i}"))).collect(),
    })
}

/// The environment as a query parameter.
fn env_param(params: &mut openlens_sqlgen::Params, environment: &str) -> Result<String> {
    params
        .set("env", environment)
        .map_err(|_| EngineError::InvalidFilter("environment"))
}

/// Each host of `environment` over the last `days`, most calls first.
///
/// # Errors
/// A bad environment or a failed query.
pub async fn list(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    days: i64,
) -> Result<Vec<DependencyStat>> {
    let days = days.clamp(1, MAX_DAYS);
    let mut params = openlens_sqlgen::Params::new();
    let env = env_param(&mut params, environment)?;
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%d");
    let w = format!(
        "date >= '{since}' AND timestamp >= now() - INTERVAL '{days} days' AND environment = {env}"
    );
    let r = q
        .with_params(&params)
        .sql(project, &stats_sql(&w, None, MAX_HOSTS))
        .await?;
    Ok(r.rows.iter().filter_map(row_stat).collect())
}

/// Each release's hosts in `environment` since `since` (a `YYYY-MM-DD`
/// day), by release name: what the verdict shows.
///
/// # Errors
/// A bad environment or a failed query.
pub(crate) async fn by_release(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    since: &str,
) -> Result<HashMap<String, Vec<DependencyStat>>> {
    let mut params = openlens_sqlgen::Params::new();
    let env = env_param(&mut params, environment)?;
    let w = format!(
        "date >= '{since}' AND environment = {env} AND release IS NOT NULL AND release <> ''"
    );
    let r = q
        .with_params(&params)
        .sql(project, &stats_sql(&w, Some("release"), MAX_HOSTS * 4))
        .await?;
    let mut out: HashMap<String, Vec<DependencyStat>> = HashMap::new();
    for x in &r.rows {
        if let (Some(release), Some(s)) = (text(x, "part"), row_stat(x)) {
            out.entry(release).or_default().push(s);
        }
    }
    Ok(out)
}

/// The latest release of an environment with client stats and the one
/// before it, each with its hosts.
#[derive(Debug, Clone, Default)]
pub struct ReleasePair {
    /// The latest release's name.
    pub current: String,
    /// Its hosts.
    pub current_hosts: Vec<DependencyStat>,
    /// The release before it, with its hosts, when there is one.
    pub previous: Option<(String, Vec<DependencyStat>)>,
}

/// The two latest releases of `environment` by their first client stats
/// over the releases' window, with their hosts; `None` without a release
/// in the stats.
///
/// # Errors
/// A bad environment or a failed query.
pub async fn latest_releases(
    q: &QueryEngine,
    project: &str,
    environment: &str,
) -> Result<Option<ReleasePair>> {
    let since = (chrono::Utc::now() - chrono::Duration::days(crate::releases::WINDOW_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let mut params = openlens_sqlgen::Params::new();
    let env = env_param(&mut params, environment)?;
    let w = format!(
        "event = '{event}' AND date >= '{since}' AND environment = {env} \
         AND release IS NOT NULL AND release <> ''",
        event = openlens_event::names::CLIENT_STATS,
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
    let r = q
        .with_params(&params)
        .sql(project, &stats_sql(&w, Some("release"), MAX_HOSTS * 2))
        .await?;
    let mut out = ReleasePair {
        current: current.clone(),
        ..ReleasePair::default()
    };
    let mut before = Vec::new();
    for x in &r.rows {
        let (Some(release), Some(s)) = (text(x, "part"), row_stat(x)) else {
            continue;
        };
        if release == current {
            out.current_hosts.push(s);
        } else {
            before.push(s);
        }
    }
    out.previous = previous.map(|p| (p, before));
    Ok(Some(out))
}

/// How one host moved from the release before to this one. Shown, never
/// judged: it never changes a verdict.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DependencyChange {
    /// The host.
    pub host: String,
    /// Calls on this release.
    pub calls: i64,
    /// Calls on the release before.
    pub previous_calls: i64,
    /// Share of failed calls on this release.
    pub error_rate: f64,
    /// The same, on the release before.
    pub previous_error_rate: f64,
    /// p95 on this release, ms.
    pub p95_ms: f64,
    /// p95 on the release before, ms.
    pub previous_p95_ms: f64,
    /// Fails significantly more than before.
    pub fails_more: bool,
    /// Got slower (p95).
    pub slower: bool,
}

impl DependencyChange {
    /// Fails more or got slower.
    #[must_use]
    pub fn worse(&self) -> bool {
        self.fails_more || self.slower
    }
}

/// Each host with at least [`MIN_CALLS`] calls on both sides, compared,
/// most calls first.
#[must_use]
pub fn changes(current: &[DependencyStat], previous: &[DependencyStat]) -> Vec<DependencyChange> {
    let mut out = Vec::new();
    for c in current {
        let Some(p) = previous.iter().find(|p| p.host == c.host) else {
            continue;
        };
        if c.calls < MIN_CALLS || p.calls < MIN_CALLS {
            continue;
        }
        let (rc, rp) = (c.error_rate(), p.error_rate());
        let fails_more = rc - rp >= MIN_5XX_MOVE
            && stats::two_proportions(rc, c.calls, rp, p.calls).is_some_and(|pv| pv < stats::ALPHA);
        let (pc, pp) = (c.p95(), p.p95());
        out.push(DependencyChange {
            host: c.host.clone(),
            calls: c.calls,
            previous_calls: p.calls,
            error_rate: rc,
            previous_error_rate: rp,
            p95_ms: pc,
            previous_p95_ms: pp,
            fails_more,
            slower: pc >= pp * P95_RATIO && pc - pp >= P95_MIN_MS,
        });
    }
    out.sort_by(|a, b| b.calls.cmp(&a.calls));
    out
}

/// What the verdict shows: the hosts that fail more or got slower than on
/// the release before. Never a check, so never part of the outcome.
#[must_use]
pub fn shown(current: &[DependencyStat], previous: &[DependencyStat]) -> Vec<DependencyChange> {
    changes(current, previous)
        .into_iter()
        .filter(DependencyChange::worse)
        .collect()
}

/// One host failing in the latest closed window.
#[derive(Debug, Clone, PartialEq)]
pub struct Failing {
    /// The host.
    pub host: String,
    /// In plain words.
    pub cause: String,
}

/// Is a host failing now? Its share of failed calls in the latest window
/// is at least [`FAILING_SHARE`] and three times its previous seven days
/// plus 5 points, on [`MIN_WINDOW_CALLS`] calls or more.
#[must_use]
pub fn failing(window: &DependencyStat, week: Option<&DependencyStat>) -> Option<Failing> {
    if window.calls < MIN_WINDOW_CALLS {
        return None;
    }
    let now = window.error_rate();
    let before = week.map_or(0.0, DependencyStat::error_rate);
    (now >= FAILING_SHARE && now >= 3.0 * before + 0.05).then(|| Failing {
        host: window.host.clone(),
        cause: format!(
            "{} failed {:.0}% of {} calls in the last 10 minutes ({} without a response; usually {:.1}%)",
            window.host,
            now * 100.0,
            window.calls,
            window.no_response,
            before * 100.0
        ),
    })
}

/// The incident rule of a failing host: `dep:<host>`.
#[must_use]
pub fn rule(host: &str) -> String {
    format!("dep:{host}")
}

/// The hosts of production failing in the latest closed 10-minute window,
/// against the seven days before.
///
/// # Errors
/// A failed query.
pub async fn failing_hosts(q: &QueryEngine, project: &str) -> Result<Vec<Failing>> {
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::days(8)).format("%Y-%m-%d");
    let w = format!("date >= '{since}' AND environment = 'production'");
    // A window is stamped with its start and sent when it ends: the
    // windows that started in the last 20 minutes are the latest closed
    // one (the open one is not sent yet).
    let part = "CASE WHEN timestamp >= now() - INTERVAL '20 minutes' THEN 'window' ELSE 'week' END";
    let r = q
        .sql(project, &stats_sql(&w, Some(part), MAX_HOSTS * 2))
        .await?;
    let mut window = Vec::new();
    let mut week = Vec::new();
    for x in &r.rows {
        let Some(s) = row_stat(x) else {
            continue;
        };
        if text(x, "part").as_deref() == Some("window") {
            window.push(s);
        } else {
            week.push(s);
        }
    }
    Ok(window
        .iter()
        .filter_map(|h| failing(h, week.iter().find(|w| w.host == h.host)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(host: &str, calls: i64, errors: i64, bucket: usize) -> DependencyStat {
        let mut buckets = vec![0; BUCKETS];
        buckets[bucket] = calls;
        DependencyStat {
            host: host.into(),
            calls,
            errors,
            no_response: 0,
            sum_ms: 0,
            max_ms: 20_000,
            buckets,
        }
    }

    #[test]
    fn a_host_that_fails_more_or_slows_down_is_shown_and_a_quiet_one_is_not() {
        let before = [stat("api.stripe.com", 400, 0, 5)];
        let failing = [stat("api.stripe.com", 400, 80, 5)];
        let slow = [stat("api.stripe.com", 400, 0, 10)];
        assert!(shown(&failing, &before).iter().any(|c| c.fails_more));
        assert!(shown(&slow, &before).iter().any(|c| c.slower));
        assert!(shown(&before, &before).is_empty());
        // Too few calls on one side: not compared.
        assert!(changes(&[stat("api.stripe.com", 10, 10, 10)], &before).is_empty());
    }

    #[test]
    fn a_failing_window_alerts_only_well_above_the_usual_share() {
        let week = stat("api.openai.com", 10_000, 100, 5);
        assert!(failing(&stat("api.openai.com", 40, 30, 5), Some(&week)).is_some());
        assert!(failing(&stat("api.openai.com", 40, 1, 5), Some(&week)).is_none());
        assert!(failing(&stat("api.openai.com", 3, 3, 5), Some(&week)).is_none());
    }
}

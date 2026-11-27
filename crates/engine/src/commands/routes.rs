//! Server routes as a command (#594, D-083): requests, 5xx share and
//! durations per route over a period, from the server SDKs' route stats,
//! each route with its p95 over the last ten days and how it moved since
//! the release before (#622). The rules are in [`crate::routes`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::routes::{self, BUCKETS, Change, MAX_DAYS, RouteStat};

/// The input of `routes.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
    /// Environment (production when absent).
    #[serde(default)]
    pub environment: Option<String>,
    /// Days of the period, 1 to 90 (7 when absent).
    #[serde(default)]
    pub days: Option<i64>,
}

/// One route, as listed.
#[derive(Debug, Clone, Serialize)]
pub struct Route {
    /// The route's numbers.
    #[serde(flatten)]
    pub stat: RouteStat,
    /// Share of requests answered 5xx.
    pub error_rate: f64,
    /// Median duration, ms (upper bound of its histogram bucket).
    pub p50_ms: f64,
    /// 95th percentile, ms (upper bound of its histogram bucket).
    pub p95_ms: f64,
    /// p95 on each of `trend_days`, ms (`None` without requests that day),
    /// each from that day's merged histograms.
    pub trend_p95_ms: Vec<Option<f64>>,
    /// How it moved from `previous_release` to `release`, when both had
    /// enough requests to judge.
    pub change: Option<Change>,
}

/// The routes of one environment.
#[derive(Debug, Clone, Serialize)]
pub struct Routes {
    /// Environment read.
    pub environment: String,
    /// Days read.
    pub days: i64,
    /// Key actions first, then the routes worse than the release before,
    /// then the largest change, then most requests. A key action declared
    /// in the feature map without a request is listed with zeros.
    pub routes: Vec<Route>,
    /// Requests over the period, every route.
    pub requests: i64,
    /// Of those, answered 5xx.
    pub errors: i64,
    /// The latest release with route stats.
    pub release: Option<String>,
    /// The release before it.
    pub previous_release: Option<String>,
    /// Routes worse than the release before.
    pub worse: usize,
    /// Of those, key actions.
    pub key_actions_worse: usize,
    /// The days of `trend_p95_ms` (UTC, oldest first, today last).
    pub trend_days: Vec<String>,
    /// Why the list is empty, when it is.
    pub note: Option<String>,
}

/// The order of [`Routes::routes`]: key actions first, then the routes
/// worse than the release before, then the largest change, then most
/// requests.
pub fn sort(list: &mut [Route]) {
    list.sort_by(|a, b| {
        let key = |r: &Route| {
            (
                r.stat.key_of.is_some(),
                r.change.as_ref().is_some_and(|c| c.worse),
            )
        };
        let size = |r: &Route| r.change.as_ref().map_or(0.0, |c| c.size);
        key(b)
            .cmp(&key(a))
            .then(size(b).total_cmp(&size(a)))
            .then(b.stat.requests.cmp(&a.stat.requests))
    });
}

/// `routes.list`: the project's server routes.
pub struct List;

impl Command for List {
    const NAME: &'static str = "routes.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Routes;

    async fn run<H: Host>(host: &H, _: &Caller, input: ListInput) -> Result<Routes, Error> {
        let days = input.days.unwrap_or(7);
        if !(1..=MAX_DAYS).contains(&days) {
            return Err(Error::Invalid(format!(
                "days must be between 1 and {MAX_DAYS}"
            )));
        }
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let environment = input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .unwrap_or(crate::releases::PRODUCTION)
            .to_owned();
        let map = host.feature_map(&p.id).await;
        let map = map.as_ref().map(|(m, _)| m);
        let trend_days = routes::trend_days(chrono::Utc::now());
        let q = host.query();
        let (list, trend, pair) = futures::join!(
            routes::list(q, &p.id, &environment, days, map),
            routes::trend(q, &p.id, &environment, &trend_days),
            routes::latest_releases(q, &p.id, &environment, map),
        );
        let failed = |e: crate::EngineError| match e {
            crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
            _ => Error::Unavailable(e.to_string()),
        };
        let mut list = list.map_err(failed)?;
        let trend = trend.map_err(failed)?;
        let pair = pair.map_err(failed)?;
        let note = list.is_empty().then(|| {
            "No server route stats yet: add the Node SDK's middleware (`@openlens/node`, see sdk/SERVER.md)."
                .to_owned()
        });
        // Declared key actions without a request still show, as zeros.
        if !list.is_empty() {
            let mut declared: Vec<(String, String)> =
                routes::key_actions(map).into_iter().collect();
            declared.sort();
            for (name, feature) in declared {
                let Some((method, route)) = name.split_once(' ') else {
                    continue;
                };
                if list.iter().any(|s| s.name() == name) || list.len() >= routes::MAX_ROUTES {
                    continue;
                }
                list.push(RouteStat {
                    method: method.to_owned(),
                    route: route.to_owned(),
                    buckets: vec![0; BUCKETS],
                    key_of: Some(feature),
                    ..RouteStat::default()
                });
            }
        }
        let (release, previous_release, changes) = match pair {
            Some(pair) => {
                let changes = pair
                    .previous
                    .as_ref()
                    .map(|(_, before)| routes::changes(&pair.current_routes, before))
                    .unwrap_or_default();
                (Some(pair.current), pair.previous.map(|(p, _)| p), changes)
            }
            None => (None, None, std::collections::HashMap::new()),
        };
        let mut out: Vec<Route> = list
            .into_iter()
            .map(|stat| {
                let name = stat.name();
                Route {
                    error_rate: stat.error_rate(),
                    p50_ms: stat.p50(),
                    p95_ms: stat.p95(),
                    trend_p95_ms: trend
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| vec![None; trend_days.len()]),
                    change: changes.get(&name).cloned(),
                    stat,
                }
            })
            .collect();
        sort(&mut out);
        let worse = |r: &&Route| r.change.as_ref().is_some_and(|c| c.worse);
        Ok(Routes {
            environment,
            days,
            requests: out.iter().map(|r| r.stat.requests).sum(),
            errors: out.iter().map(|r| r.stat.errors).sum(),
            release,
            previous_release,
            worse: out.iter().filter(worse).count(),
            key_actions_worse: out
                .iter()
                .filter(worse)
                .filter(|r| r.stat.key_of.is_some())
                .count(),
            trend_days,
            routes: out,
            note,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str, requests: i64, key: bool, change: Option<(bool, f64)>) -> Route {
        Route {
            stat: RouteStat {
                method: "GET".into(),
                route: name.into(),
                requests,
                buckets: vec![0; BUCKETS],
                key_of: key.then(|| "export".to_owned()),
                ..RouteStat::default()
            },
            error_rate: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            trend_p95_ms: Vec::new(),
            change: change.map(|(worse, size)| Change {
                worse,
                size,
                ..Change::default()
            }),
        }
    }

    #[test]
    fn key_actions_come_first_then_the_worse_then_the_largest_change() {
        let mut list = vec![
            route("/busy", 9_000, false, None),
            route("/moved", 100, false, Some((false, 4.0))),
            route("/broke", 100, false, Some((true, 2.0))),
            route("/key-quiet", 0, true, None),
            route("/key-broke", 50, true, Some((true, 1.5))),
            route("/small-move", 100, false, Some((false, 0.5))),
        ];
        sort(&mut list);
        let order: Vec<&str> = list.iter().map(|r| r.stat.route.as_str()).collect();
        let at = |n: &str| order.iter().position(|x| *x == n).unwrap_or(usize::MAX);
        // Every key action before any other route, a worse one first.
        assert!(at("/key-broke") < at("/key-quiet"));
        assert!(at("/key-quiet") < at("/broke"));
        // Then worse routes, then by size of change, then traffic.
        assert!(at("/broke") < at("/moved"));
        assert!(at("/moved") < at("/small-move"));
        assert!(at("/small-move") < at("/busy"));
    }
}

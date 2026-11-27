//! The backend of a release verdict, ready to show (#623): its **Backend**
//! group (routes worse than the release before, new server error groups,
//! the routes that did not move) and the **routes that got worse**, each
//! with its top server error and the browser sessions that made the
//! request (#593 C, #624).
//!
//! Everything but the errors and sessions comes from the verdict itself
//! ([`crate::verdict::Report`]): the group adds no query, and a release
//! without server stats has no Backend group and keeps its verdict
//! (#594 A). The errors and sessions are one query of the release's server
//! errors and one of their linked sessions, read only when a route got
//! worse.

use std::collections::{BTreeSet, HashMap};

use openlens_query::QueryEngine;
use serde::Serialize;

use crate::board_verdict::{Layer, Row};
use crate::routes::RouteStat;
use crate::verdict::{Check, Outcome, Report, Verdict};
use crate::{EngineError, Result, int, text};

/// Worse routes a group or the table shows at most.
pub const MAX_WORSE: usize = 8;
/// New server errors the group shows at most.
const MAX_NEW_ERRORS: usize = 3;
/// Browser sessions shown per route at most.
pub const MAX_SESSIONS: usize = 3;

/// A route check's kind (`5xx`, `p95`) and route name.
fn route_of(c: &Check) -> Option<(&str, &str)> {
    if c.kind != "route" {
        return None;
    }
    c.key.split_once(':')
}

/// A route check's value as shown.
pub(crate) fn shown(kind: &str, v: f64) -> String {
    if kind == "5xx" {
        format!("{:.1}%", v * 100.0)
    } else {
        format!("{v:.0} ms")
    }
}

fn stat<'a>(list: &'a [RouteStat], name: &str) -> Option<&'a RouteStat> {
    list.iter().find(|s| s.name() == name)
}

/// Whether the route `name` is a key action on this release.
fn key_of(r: &Report, name: &str) -> Option<String> {
    stat(&r.current.routes, name).and_then(|s| s.key_of.clone())
}

/// The routes worse than the release before, key actions first, then by
/// requests.
fn worse_names(r: &Report) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for c in &r.checks {
        if let Some((_, n)) = route_of(c)
            && c.judged
            && c.outcome == Outcome::Worse
            && !names.iter().any(|x| x == n)
        {
            names.push(n.to_owned());
        }
    }
    let requests = |n: &str| stat(&r.current.routes, n).map_or(0, |s| s.requests);
    names.sort_by(|a, b| {
        key_of(r, b)
            .is_some()
            .cmp(&key_of(r, a).is_some())
            .then(requests(b).cmp(&requests(a)))
    });
    names
}

/// The Backend group of a verdict; none when the release sent no server
/// stats and has no new server error.
#[must_use]
pub fn backend_layer(r: &Report) -> Option<Layer> {
    let worse = worse_names(r);
    let mut rows = Vec::new();
    for name in worse.iter().take(MAX_WORSE) {
        let key = key_of(r, name).is_some();
        for c in r.checks.iter().filter(|c| c.outcome == Outcome::Worse) {
            let Some((kind, n)) = route_of(c) else {
                continue;
            };
            if n == name {
                rows.push(Row {
                    label: format!("{name} {kind}"),
                    from: c.previous.map(|p| shown(kind, p)),
                    to: shown(kind, c.current),
                    outcome: Outcome::Worse,
                    key,
                });
            }
        }
    }
    let new_errors: Vec<&Check> = r
        .checks
        .iter()
        .filter(|c| crate::board_verdict::server_error(r, c))
        .collect();
    for c in new_errors.iter().take(MAX_NEW_ERRORS) {
        let sample = r
            .current
            .new_errors
            .iter()
            .find(|e| e.fingerprint == c.key)
            .map(|e| e.sample.chars().take(60).collect::<String>())
            .unwrap_or_default();
        rows.push(Row {
            label: format!("New server error: {sample}"),
            from: None,
            to: format!("{:.0} users", c.current),
            outcome: Outcome::Worse,
            key: false,
        });
    }
    // The other judged routes: better, or unchanged.
    let judged: BTreeSet<&str> = r
        .checks
        .iter()
        .filter_map(route_of)
        .map(|(_, n)| n)
        .filter(|n| !worse.iter().any(|w| w == n))
        .collect();
    let better = judged
        .iter()
        .filter(|n| {
            r.checks
                .iter()
                .any(|c| route_of(c).is_some_and(|x| x.1 == **n) && c.outcome == Outcome::Better)
        })
        .count();
    let routes = |n: usize| if n == 1 { "route" } else { "routes" };
    let other = if worse.is_empty() { "" } else { "other " };
    if better > 0 {
        rows.push(Row {
            label: format!("{better} {other}{}", routes(better)),
            from: None,
            to: "better".to_owned(),
            outcome: Outcome::Better,
            key: false,
        });
    }
    let same = judged.len() - better;
    if same > 0 {
        rows.push(Row {
            label: format!("{same} {other}{}", routes(same)),
            from: None,
            to: "unchanged".to_owned(),
            outcome: Outcome::Same,
            key: false,
        });
    }
    // Stats, but no route with enough requests on both releases.
    if rows.is_empty() && !r.current.routes.is_empty() {
        let n = r.current.routes.len();
        rows.push(Row {
            label: format!("{n} {}", routes(n)),
            from: None,
            to: "too few requests to judge".to_owned(),
            outcome: Outcome::TooFew,
            key: false,
        });
    }
    let more =
        worse.len().saturating_sub(MAX_WORSE) + new_errors.len().saturating_sub(MAX_NEW_ERRORS);
    (!rows.is_empty()).then_some(Layer {
        name: "Backend",
        rows,
        more,
    })
}

/// The top server error of a route on the release.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TopError {
    /// Its group (as on the Errors page).
    pub fingerprint: String,
    /// One real message.
    pub sample: String,
    /// Distinct users of the browser sessions it reached (the `$server`
    /// pseudo user is none).
    pub users: i64,
    /// Occurrences on the release.
    pub count: i64,
}

/// A browser session that made a request to the route: an exemplar's
/// session (#607) or a session linked to its server error (#624). Never
/// its user.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RouteSession {
    /// The browser's session id.
    pub session: String,
    /// When (UTC): where its replay opens.
    pub at: String,
    /// A recording of it exists.
    pub replay: bool,
}

/// A route worse than on the release before.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorseRoute {
    /// `GET`, `POST`…
    pub method: String,
    /// Route template.
    pub route: String,
    /// The feature whose key action it is.
    pub key_of: Option<String>,
    /// Requests on the release.
    pub requests: i64,
    /// Share answered 5xx.
    pub error_rate: f64,
    /// On the release before.
    pub previous_error_rate: Option<f64>,
    /// Its 5xx check got worse.
    pub errors_worse: bool,
    /// p95, ms.
    pub p95_ms: f64,
    /// On the release before.
    pub previous_p95_ms: Option<f64>,
    /// Its p95 check got worse.
    pub p95_worse: bool,
    /// Its most frequent server error on the release, by users.
    pub top_error: Option<TopError>,
    /// Browser sessions that made the request, latest first, at most
    /// [`MAX_SESSIONS`].
    pub sessions: Vec<RouteSession>,
}

/// The routes worse than the release before, at most [`MAX_WORSE`],
/// without their errors and sessions yet.
#[must_use]
pub fn worse_routes(r: &Report) -> Vec<WorseRoute> {
    let previous = r.previous.as_ref().map(|p| p.routes.as_slice());
    worse_names(r)
        .into_iter()
        .take(MAX_WORSE)
        .filter_map(|name| {
            let s = stat(&r.current.routes, &name)?;
            let p = previous.and_then(|l| stat(l, &name));
            let worse = |kind: &str| {
                r.checks.iter().any(|c| {
                    route_of(c) == Some((kind, name.as_str())) && c.outcome == Outcome::Worse
                })
            };
            // The exemplars' sessions first: they made the request.
            let sessions = [&s.first_5xx, &s.slowest]
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    Some(RouteSession {
                        session: e.session_id.clone()?,
                        at: e.time.clone(),
                        replay: false,
                    })
                })
                .collect();
            Some(WorseRoute {
                method: s.method.clone(),
                route: s.route.clone(),
                key_of: s.key_of.clone(),
                requests: s.requests,
                error_rate: s.error_rate(),
                previous_error_rate: p.map(RouteStat::error_rate),
                errors_worse: worse("5xx"),
                p95_ms: s.p95(),
                previous_p95_ms: p.map(RouteStat::p95),
                p95_worse: worse("p95"),
                top_error: None,
                sessions,
            })
        })
        .collect()
}

/// The verdict of the newest judged release, by layer, with the routes
/// that got worse (the Releases page, #623).
#[derive(Debug, Clone, Serialize)]
pub struct LatestVerdict {
    /// The release.
    pub release: String,
    /// Its environment.
    pub environment: String,
    /// The release it is compared with.
    pub previous_release: Option<String>,
    /// Its verdict; none while it is measured.
    pub verdict: Option<Verdict>,
    /// Its checks by layer (Browser, Backend, Features).
    pub layers: Vec<Layer>,
    /// The release sent server route stats.
    pub backend: bool,
    /// Routes worse than the release before.
    pub routes: Vec<WorseRoute>,
    /// Why the routes' errors and sessions are missing, when they could
    /// not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Per route name: its top server error and its linked sessions.
type RouteErrors = HashMap<String, (Option<TopError>, Vec<RouteSession>)>;

/// The server errors of `release` in `environment` per route (their
/// `http.route` and `http.request.method`), with the browser sessions
/// linked to them as on the Errors page (#624), over the releases'
/// window. Bounded: 500 (route, group) pairs, 1000 sessions.
async fn route_errors(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    release: &str,
) -> Result<RouteErrors> {
    let mut params = openlens_sqlgen::Params::new();
    let env = params
        .set("env", environment)
        .map_err(|_| EngineError::InvalidFilter("environment"))?;
    let rel = params
        .set("rel", release)
        .map_err(|_| EngineError::InvalidFilter("release"))?;
    let since = (chrono::Utc::now() - chrono::Duration::days(crate::releases::WINDOW_DAYS))
        .format("%Y-%m-%d");
    let browser = format!("date >= '{since}' AND environment = {env}");
    let w = format!("{browser} AND release = {rel}");
    let method = crate::routes::attr_sql("http\\.request\\.method", "\"([A-Z]{1,10})\"");
    let route = crate::routes::attr_sql("http\\.route", "\"((?:[^\"\\\\]|\\\\.){1,300})\"");
    let base = format!(
        "WITH e AS (SELECT nullif(distinct_id, '{server}') AS uid, session_id, timestamp, \
           error_message AS msg, error_source AS source, {srv} AS srv, {trace} AS trace, \
           {method} AS m, {route} AS r FROM events WHERE {w} AND event = '$error'), \
         g AS (SELECT *, {fp} AS efp, concat(m, ' ', r) AS fp FROM e \
           WHERE srv = 1 AND m IS NOT NULL AND r IS NOT NULL)",
        server = crate::errors::SERVER_ID,
        srv = crate::errors::SERVER_ERROR,
        trace = crate::errors::TRACE_ID,
        fp = crate::errors::FINGERPRINT,
    );
    let top_sql = format!(
        "{base} SELECT fp, efp, min(msg) AS sample, count(*) AS n, count(DISTINCT uid) AS users \
         FROM g GROUP BY fp, efp ORDER BY users DESC, n DESC LIMIT 500"
    );
    let linked_sql = crate::errors::linked_sql(&base, "events", &browser);
    let qp = q.with_params(&params);
    let (top, linked) = futures::join!(qp.sql(project, &top_sql), qp.sql(project, &linked_sql));
    let mut out: RouteErrors = HashMap::new();
    for x in &top?.rows {
        let Some(fp) = text(x, "fp") else { continue };
        let e = out.entry(fp).or_default();
        if e.0.is_none() {
            e.0 = Some(TopError {
                fingerprint: text(x, "efp").unwrap_or_default(),
                sample: text(x, "sample").unwrap_or_else(|| "(no message)".into()),
                users: int(x, "users"),
                count: int(x, "n"),
            });
        }
    }
    for x in &linked?.rows {
        let (Some(fp), Some(session)) = (text(x, "fp"), text(x, "sid")) else {
            continue;
        };
        let e = out.entry(fp).or_default();
        if e.1.len() < MAX_SESSIONS {
            e.1.push(RouteSession {
                session,
                at: text(x, "seen").unwrap_or_default(),
                replay: false,
            });
        }
    }
    Ok(out)
}

/// [`LatestVerdict`] of the report `r` of `environment`: its sessions
/// marked when a recording of them is kept under `data_dir`.
pub async fn latest(
    q: &QueryEngine,
    data_dir: &std::path::Path,
    project: &str,
    environment: &str,
    r: &Report,
) -> LatestVerdict {
    let mut routes = worse_routes(r);
    let mut note = None;
    if !routes.is_empty() {
        match route_errors(q, project, environment, &r.release).await {
            Ok(found) => {
                for w in &mut routes {
                    if let Some((top, linked)) = found.get(&format!("{} {}", w.method, w.route)) {
                        w.top_error.clone_from(top);
                        for s in linked {
                            if !w.sessions.iter().any(|x| x.session == s.session) {
                                w.sessions.push(s.clone());
                            }
                        }
                    }
                }
            }
            Err(e) => {
                note = Some(format!(
                    "The server errors and sessions behind these routes could not be read now: {e}"
                ));
            }
        }
        for w in &mut routes {
            w.sessions.truncate(MAX_SESSIONS);
            for s in &mut w.sessions {
                s.replay = openlens_replay::exists(data_dir, project, &s.session);
            }
        }
    }
    LatestVerdict {
        release: r.release.clone(),
        environment: environment.to_owned(),
        previous_release: r.previous_release.clone(),
        verdict: r.verdict,
        layers: crate::board_verdict::layers(r),
        backend: !r.current.routes.is_empty(),
        routes,
        note,
    }
}

#[cfg(test)]
mod tests;

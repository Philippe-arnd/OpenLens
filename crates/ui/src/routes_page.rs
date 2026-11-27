//! Quality › Routes (#622): the server routes of `routes.list`, with the
//! period's totals, how each route moved since the release before and a
//! ten-day p95 line. Without route stats, how to get them.

use openlens_engine::commands::routes::{Route, Routes};
use openlens_engine::routes::Exemplar;

use super::*;
use crate::tables::Col;

/// Milliseconds as shown.
pub(crate) fn ms(v: f64) -> String {
    format!("{v:.0} ms")
}

/// A route's p95 over the trend days as a line; days without requests are
/// skipped. Its size is in SVG attributes (the CSP forbids `style`).
fn trend(series: &[Option<f64>], days: &[String]) -> Markup {
    let pts: Vec<(usize, f64)> = series
        .iter()
        .enumerate()
        .filter_map(|(i, v)| Some((i, (*v)?)))
        .collect();
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
        return html! { span.muted { "—" } };
    };
    if pts.len() < 2 {
        return html! { span.muted { "—" } };
    }
    let max = pts.iter().map(|p| p.1).fold(1.0, f64::max);
    let step = 72.0 / (series.len().max(2) - 1) as f64;
    let points: Vec<String> = pts
        .iter()
        .map(|(i, v)| format!("{:.1},{:.1}", *i as f64 * step, 18.0 - v / max * 16.0))
        .collect();
    let day = |i: usize| days.get(i).map_or("", String::as_str);
    let label = format!(
        "p95 per day: {} on {}, {} on {}, highest {}",
        ms(first.1),
        day(first.0),
        ms(last.1),
        day(last.0),
        ms(max)
    );
    html! {
        svg width="72" height="20" viewBox="0 0 72 20" role="img" aria-label=(label) focusable="false" {
            title { (label) }
            polyline points=(points.join(" ")) fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round" {}
        }
    }
}

/// A rate or a duration, marked when it got worse since the release
/// before, with the earlier value when it moved enough to count.
pub(crate) fn moved(
    now: String,
    worse: bool,
    was: Option<String>,
    previous: Option<&str>,
) -> Markup {
    html! {
        @if worse { span.status.bad { (icon("alert")) (now) } } @else { (now) }
        @if let Some(w) = was {
            " " span.muted.small { "was " (w) @if let Some(p) = previous { " in " (p) } }
        }
    }
}

/// One real request behind a route's numbers (#607, #629): its duration
/// and time, then its session's replay at that time when it was recorded,
/// else its trace id as text to copy, else its session, not linked.
/// Nothing else of the request: no body, header, query string or user.
pub(crate) fn exemplar(pid: &str, e: &Exemplar, tz: &str) -> Markup {
    html! {
        (ms(e.ms)) " " span.muted.small { "at " (local_time(&e.time, tz, true)) }
        @match (&e.session_id, &e.trace_id) {
            (Some(s), _) if e.replay => {
                br;
                a.small href={ "/p/" (pid) "/replays/" (url_enc(s)) "?at=" (url_enc(&e.time)) } title="Replay the session that made this request, at that moment" {
                    (icon("play")) " Replay"
                }
            }
            (_, Some(t)) => { br; code.small title="Trace id: select it to copy" { (t) } }
            (Some(s), None) => { br; span.small.mono title="Session not recorded" { (s.get(..8).unwrap_or(s)) } }
            (None, None) => {}
        }
    }
}

fn row(pid: &str, tz: &str, x: &Route, previous: Option<&str>, days: &[String]) -> Markup {
    let s = &x.stat;
    let c = x.change.as_ref();
    let errors = c.filter(|c| c.errors_moved);
    let p95 = c.filter(|c| c.p95_moved);
    html! {
        tr {
            th scope="row" {
                span.tag.mono { (s.method) } " " code { (s.route) }
                @if let Some(k) = &s.key_of {
                    " " span.tag.tone-accent title={ "Key action of " (k) } { "Key action" }
                }
            }
            td.num { (compact(s.requests)) }
            @if s.requests == 0 {
                td.num.muted { "—" } td.num.muted { "—" } td.num.muted { "—" } td.muted { "—" } td { span.muted { "—" } }
            } @else {
                td.num { (moved(
                    pct(x.error_rate),
                    errors.is_some_and(|c| c.error_rate > c.previous_error_rate),
                    errors.map(|c| pct(c.previous_error_rate)),
                    previous,
                ))
                    @if let Some(e) = s.first_5xx.as_ref().filter(|_| s.errors > 0) {
                        br; span.muted.small { "first: " } (exemplar(pid, e, tz))
                    }
                }
                td.num { (ms(x.p50_ms)) }
                td.num { (moved(
                    ms(x.p95_ms),
                    p95.is_some_and(|c| c.p95_ms > c.previous_p95_ms),
                    p95.map(|c| ms(c.previous_p95_ms)),
                    previous,
                )) }
                @match &s.slowest {
                    Some(e) => td { (exemplar(pid, e, tz)) },
                    None => td.muted title="No request of this route carried an exemplar in the period" { "—" },
                }
                td { (trend(&x.trend_p95_ms, days)) }
            }
        }
    }
}

/// What a project without route stats sees: how to send them.
fn empty(pid: &str) -> Markup {
    html! {
        section.card.empty aria-labelledby="no-routes" {
            svg.art viewBox="0 0 96 72" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true" focusable="false" {
                rect x="8" y="14" width="80" height="16" rx="4" opacity=".5" {}
                rect x="8" y="40" width="80" height="16" rx="4" opacity=".3" {}
                circle cx="20" cy="22" r="2" fill="currentColor" {}
                circle cx="20" cy="48" r="2" fill="currentColor" opacity=".5" {}
            }
            div {
                h2 id="no-routes" { "No server routes yet" }
                p {
                    "Routes come from your backend. Install " code { "@openlens/node" }
                    " and mount its Express middleware before your routes: every route template then gets its requests, "
                    "its share answered 5xx and its p95 here, sent as one small summary per route every 10 minutes, never one call per request. "
                    "Release verdicts start judging the backend, and the key actions declared in the feature map are flagged when they degrade."
                }
                p { "Another Node framework can record each request itself with " code { "recordRequest" } ". The other server SDKs do not send route stats yet." }
                pre { code { "import { OpenLens } from \"@openlens/node\";\nconst ol = new OpenLens({ host, key });\napp.use(ol.express()); // first, before the routes" } }
                div.actions { a.btn.sm href={ "/p/" (pid) "/settings/install" } { "Install guide" } }
            }
        }
    }
}

/// Quality › Routes (#622).
pub fn routes_page(p: &ProjectInfo<'_>, b: &Board, f: &Filters, r: &Routes) -> Markup {
    let previous = r.previous_release.as_deref();
    layout_in(
        "Routes",
        Some(p),
        context_bar(p.id, b, f, "/routes", false),
        html! {
            div.heading {
                div {
                    h1 { "Routes" }
                    p.muted {
                        "Server requests per route in " strong { (r.environment) } ", last " (r.days) " days, from the route stats your server SDK sends every 10 minutes. "
                        "They carry no user, so only the environment applies. "
                        "When your server SDK sends exemplars, the slowest request and the first 5xx open the replay of the session that made them, or show their trace id."
                    }
                }
            }
            @if r.routes.is_empty() {
                (empty(p.id))
            } @else {
                p {
                    strong { (compact(r.requests)) } " requests · "
                    strong { (pct(r.errors as f64 / r.requests.max(1) as f64)) } " answered 5xx · "
                    @match previous {
                        Some(prev) => {
                            strong { (r.worse) } " " (if r.worse == 1 { "route" } else { "routes" }) " worse than " (prev) " · "
                            strong { (r.key_actions_worse) } " " (if r.key_actions_worse == 1 { "key action" } else { "key actions" }) " degraded"
                        }
                        None => span.muted { "no earlier release with route stats to compare with" },
                    }
                }
                (crate::tables::TableCard {
                    label: "Routes",
                    head: html! {},
                    cols: &[
                        Col::Shown("Route"), Col::Num("Requests"), Col::Num("5xx"), Col::Num("p50"),
                        Col::Num("p95"), Col::Shown("Slowest"), Col::Shown("p95, 10 days"),
                    ],
                    body: html! { @for x in &r.routes { (row(p.id, &f.tz, x, previous, &r.trend_days)) } },
                    foot: html! { div.pager {
                        span { (r.routes.len()) " routes · key actions first, then the largest change"
                            @if let Some(prev) = previous { " since " (prev) } }
                        a.btn.sm href={ "/p/" (p.id) "/features" } { "Declare key actions" }
                    } },
                }
                .render())
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::routes::{BUCKETS, Exemplar, RouteStat};

    fn info() -> ProjectInfo<'static> {
        ProjectInfo {
            id: "app",
            name: "App",
            key: "k",
        }
    }

    fn routes(list: Vec<Route>) -> Routes {
        Routes {
            environment: "production".into(),
            days: 10,
            requests: list.iter().map(|r| r.stat.requests).sum(),
            errors: 0,
            release: None,
            previous_release: None,
            worse: 0,
            key_actions_worse: 0,
            trend_days: Vec::new(),
            routes: list,
            note: None,
        }
    }

    #[test]
    fn without_route_stats_the_page_explains_the_node_sdk_instead_of_zeros() {
        let html = routes_page(
            &info(),
            &Board::default(),
            &Filters::default(),
            &routes(Vec::new()),
        )
        .into_string();
        assert!(html.contains("@openlens/node") && !html.contains("<table"));
    }

    #[test]
    fn a_route_draws_its_trend_and_names_its_key_action() {
        let days: Vec<String> = (1..=4).map(|d| format!("2026-11-0{d}")).collect();
        let route = Route {
            stat: RouteStat {
                method: "POST".into(),
                route: "/api/login".into(),
                requests: 40,
                buckets: vec![0; BUCKETS],
                key_of: Some("sign-in".into()),
                ..RouteStat::default()
            },
            error_rate: 0.0,
            p50_ms: 10.0,
            p95_ms: 50.0,
            trend_p95_ms: vec![Some(25.0), None, Some(50.0), Some(250.0)],
            change: None,
        };
        let mut r = routes(vec![route]);
        r.trend_days = days;
        let html = routes_page(&info(), &Board::default(), &Filters::default(), &r).into_string();
        assert!(html.contains("<polyline") && html.contains("Key action"));
        assert!(!html.contains("style="), "sizes are SVG attributes (CSP)");
    }

    fn route(path: &str, errors: i64, slowest: Option<Exemplar>, first: Option<Exemplar>) -> Route {
        Route {
            stat: RouteStat {
                method: "GET".into(),
                route: path.into(),
                requests: 40,
                errors,
                buckets: vec![0; BUCKETS],
                slowest,
                first_5xx: first,
                ..RouteStat::default()
            },
            error_rate: errors as f64 / 40.0,
            p50_ms: 10.0,
            p95_ms: 50.0,
            trend_p95_ms: Vec::new(),
            change: None,
        }
    }

    fn page(list: Vec<Route>) -> String {
        routes_page(
            &info(),
            &Board::default(),
            &Filters::default(),
            &routes(list),
        )
        .into_string()
    }

    /// The `<tr>` of the route `path`.
    fn row_of<'a>(html: &'a str, path: &str) -> &'a str {
        html.split("<tr>")
            .find(|r| r.contains(&format!("<code>{path}</code>")))
            .and_then(|r| r.split("</tr>").next())
            .unwrap_or_default()
    }

    /// #629: a slow request from a recorded session opens that replay at
    /// its time; one from a session without a recording shows its trace id
    /// instead, never a replay link.
    #[test]
    fn a_recorded_slow_request_opens_its_replay_at_its_time_otherwise_its_trace_id() {
        let time = "2026-11-20T10:02:03.000Z";
        let ex = |session: &str, replay: bool| Exemplar {
            ms: 2400.0,
            time: time.into(),
            status: 200,
            trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".into()),
            session_id: Some(session.into()),
            replay,
        };
        let html = page(vec![
            route("/recorded", 0, Some(ex("rec-session", true)), None),
            route("/plain", 0, Some(ex("plain-session", false)), None),
        ]);
        let at = format!("/p/app/replays/rec-session?at={}", url_enc(time));
        assert!(row_of(&html, "/recorded").contains(&at), "{html}");
        let plain = row_of(&html, "/plain");
        assert!(
            !plain.contains("/replays/") && plain.contains("4bf92f3577b34da6a3ce929d0e0e4736"),
            "{plain}"
        );
        assert!(
            !crate::tests::has_iso_time(&html),
            "times as the reader reads them: {html}"
        );
    }

    /// #629: the first 5xx shows in the 5xx cell of a route that answered
    /// any; a route without exemplars shows no link and no made-up value.
    #[test]
    fn the_first_5xx_shows_only_where_one_was_kept_and_no_exemplar_means_no_link() {
        let first = Exemplar {
            ms: 30.0,
            time: "2026-11-20T10:00:00.000Z".into(),
            status: 503,
            session_id: Some("fail-session".into()),
            replay: true,
            ..Exemplar::default()
        };
        let html = page(vec![
            route("/failing", 4, None, Some(first)),
            route("/bare", 4, None, None),
        ]);
        let failing = row_of(&html, "/failing");
        assert!(
            failing.contains("first:") && failing.contains("/replays/fail-session?at="),
            "{failing}"
        );
        let bare = row_of(&html, "/bare");
        assert!(!bare.is_empty(), "{html}");
        assert!(
            !bare.contains("<a") && !bare.contains("first:") && !bare.contains(" at "),
            "nothing made up: {bare}"
        );
    }
}

//! Quality › Routes (#622): the server routes of `routes.list`, with the
//! period's totals, how each route moved since the release before and a
//! ten-day p95 line. Without route stats, how to get them.

use openlens_engine::commands::routes::{Route, Routes};

use super::*;
use crate::tables::Col;

/// Milliseconds as shown.
fn ms(v: f64) -> String {
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
fn moved(now: String, worse: bool, was: Option<String>, previous: Option<&str>) -> Markup {
    html! {
        @if worse { span.status.bad { (icon("alert")) (now) } } @else { (now) }
        @if let Some(w) = was {
            " " span.muted.small { "was " (w) @if let Some(p) = previous { " in " (p) } }
        }
    }
}

fn row(x: &Route, previous: Option<&str>, days: &[String]) -> Markup {
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
                td.num.muted { "—" } td.num.muted { "—" } td.num.muted { "—" } td { span.muted { "—" } }
            } @else {
                td.num { (moved(
                    pct(x.error_rate),
                    errors.is_some_and(|c| c.error_rate > c.previous_error_rate),
                    errors.map(|c| pct(c.previous_error_rate)),
                    previous,
                )) }
                td.num { (ms(x.p50_ms)) }
                td.num { (moved(
                    ms(x.p95_ms),
                    p95.is_some_and(|c| c.p95_ms > c.previous_p95_ms),
                    p95.map(|c| ms(c.previous_p95_ms)),
                    previous,
                )) }
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
                        "They carry no user, so only the environment applies."
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
                        Col::Num("p95"), Col::Shown("p95, 10 days"),
                    ],
                    body: html! { @for x in &r.routes { (row(x, previous, &r.trend_days)) } },
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
    use openlens_engine::routes::{BUCKETS, RouteStat};

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
}

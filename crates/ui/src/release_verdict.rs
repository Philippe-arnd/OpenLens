//! A release's verdict by layer (#615, #623): the checks of Browser,
//! Backend and Features, each with its mark and how it moved, on the
//! board's lead and on the Releases page; and on the Releases page, the
//! routes that got worse with their top server error and the browser
//! sessions that made the request (#593 C).

use super::*;
use crate::routes_page::{exemplar, moved, ms};
use crate::tables::Col;
use openlens_engine::board_verdict::Layer;
use openlens_engine::verdict::Outcome;
use openlens_engine::verdict_backend::{LatestVerdict, WorseRoute};

/// A check's mark: its tone, glyph and the words a screen reader says.
fn mark(o: Outcome) -> (&'static str, Markup, &'static str) {
    match o {
        Outcome::Worse => ("bad", html! { "!" }, "worse"),
        Outcome::Better => ("good", icon("task"), "better"),
        Outcome::Same => ("muted", html! { "=" }, "no change that counts"),
        Outcome::TooFew => ("muted", html! { "?" }, "too few to say"),
        Outcome::Info => ("muted", html! { "·" }, "shown, not judged"),
    }
}

/// A verdict's checks by layer: each check with its mark, a *Key* tag on
/// a key action's route, and how it moved.
pub(crate) fn layers(layers: &[Layer]) -> Markup {
    html! {
        @if !layers.is_empty() {
            div.layers {
                @for g in layers {
                    div.layer {
                        h3 { (icon(match g.name { "Features" => "layers", "Backend" => "code", _ => "globe" })) (g.name) }
                        @for r in &g.rows {
                            @let (tone, glyph, says) = mark(r.outcome);
                            div class={ "check tone-" (tone) } {
                                span.mark role="img" aria-label=(says) { (glyph) }
                                span { (r.label) @if r.key { " " span.tag.tone-accent title="A key action in the feature map" { "Key" } } }
                                span.fromto {
                                    @if let Some(p) = &r.from { (p) " → " }
                                    @if r.outcome == Outcome::Worse { b { (r.to) } } @else { (r.to) }
                                }
                            }
                        }
                        @if g.more > 0 { p.muted.small { (g.more) " more in the full verdict" } }
                    }
                }
            }
        }
    }
}

/// One route that got worse.
fn route_row(pid: &str, tz: &str, x: &WorseRoute, previous: Option<&str>) -> Markup {
    html! {
        tr {
            th scope="row" {
                span.tag.mono { (x.method) } " " code { (x.route) }
                @if let Some(k) = &x.key_of {
                    " " span.tag.tone-accent title={ "Key action of " (k) } { "Key action" }
                }
            }
            td.num { (compact(x.requests)) }
            td.num { (moved(pct(x.error_rate), x.errors_worse, x.previous_error_rate.map(pct), previous)) }
            td.num {
                (moved(ms(x.p95_ms), x.p95_worse, x.previous_p95_ms.map(ms), previous))
                // Its slowest request, the reason of its p95 check (#629).
                @if let Some(e) = &x.slowest { br; span.muted.small { "slowest: " } (exemplar(pid, e, tz)) }
            }
            td {
                @match &x.top_error {
                    Some(e) => {
                        code { (e.sample.chars().take(80).collect::<String>()) }
                        span.muted.small { " · " (e.count) " times" @if e.users > 0 { ", " (e.users) " users" } }
                    }
                    None => span.muted { "—" },
                }
            }
            td {
                @if x.sessions.is_empty() {
                    span.muted { "—" }
                } @else {
                    @for (i, s) in x.sessions.iter().enumerate() {
                        @if i > 0 { ", " }
                        @let short = s.session.get(..8).unwrap_or(&s.session);
                        @if s.replay {
                            a.small href={ "/p/" (pid) "/replays/" (url_enc(&s.session)) "?at=" (url_enc(&s.at)) } title="Replay this session" {
                                (icon("play")) " " (short)
                            }
                        } @else {
                            span.small.mono title="Not recorded" { (short) }
                        }
                    }
                }
            }
        }
    }
}

/// The newest judged release's verdict by layer and the routes that got
/// worse, times in `tz`. Without server stats, one line says the backend
/// is not judged.
pub(crate) fn latest(pid: &str, l: &LatestVerdict, tz: &str) -> Markup {
    let previous = l.previous_release.as_deref();
    html! {
        section.card.wide aria-labelledby="ver-h" {
            h2 id="ver-h" { "Verdict of " code { (l.release) } }
            p.muted.small {
                @match (l.verdict, previous) {
                    (Some(v), Some(p)) => { (v.label()) ", compared with " code { (p) } " in " (l.environment) }
                    (Some(v), None) => { (v.label()) " in " (l.environment) }
                    (None, _) => { "Still measured in " (l.environment) }
                }
            }
            (layers(&l.layers))
            // Runtime (#631): its line, or how to get one.
            @match &l.runtime {
                Some(rt) => (crate::release_runtime::line(rt, previous)),
                None => (crate::release_runtime::missing()),
            }
            @if !l.backend {
                @if l.runtime.is_some() {
                    p.muted.small { "No server route stats for this release: its routes are not judged. The Node SDK sends them (Quality › Routes)." }
                } @else {
                    p.muted.small { "No server route stats for this release: its backend is not judged. The Node SDK sends them (Quality › Routes)." }
                }
            }
            @if let Some(n) = &l.note { p.muted.small { (n) } }
        }
        @if !l.routes.is_empty() {
            (crate::tables::TableCard {
                label: "Routes that got worse",
                head: html! {
                    h2 { "Routes that got worse" }
                    a href={ "/p/" (pid) "/routes?env=" (url_enc(&l.environment)) } { "All routes" }
                },
                cols: &[
                    Col::Shown("Route"), Col::Num("Requests"), Col::Num("5xx"), Col::Num("p95"),
                    Col::Shown("Top server error"), Col::Shown("Sessions"),
                ],
                body: html! { @for x in &l.routes { (route_row(pid, tz, x, previous)) } },
                foot: html! {},
            }
            .render())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::board_verdict::Row;
    use openlens_engine::routes::Exemplar;
    use openlens_engine::verdict::Verdict;
    use openlens_engine::verdict_backend::{RouteSession, TopError};

    fn verdict(backend: bool, routes: Vec<WorseRoute>) -> LatestVerdict {
        let mut layers = vec![Layer {
            name: "Browser",
            rows: vec![Row {
                label: "Sessions with errors".into(),
                from: Some("2.0%".into()),
                to: "2.1%".into(),
                outcome: Outcome::Same,
                key: false,
            }],
            more: 0,
        }];
        if backend {
            layers.push(Layer {
                name: "Backend",
                rows: vec![
                    Row {
                        label: "POST /api/auth/sign-in 5xx".into(),
                        from: Some("0.0%".into()),
                        to: "100.0%".into(),
                        outcome: Outcome::Worse,
                        key: true,
                    },
                    Row {
                        label: "4 other routes".into(),
                        from: None,
                        to: "unchanged".into(),
                        outcome: Outcome::Same,
                        key: false,
                    },
                ],
                more: 0,
            });
        }
        LatestVerdict {
            release: "v2".into(),
            environment: "production".into(),
            previous_release: Some("v1".into()),
            verdict: Some(if backend {
                Verdict::Regressed
            } else {
                Verdict::Healthy
            }),
            layers,
            backend,
            routes,
            runtime: None,
            note: None,
        }
    }

    fn sign_in() -> WorseRoute {
        WorseRoute {
            method: "POST".into(),
            route: "/api/auth/sign-in".into(),
            key_of: Some("sign-in".into()),
            requests: 300,
            error_rate: 1.0,
            previous_error_rate: Some(0.0),
            errors_worse: true,
            p95_ms: 50.0,
            previous_p95_ms: Some(50.0),
            p95_worse: false,
            top_error: Some(TopError {
                fingerprint: "Error: pool @ db.js".into(),
                sample: "Error: pool <exhausted>".into(),
                users: 3,
                count: 9,
            }),
            sessions: vec![
                RouteSession {
                    session: "recorded-session".into(),
                    at: "2026-11-20T10:00:00Z".into(),
                    replay: true,
                },
                RouteSession {
                    session: "plain-session".into(),
                    at: "2026-11-20T10:01:00Z".into(),
                    replay: false,
                },
            ],
            slowest: None,
        }
    }

    #[test]
    fn the_releases_verdict_has_a_backend_group_and_the_routes_that_got_worse() {
        let html = latest("p1", &verdict(true, vec![sign_in()]), "UTC").into_string();
        let backend = html
            .split("<h3>")
            .find(|g| g.contains("Backend"))
            .unwrap_or_default();
        assert!(
            backend.contains("/api/auth/sign-in") && backend.contains(">Key<"),
            "{backend}"
        );
        assert!(backend.contains("4 other routes") && backend.contains("unchanged"));
        let table = &html[html.find("Routes that got worse").unwrap_or(0)..];
        assert!(
            table.contains("Error: pool &lt;exhausted&gt;"),
            "escaped: {table}"
        );
        // A recorded session opens its replay at the time; another is named only.
        assert!(
            table.contains("/p/p1/replays/recorded-session?at="),
            "{table}"
        );
        assert!(!table.contains("/replays/plain-session"), "{table}");
        assert!(!html.contains("style=") && !html.contains("<script"));
    }

    #[test]
    fn without_server_stats_one_line_says_so_and_no_backend_or_table_shows() {
        let html = latest("p1", &verdict(false, Vec::new()), "UTC").into_string();
        assert!(html.contains("backend is not judged"), "{html}");
        assert!(!html.contains("Backend") && !html.contains("Routes that got worse"));
        assert!(!html.contains("5xx"), "no empty numbers: {html}");
    }

    /// #629: a slower route names its slowest request, which opens the
    /// replay at its time when recorded; a route that only fails more
    /// names none.
    #[test]
    fn a_slower_route_names_its_slowest_request_opening_its_replay() {
        let time = "2026-11-20T10:02:03.000Z";
        let mut slow = sign_in();
        slow.route = "/api/export".into();
        slow.p95_worse = true;
        slow.sessions.clear();
        slow.slowest = Some(Exemplar {
            ms: 2400.0,
            time: time.into(),
            session_id: Some("slow-session".into()),
            replay: true,
            ..Exemplar::default()
        });
        let html = latest("p1", &verdict(true, vec![slow, sign_in()]), "UTC").into_string();
        let table = &html[html.find("Routes that got worse").unwrap_or(0)..];
        let rows: Vec<&str> = table.split("<tr>").collect();
        let row = |r: &str| {
            rows.iter()
                .find(|x| x.contains(r))
                .copied()
                .unwrap_or_default()
        };
        let at = format!("/p/p1/replays/slow-session?at={}", url_enc(time));
        assert!(row("/api/export").contains(&at), "{table}");
        assert!(!row("/api/auth/sign-in").contains("slowest"), "{table}");
        assert!(!crate::tests::has_iso_time(&html), "{html}");
    }
}

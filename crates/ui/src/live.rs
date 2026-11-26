//! The Live page (#228, #544): who is using the app right now, and one
//! explained empty state when nothing happened in the last 30 minutes.

use super::*;
use crate::tables::{EventCols, empty_art, event_table};

/// Right now (#228): who is using the app, refreshed every 10 seconds by
/// the browser (a `Refresh` header: no script).
pub fn live(p: &ProjectInfo<'_>, f: &Filters, l: &openlens_engine::Live) -> Markup {
    // Quiet: nobody in the last 30 minutes. The latest events still cover
    // the last 24 hours, so the newest one says since when.
    let quiet = l.users_30m == 0;
    let list = |title: &str, glyph: &str, rows: &[(String, i64)]| {
        html! {
            @if !rows.is_empty() {
                section.card {
                    h2.violet { (icon(glyph)) (title) }
                    table.plain { tbody { @for (v, n) in rows {
                        tr { th scope="row" { span.mono { (v) } } td.num { (compact(*n)) } }
                    } } }
                }
            }
        }
    };
    let refresh = html! {
        span.signal.refresh {
            span class=(if quiet { "live-dot off" } else { "live-dot" }) aria-hidden="true" {}
            "Refreshes every 10 s"
        }
    };
    let since = l
        .events
        .first()
        .map(|e| local_time(&e.timestamp, &f.tz, false));
    layout_in(
        "Live",
        Some(p),
        refresh,
        html! {
            div.heading { div {
                h1 { "Live" }
                p.muted { "Last 30 minutes in " strong { (f.environment) } ". Events show up within the flush delay (a few seconds)." }
            } }
            section.tiles aria-label="Right now" {
                // Each opens its detail with the filters kept (#409).
                (tile_to(&detail_url(p.id, "/events", f), "violet", "users", "Active now", &compact(l.users_5m), "users, last 5 minutes"))
                (tile_to(&detail_url(p.id, "/events", f), "sky", "clock", "Last 30 minutes", &compact(l.users_30m), "users"))
                (tile_to(&detail_url(p.id, "/errors", f), "coral", "alert", "Errors", &compact(l.errors_30m), "last 30 minutes"))
            }
            @if quiet {
                section.card.empty aria-labelledby="quiet-h" {
                    (empty_art())
                    div {
                        h2 id="quiet-h" { @match &since {
                            Some(t) => { "Quiet: no event since " (t) " (" (tz_label(&f.tz)) ")" },
                            None => "Quiet: no event in the last 24 hours",
                        } }
                        p { "Nobody used the app in " strong { (f.environment) } " in the last 30 minutes. Releases, roles, features and pages show up here as soon as traffic comes back." }
                        div.actions {
                            a.button href={ "/p/" (p.id) "/uptime" } { "Open Uptime" }
                            a.button href={ "/p/" (p.id) "/settings/install" } { "Check the install" }
                        }
                    }
                }
            } @else {
                div.grid {
                    (list("Releases", "release", &l.releases))
                    (list("Roles", "users", &l.roles))
                    (list("Features", "star", &l.features))
                    (list("Pages", "route", &l.pages))
                }
            }
            @if !l.events.is_empty() {
                (event_table(
                    p.id,
                    "Latest events",
                    html! { h2 { (icon("events")) "Latest events" } a href=(detail_url(p.id, "/events", f)) { "All events" } },
                    &l.events,
                    &f.tz,
                    EventCols { user: true, role: false, groups: true },
                    html! {},
                ))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn p() -> ProjectInfo<'static> {
        ProjectInfo {
            id: "d",
            name: "D",
            key: "k",
        }
    }

    fn f() -> Filters {
        Filters {
            environment: "production".into(),
            tz: "UTC".into(),
            ..Filters::default()
        }
    }

    fn ev(ts: &str) -> EventRow {
        EventRow {
            timestamp: ts.into(),
            event: "$pageview".into(),
            distinct_id: "u1".into(),
            role: None,
            path: Some("/".into()),
            feature: None,
            release: None,
            session_id: None,
        }
    }

    #[test]
    fn a_quiet_live_page_shows_one_explained_empty_state() {
        let quiet = openlens_engine::Live {
            events: vec![ev("2026-11-26 07:54:00")],
            ..openlens_engine::Live::default()
        };
        let html = render(live(&p(), &f(), &quiet));
        assert_eq!(html.matches("class=\"card empty\"").count(), 1);
        assert!(html.contains("live-dot off"));
        assert!(
            html.contains("2026-11-26 07:54"),
            "the reason says since when"
        );
        assert!(html.contains("/p/d/uptime") && html.contains("/p/d/settings/install"));
        assert!(!html.contains("Nothing in the last 30 minutes"));
        assert!(!html.contains("style=") && !html.contains("<script"));
        // Traffic: no empty state, and the dot is on.
        let busy = openlens_engine::Live {
            users_5m: 1,
            users_30m: 1,
            pages: vec![("/".into(), 1)],
            events: vec![ev("2026-11-26 07:54:00")],
            ..openlens_engine::Live::default()
        };
        let html = render(live(&p(), &f(), &busy));
        assert!(!html.contains("card empty") && !html.contains("live-dot off"));
        assert!(html.contains("class=\"live-dot\""));
    }
}

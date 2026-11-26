//! The Releases page (#500, D-049): each release per environment, with its
//! state, when it was seen, its users, its time in staging and a link to
//! the board filtered on it.

use super::*;
use crate::components::detail_url;
use openlens_engine::releases::{Releases, State};

/// A duration in seconds, short: `3 d 4 h`, `5 h 10 min`, `12 min`.
fn duration(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{} min", m.max(1))
    }
}

fn state_class(s: State) -> &'static str {
    match s {
        State::Live => "status good",
        State::RollingOut => "status warn",
        State::RolledBack => "status bad",
        State::Superseded => "status muted",
    }
}

/// Releases (#500): `releases.list`, as a table.
pub fn releases(p: &ProjectInfo<'_>, r: &Releases) -> Markup {
    layout(
        "Releases",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Releases" }
                p.muted { "Last " (r.days) " days, times in " (r.timezone) ". A release's share is its part of the environment's distinct users per day. "
                    strong { "Live" } ": at least 95 % on the latest day. "
                    strong { "Rolling out" } ": below that, not overtaken. "
                    strong { "Superseded" } ": a newer release took the majority. "
                    strong { "Rolled back" } ": it had the majority, then an older release took it back. "
                    "Time in staging runs from the build's first event in staging to its first in production." }
            } }
            section.card.wide {
                @if r.releases.is_empty() {
                    p.muted { "No release seen yet. Pass a release to the SDK, or let the browser SDK name builds on its own (D-044)." }
                } @else {
                    div.table-scroll { table.plain {
                        thead { tr {
                            th scope="col" { "Release" } th scope="col" { "Environment" } th scope="col" { "State" }
                            th.num scope="col" { "Share" } th scope="col" { "First seen" } th scope="col" { "Last seen" }
                            th.num scope="col" { "Users" } th scope="col" { "Time in staging" } th scope="col" { "Board" }
                        } }
                        tbody { @for x in &r.releases {
                            tr {
                                td { code { (x.release) } }
                                td { (x.environment) }
                                td { span class=(state_class(x.state)) { (x.state.label()) } }
                                td.num { (pct(x.share)) }
                                td.small.mono { (x.first_seen) }
                                td.small.mono { (x.last_seen) }
                                td.num { (x.users) }
                                td.small {
                                    @match (x.staging_secs, &x.staged_as) {
                                        (Some(s), Some(n)) if *n != x.release => { (duration(s)) span.muted { " as " code { (n) } } }
                                        (Some(s), _) => { (duration(s)) }
                                        _ => { span.muted { "—" } }
                                    }
                                }
                                td.small { a href=(detail_url(p.id, "", &Filters {
                                    environment: x.environment.clone(),
                                    release: Some(x.release.clone()),
                                    ..Filters::default()
                                })) { "Board" } }
                            }
                        } }
                    } }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::releases::Release;

    #[test]
    fn each_release_links_to_the_board_filtered_on_it() {
        let p = ProjectInfo {
            id: "demo",
            name: "Demo",
            key: "k",
        };
        let row = Release {
            release: "v2 <b>".into(),
            environment: "production".into(),
            state: State::RolledBack,
            share: 0.1,
            first_seen: "2026-11-20 10:00".into(),
            last_seen: "2026-11-21 10:00".into(),
            users: 8,
            events: 8,
            staging_secs: Some(90_000),
            staged_as: Some("2026-11-19.1".into()),
        };
        let r = Releases {
            days: 90,
            timezone: "UTC".into(),
            measure: "distinct users per day",
            releases: vec![row],
        };
        let html = releases(&p, &r).into_string();
        assert!(
            html.contains("/p/demo?env=production&amp;release=v2%20%3Cb%3E"),
            "{html}"
        );
        assert!(html.contains("rolled back"));
        assert!(html.contains("1 d 1 h"));
        assert!(!html.contains("v2 <b>"), "escaped");
    }
}

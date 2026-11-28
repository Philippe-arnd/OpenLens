//! The portfolio (#717, D-118): one row per project the reader may see,
//! the ones that need attention first. Each row shows the board's
//! Outcomes in compact form, the newest production release and its
//! verdict, goals at risk or off track, open incidents and the top
//! insight card, read from the board as last computed. A project whose
//! board is not kept yet says *computing* (with the last compute time,
//! D-073) instead of numbers.

use openlens_engine::commands::portfolio::{Portfolio, ReleaseLine, Row};

use crate::tables::{Col, TableCard};
use crate::*;

/// How old a board is, in words.
fn age(secs: u64) -> String {
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", secs / 60),
        _ => format!("{} h ago", secs / 3600),
    }
}

/// A share, or a dash when there is nothing to count.
fn share(v: Option<f64>) -> Markup {
    html! { @match v { Some(v) => (pct(v)), None => span.muted { "—" } } }
}

/// A verdict's tag classes and icon, as on the Releases page.
fn verdict_look(verdict: &str) -> (&'static str, Option<&'static str>) {
    match verdict {
        "healthy" => ("tag tone-good", Some("task")),
        "regressed" => ("tag tone-bad", Some("down")),
        _ => ("tag", None),
    }
}

fn release(pid: &str, r: Option<&ReleaseLine>) -> Markup {
    let Some(r) = r else {
        return html! { span.muted { "—" } };
    };
    let (class, glyph) = verdict_look(&r.verdict);
    html! {
        @if let Some(name) = &r.release { a href={ "/p/" (pid) "/releases" } { code { (name) } } " " }
        span class=(class) { @if let Some(g) = glyph { (icon(g)) } (r.verdict) }
        @if let Some(why) = &r.reason { br; span.muted.small { (why) } }
    }
}

fn goals(pid: &str, r: &Row) -> Markup {
    html! {
        @if r.goals_off_track + r.goals_at_risk == 0 {
            span.muted { "None at risk" }
        } @else {
            a href={ "/p/" (pid) "/goals" } {
                @if r.goals_off_track > 0 { span.tag.tone-bad { (icon("alert")) (r.goals_off_track) " off track" } " " }
                @if r.goals_at_risk > 0 { span.tag.tone-warn { (r.goals_at_risk) " at risk" } }
            }
            @if let Some((line, status)) = &r.worst_goal { br; span.muted.small { (line) " (" (status) ")" } }
        }
    }
}

fn row(r: &Row) -> Markup {
    let pid = r.project.as_str();
    html! {
        tr {
            th scope="row" {
                a href=(r.board_url) { (r.name) }
                @if let Some(s) = r.board_age_secs { br; span.muted.small { "Board " (age(s)) } }
            }
            @match &r.outcomes {
                Some(o) => {
                    td.num { (compact(o.reach)) }
                    td.num { @match o.value { Some(v) => (format!("{v:.2}")), None => span.muted { "—" } } }
                    td.num { (share(o.retention)) }
                    td.num { (share(o.quality)) }
                    td.num { @match o.speed_ms { Some(ms) => { (format!("{:.1}", ms / 1000.0)) " s" }, None => span.muted { "—" } } }
                }
                None => td colspan="5" {
                    span.tag { (icon("clock")) "Computing" }
                    " "
                    span.muted.small {
                        "Its board is not ready yet"
                        @if let Some(s) = r.last_compute_secs { " (about " (s) " s last time)" }
                        ". Open it to see it now."
                    }
                },
            }
            td.small { (release(pid, r.latest_release.as_ref())) }
            td.small { (goals(pid, r)) }
            td.num {
                @if r.open_incidents == 0 { span.muted { "0" } }
                @else { a href={ "/p/" (pid) "/uptime" } { span.tag.tone-bad { (r.open_incidents) " open" } } }
            }
            td.small {
                @match &r.top_insight {
                    Some(t) => a href=(r.board_url) { (t) },
                    None => span.muted { "—" },
                }
            }
        }
    }
}

/// The portfolio page.
pub fn portfolio(p: &Portfolio) -> Markup {
    layout(
        "Portfolio",
        None,
        html! {
            div.heading { div {
                h1 { "Portfolio" }
                p.muted { "Every project you can open, the ones that need you first: a regressed release, goals off track, open incidents, then the busiest. The numbers are each board's Outcomes as last computed; nothing is computed for this page." }
            } }
            @if p.projects.is_empty() {
                section.card.empty { div {
                    h2 { "No project to show" }
                    p { "You do not have access to any project yet. Ask an admin." }
                } }
            } @else {
                (TableCard {
                    label: "Projects",
                    head: html! {},
                    cols: &[
                        Col::Shown("Project"), Col::Num("Reach"), Col::Num("Value"), Col::Num("Retention"),
                        Col::Num("Quality"), Col::Num("Speed"), Col::Shown("Latest release"), Col::Shown("Goals"),
                        Col::Num("Incidents"), Col::Shown("Top action"),
                    ],
                    body: html! { @for r in &p.projects { (row(r)) } },
                    foot: html! {},
                }.render())
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::commands::portfolio::Outcomes;

    fn r(name: &str, kept: bool) -> Row {
        Row {
            project: name.into(),
            name: name.into(),
            board_url: format!("/p/{name}"),
            computing: !kept,
            board_age_secs: kept.then_some(120),
            last_compute_secs: Some(3),
            outcomes: kept.then(|| Outcomes {
                reach: 42,
                ..Outcomes::default()
            }),
            latest_release: None,
            goals_at_risk: 0,
            goals_off_track: 0,
            worst_goal: None,
            open_incidents: 0,
            top_insight: kept.then(|| "Exports keep people".to_owned()),
        }
    }

    #[test]
    fn a_project_without_a_kept_board_says_computing_instead_of_numbers() {
        let html = portfolio(&Portfolio {
            projects: vec![r("shop", true), r("blog", false)],
        })
        .into_string();
        let blog = &html[html.find("/p/blog").unwrap()..];
        assert!(blog.contains("Computing") && blog.contains("about 3 s"));
        let shop = &html[html.find("/p/shop").unwrap()..html.find("/p/blog").unwrap()];
        assert!(!shop.contains("Computing") && shop.contains("Exports keep people"));
    }
}

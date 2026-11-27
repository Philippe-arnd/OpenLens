//! The Focus board (#543: the v2 layout) and its sections.
//!
//! `now`: the status strip, "Production right now" (#614); `focus`: what
//! to focus on (the lead insight, the ranked list); `tiles`: the key
//! numbers; `charts`: active users per day, the feature pulse, the
//! retention line, who uses what and the release comparison;
//! `sections`: the other cards (Web Vitals, revenue, role gaps, funnels,
//! flags, paths to value), closed disclosures after the release
//! comparison (#588).

use crate::*;

mod charts;
mod focus;
mod lead;
mod now;
mod sections;
mod tiles;

pub(crate) use charts::{bar_with, barcell, tenth};
pub(crate) use focus::{Side, compare};
// The board's deltas, also on the Releases page (#549).
pub(crate) use tiles::{Kpi, delta, delta_mark, kpi_tile};

/// The board page before its content (#301): the layout and a waiting
/// panel, sent while a cold board is computed. `wait`: the seconds the
/// board took to compute last time, when known (D-073). The content
/// follows the panel in the same `<main>`, and the panel hides itself then
/// (`.board-waiting:not(:last-child)` in `app.css`; no script).
pub fn board_shell(p: &ProjectInfo<'_>, wait: Option<u64>) -> Markup {
    let wait = match wait {
        Some(s) => format!("This takes about {s} s."),
        None => "This takes a few seconds.".to_owned(),
    };
    layout(
        "Focus board",
        Some(p),
        html! {
            div.board-waiting role="status" aria-live="polite" {
                span.spinner aria-hidden="true" {}
                div {
                    strong { "Computing the board…" }
                    span.muted { (wait) }
                    span.waiting-bar aria-hidden="true" { span {} }
                }
            }
        },
    )
}

/// The Focus board (#543, #588): the page head, the status strip
/// (#614), what to focus on, the key numbers, active users and the
/// feature pulse side by side, the retention line (#619) and the release
/// comparison (closed), then the other cards as closed disclosures. `plain`: optional plain-language `(title, next step)` per
/// insight (same order), shown instead of the original, which stays as
/// the tooltip.
pub fn board(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    host: &str,
    plain: &[(String, String)],
    revenue: Option<&openlens_engine::Revenue>,
    marks: &[Mark],
) -> Markup {
    if b.total_events == 0 {
        return layout("Focus board", Some(p), empty(p, host));
    }
    let scope = match (&f.role, &f.release) {
        (Some(r), Some(v)) => format!("{r} · {v}"),
        (Some(r), None) => r.clone(),
        (None, Some(v)) => v.clone(),
        (None, None) => "everyone".into(),
    };
    layout_in(
        "Focus board",
        Some(p),
        context_bar(p.id, b, f, "", true),
        html! {
            div.board {
                div.page-head {
                    div {
                        h1 { "Focus board" }
                        (subtitle(b, f, &scope))
                        @if let Some(e) = b.window.as_ref().and_then(|w| w.estimate.as_ref()) {
                            (crate::cost::cost_line(e))
                        }
                        (crate::cost::unavailable_note(&b.unavailable_days))
                    }
                    @if !shared() {
                        div.actions {
                            a.btn href={ "/p/" (p.id) "/digest" } { "This week vs last" }
                            a.btn href={ "/p/" (p.id) "/breakdown?by=role&env=" (url_enc(&f.environment)) } { "Break down" }
                        }
                    }
                }
                (now::strip(p.id, f, b))
                (focus::focus(p.id, f, b, plain))
                (tiles::key_numbers(p.id, f, b))
                div.split {
                    (charts::active_users(p.id, f, b, marks))
                    (charts::feature_pulse(p.id, f, b))
                }
                (charts::retention_line(p.id, f, b))
                (charts::releases(p.id, f, b))
                (sections::others(p.id, f, b, revenue))
                (charts::role_matrix(p.id, f, b))
                (sections::funnels(p.id, f, b))
                (sections::flags(p.id, f, b))
                (sections::journeys(p.id, f, b))
                p.muted.small.center {
                    @match b.map_version {
                        Some(v) => { "Feature map v" (v) " in use. Cards compare users who stayed with users who churned; each shows the cohort sizes behind it." }
                        None => { "No feature map yet: roles are raw and features come only from data-feature. Ask your agent to push openlens.features.yaml for sharper insights." }
                    }
                }
            }
        },
    )
}

/// What a period block covers, in a sentence: the release window's label
/// (D-055), or the block's fixed window `fixed` ("last 7 days").
pub(crate) fn period(b: &Board, fixed: &str) -> String {
    let Some(w) = &b.window else {
        return fixed.to_owned();
    };
    let mut c = w.label.chars();
    c.next()
        .map_or_else(String::new, |first| first.to_lowercase().chain(c).collect())
}

/// The window's length, as "the 4 days before" says it.
pub(crate) fn days_before(w: &openlens_engine::BoardWindow) -> String {
    if w.days == 1 {
        "day".to_owned()
    } else {
        format!("{} days", w.days)
    }
}

/// The page head's one line (#588): the window and what it is compared
/// with (D-055), or what the board reads without one. The numbers are in
/// the key numbers and the active users card, not repeated here.
fn subtitle(b: &Board, f: &Filters, scope: &str) -> Markup {
    match &b.window {
        Some(w) => html! {
            p {
                (w.label) " in " strong { (f.environment) } ", compared with the " (days_before(w)) " before."
                @if scope != "everyone" { " Only " strong { (scope) } "." }
            }
        },
        None => html! {
            p { "What " (scope) " did " (period(b, "lately")) " in " strong { (f.environment) } "." }
        },
    }
}

/// The window's users against the same length before it (D-055), for
/// the active users card: "144 users, -8% against the 10 days before".
pub(crate) fn window_users(w: &openlens_engine::BoardWindow) -> String {
    let (now, before) = (w.current.users, w.previous.users);
    let change = if before == 0 {
        if now == 0 { "no change" } else { "new" }.to_owned()
    } else {
        let d = (now - before) as f64 / before as f64 * 100.0;
        format!("{}{d:.0}%", if d >= 0.0 { "+" } else { "" })
    };
    format!(
        "{} users, {change} against the {} before",
        compact(now),
        days_before(w)
    )
}

#[cfg(test)]
mod tests;

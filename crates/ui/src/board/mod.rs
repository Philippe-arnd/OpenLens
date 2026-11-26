//! The Focus board (#543: the v2 layout) and its sections.
//!
//! `focus`: what to focus on (the lead insight, the ranked list);
//! `tiles`: the key numbers; `charts`: active users per day, the feature
//! pulse, the cohorts, who uses what and the release comparison;
//! `sections`: the other cards (Web Vitals, revenue, role gaps, funnels,
//! flags, paths to value).

use crate::*;

mod charts;
mod focus;
mod sections;
mod tiles;

// The board's deltas, also on the Releases page (#549).
pub(crate) use tiles::{delta, delta_mark};

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

/// The Focus board (#543): the signals, the page head, what to focus on,
/// the key numbers, active users and the feature pulse side by side, the
/// other cards, then the cohorts and the release comparison as
/// disclosures. `plain`: optional plain-language `(title, next step)` per
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
                @if !shared() { (signals(&board_signals(p.id, b))) }
                div.page-head {
                    div {
                        h1 { "Focus board" }
                        p { "What " (scope) " did " (period(b, "lately")) " in " strong { (f.environment) } "." }
                        @if let Some(w) = &b.window {
                            (window_line(w))
                            @if let Some(e) = &w.estimate { (crate::cost::cost_line(e)) }
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
                (focus::focus(p.id, f, b, plain))
                (tiles::key_numbers(p.id, f, b))
                div.split {
                    (charts::active_users(p.id, f, b, marks))
                    (charts::feature_pulse(p.id, f, b))
                }
                (sections::others(p.id, f, b, revenue))
                (charts::role_matrix(p.id, f, b))
                (sections::funnels(p.id, f, b))
                (sections::flags(p.id, f, b))
                (sections::journeys(p.id, f, b))
                (charts::cohorts(p.id, f, b))
                (charts::releases(p.id, f, b))
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

/// What the board flags (#540): the app is down, objectives at risk, the
/// setup left to do; one pill each on the signals line.
fn board_signals(pid: &str, b: &Board) -> Vec<Signal> {
    let mut out = Vec::new();
    if let Some(d) = &b.app_down {
        out.push(Signal {
            tone: "bad",
            text: d.clone(),
            progress: None,
            href: format!("/p/{pid}/uptime"),
            link: "Uptime",
        });
    }
    if !b.objectives_at_risk.is_empty() {
        out.push(Signal {
            tone: "warn",
            text: format!("Objectives at risk: {}", b.objectives_at_risk.join(" · ")),
            progress: None,
            href: format!("/p/{pid}/goals"),
            link: "Goals",
        });
    }
    if let Some((done, total)) = b.setup {
        out.push(Signal {
            tone: "todo",
            text: format!("Setup: {done} of {total} required steps done"),
            progress: Some((done, total)),
            href: format!("/p/{pid}/settings"),
            link: "Finish in Settings",
        });
    }
    out
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

/// The release window against the same length before it (D-055): users,
/// sessions and sessions with errors.
pub(crate) fn window_line(w: &openlens_engine::BoardWindow) -> Markup {
    let change = |now: i64, before: i64| {
        if before == 0 {
            return if now == 0 { "no change" } else { "new" }.to_owned();
        }
        let d = (now - before) as f64 / before as f64 * 100.0;
        format!("{}{d:.0}%", if d >= 0.0 { "+" } else { "" })
    };
    let (now, before) = (&w.current, &w.previous);
    html! {
        p.muted.small.window-line {
            strong { (w.label) } ", against the " (days_before(w)) " before: "
            (compact(now.users)) " users (" (change(now.users, before.users)) "), "
            (compact(now.sessions.count)) " sessions (" (change(now.sessions.count, before.sessions.count)) "), "
            (pct(now.sessions.error_rate)) " of sessions with errors (was " (pct(before.sessions.error_rate)) ")."
        }
    }
}

#[cfg(test)]
mod tests;

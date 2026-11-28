//! Next actions (#715, D-114): the findings of the board, the errors, the
//! goals and the feature map, ranked in one list with where each stands
//! (new, proposed by an agent, approved, shipped, learned). An admin
//! approves an action, which files its ticket, or dismisses it for 30
//! days.

use crate::*;
use openlens_engine::commands::actions::{Action, NextActions, State};

/// The tag tone of a state.
fn tone(s: State) -> &'static str {
    match s {
        State::New => "info",
        State::Proposed => "warn",
        State::Approved | State::Shipped => "good",
        State::Learned => "accent",
        State::Dismissed => "muted",
    }
}

/// A link's label: the page it opens.
fn label(link: &str) -> &str {
    let path = link.split('?').next().unwrap_or(link);
    match path.rsplit('/').next().unwrap_or("") {
        "errors" => "Errors",
        "goals" => "Goals",
        "features" => "Feature map",
        "releases" => "Releases",
        "replays" => "Replays",
        "tickets" => "Tickets",
        _ => "Focus board",
    }
}

fn row(pid: &str, a: &Action, admin: bool, github: bool) -> Markup {
    let base = format!("/p/{pid}/actions/{}", a.id);
    html! {
        tr {
            th scope="row" {
                (a.finding.title)
                @for e in &a.finding.evidence { br; span.muted.small { (e) } }
                @if let Some((day, note)) = &a.proposed {
                    br; span.small { "Proposed by an agent on " (day) @if !note.is_empty() { ": " (note) } }
                }
                @if let Some(md) = &a.draft {
                    details { summary.small { "Drafted ticket" } pre.mono.small { (md) } }
                }
                br;
                span.small {
                    @for (i, l) in a.finding.links.iter().enumerate() {
                        @if i > 0 { " · " }
                        a href=(l) { (label(l)) }
                    }
                    @if let Some(u) = a.ticket.as_ref().and_then(|t| t.url.as_ref()) {
                        " · " a href=(u) { "Issue" }
                    }
                }
            }
            td { span.tag.tone-info { (a.finding.kind) } }
            td {
                span class={ "tag tone-" (tone(a.state)) } { (a.state.word()) }
                @if let Some(rel) = a.ticket.as_ref().and_then(|t| t.shipped_in.as_ref()) { br; span.muted.small { "in " code { (rel) } } }
                @if let Some((w, _)) = a.ticket.as_ref().and_then(|t| t.outcome.as_ref()) { br; span.muted.small { (w) } }
            }
            td.num { (format!("{:.2}", a.rank)) }
            @if admin {
                td.small {
                    @if matches!(a.state, State::New | State::Proposed) {
                        @if github {
                            form method="post" action={ (base) "/approve" } {
                                button.btn.primary type="submit" { "Approve and file the ticket" }
                            }
                        } @else {
                            span.muted { "Set the GitHub repository on " a href={ "/p/" (pid) "/tickets" } { "Tickets" } " to approve." }
                        }
                    }
                    form method="post" action={ (base) "/dismiss" } {
                        label.sr-only for={ "why-" (a.id) } { "Why dismiss it" }
                        input id={ "why-" (a.id) } type="text" name="reason" required maxlength="500" placeholder="Why (hidden 30 days)";
                        button.btn type="submit" { "Dismiss" }
                    }
                }
            }
        }
    }
}

/// The page: the ranked actions, with Approve and Dismiss for admins
/// (`github`: a repository and a token are set, so approving can file).
pub fn next_actions(p: &ProjectInfo<'_>, v: &NextActions, admin: bool, github: bool) -> Markup {
    let cols: Vec<crate::tables::Col<'_>> = [
        crate::tables::Col::Shown("Action"),
        crate::tables::Col::Shown("From"),
        crate::tables::Col::Shown("State"),
        crate::tables::Col::Num("Rank"),
    ]
    .into_iter()
    .chain(admin.then_some(crate::tables::Col::Shown("Decide")))
    .collect();
    layout(
        "Next actions",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Next actions" }
                p.muted { "What to work on next: the board's findings, the errors, the goals at risk and the feature map's errors in one list, ranked by impact × confidence. An agent proposes to take one (its ticket is drafted, nothing is posted); an admin approves it, which files the ticket; OpenLens then follows it to the release that ships it and to its measured effect." }
            } }
            @if !v.unavailable.is_empty() {
                p.muted.small { "Not read now, so missing from the list: " (v.unavailable.join("; ")) }
            }
            @if v.actions.is_empty() {
                section.card.empty aria-labelledby="no-actions" {
                    (crate::tables::empty_art())
                    div {
                        h2 #no-actions { "Nothing to act on" }
                        p { "No finding on the board, the errors, the goals or the feature map right now"
                            @if v.dismissed > 0 { " (" (v.dismissed) " dismissed for now)" } "." }
                        div.actions {
                            a.btn.primary href={ "/p/" (p.id) } { "Open the Focus board" }
                            a.btn href={ "/p/" (p.id) "/tickets" } { "Tickets" }
                        }
                    }
                }
            } @else {
                (crate::tables::TableCard {
                    label: "Next actions",
                    head: html! { h2 { (icon("task")) "Ranked" } },
                    cols: &cols,
                    body: html! { @for a in &v.actions { (row(p.id, a, admin, github)) } },
                    foot: html! {
                        @if v.more > 0 || v.dismissed > 0 {
                            p.muted.small {
                                @if v.more > 0 { (v.more) " more after the first 20. " }
                                @if v.dismissed > 0 { (v.dismissed) " dismissed for now." }
                            }
                        }
                    },
                }
                .render())
            }
        },
    )
}

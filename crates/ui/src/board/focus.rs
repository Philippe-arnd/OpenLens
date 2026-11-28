//! What to focus on (#543): the first insight as the lead card (rank,
//! tag, evidence, a before/after comparison when the board holds both
//! values, the next step, one primary action), the others as a ranked
//! list (rank, title, tag and evidence, links; #588: no next step there).
//! A regressed latest release leads instead, as its verdict (#615, `lead`),
//! and the list shows four rows, the rest behind "Show all". On a share
//! link, the actions that need a login are left out.

use crate::*;
use openlens_engine::Insight;

/// The tone, icon and word of an insight kind. The tag always says the
/// word, so colour is never the only cue.
pub(super) fn tone(kind: &str) -> (&'static str, &'static str, &'static str) {
    match kind {
        "release_regression" | "flag_regression" => ("bad", "alert", "Regression"),
        "churn_point" => ("warn", "down", "Churn"),
        "friction" => ("warn", "funnel", "Friction"),
        "funnel_drop" => ("warn", "funnel", "Drop-off"),
        "stick_driver" => ("good", "star", "Retention"),
        "value_moment" => ("good", "spark", "Value moment"),
        "conversion_driver" => ("good", "up", "Conversion"),
        "flag_win" => ("good", "up", "Flag win"),
        "role_blind_spot" => ("info", "shield", "Adoption"),
        "slow_pages" => ("accent", "bolt", "Performance"),
        _ => ("info", "idea", "Insight"),
    }
}

fn tag(kind: &str) -> Markup {
    let (tone, glyph, word) = tone(kind);
    html! { span class={ "tag tone-" (tone) } { (icon(glyph)) (word) } }
}

/// Where an insight's ticket is drafted.
pub(super) fn ticket(pid: &str, c: &Insight) -> String {
    format!(
        "/p/{pid}/tickets/new?kind=insight&subject={}",
        url_enc(&format!("{}:{}", c.kind, c.subject))
    )
}

/// The pages an insight points at, besides its ticket: (href, label).
fn links(pid: &str, f: &Filters, b: &Board, c: &Insight) -> Vec<(String, String)> {
    let s = &c.subject;
    match c.kind {
        "release_regression" => {
            let on = Filters {
                release: Some(s.clone()),
                ..f.clone()
            };
            vec![
                (detail_url(pid, "/errors", &on), format!("Errors on {s}")),
                (
                    detail_url(pid, "/events?family=error", &on),
                    format!("Events on {s}"),
                ),
            ]
        }
        "churn_point" => vec![
            (
                format!("/p/{pid}/watch?churned_at={}", url_enc(s)),
                "Watch a session that churned here".to_owned(),
            ),
            (detail_url(pid, "/churn", f), "Churn".to_owned()),
        ],
        "stick_driver" | "value_moment" | "friction" | "conversion_driver" => b
            .features
            .iter()
            .find(|x| x.declared && x.key == *s)
            .map(|x| {
                vec![(
                    detail_url(pid, &format!("/features/{}", url_enc(s)), f),
                    x.name.clone().unwrap_or_else(|| s.clone()),
                )]
            })
            .unwrap_or_default(),
        // The subject is `feature:step`; the funnel is on the feature's
        // page since #620.
        "funnel_drop" => {
            let key = s.split_once(':').map_or(s.as_str(), |(k, _)| k);
            vec![(
                detail_url(pid, &format!("/features/{}", url_enc(key)), f),
                "Its funnel".to_owned(),
            )]
        }
        "role_blind_spot" => vec![(
            detail_url(pid, "/breakdown?by=role", f),
            "Break down by role".to_owned(),
        )],
        "flag_regression" | "flag_win" => {
            vec![(detail_url(pid, "/flags", f), "Flags".to_owned())]
        }
        "slow_pages" => vec![(detail_url(pid, "/pages", f), "Pages".to_owned())],
        _ => Vec::new(),
    }
}

/// One side of a before/after comparison.
pub(crate) struct Side {
    /// What it is ("v1", "with the flag").
    pub(crate) label: String,
    /// The value.
    pub(crate) value: f64,
    /// The value as shown.
    pub(crate) shown: String,
    /// What it rests on ("4,814 sessions").
    pub(crate) base: String,
}

/// The two values an insight compares, when the board holds them both
/// (the release table, the flag cohorts); none otherwise, and then the
/// lead card shows no comparison rather than an invented one. The second
/// side is the one the insight is about; `bool`: it is the worse one.
pub(super) fn compared(b: &Board, c: &Insight) -> Option<([Side; 2], bool)> {
    let sessions = |n: i64| format!("{} sessions", compact(n));
    match c.kind {
        "release_regression" => {
            let [new, old, ..] = b.releases.as_slice() else {
                return None;
            };
            (new.release == c.subject).then(|| {
                (
                    [
                        Side {
                            label: old.release.clone(),
                            value: old.error_rate,
                            shown: pct(old.error_rate),
                            base: sessions(old.sessions),
                        },
                        Side {
                            label: new.release.clone(),
                            value: new.error_rate,
                            shown: pct(new.error_rate),
                            base: sessions(new.sessions),
                        },
                    ],
                    new.error_rate > old.error_rate,
                )
            })
        }
        "flag_regression" | "flag_win" => {
            let x = b.flag_cohorts.iter().find(|x| x.flag == c.subject)?;
            let (w, o) = (&x.with, &x.without);
            let side = |label: &str, s: &openlens_engine::CohortStats| {
                if c.kind == "flag_win" {
                    Side {
                        label: label.to_owned(),
                        value: s.tasks_per_session,
                        shown: format!("{:.2}", s.tasks_per_session),
                        base: sessions(s.sessions),
                    }
                } else {
                    Side {
                        label: label.to_owned(),
                        value: s.error_rate,
                        shown: pct(s.error_rate),
                        base: sessions(s.sessions),
                    }
                }
            };
            let worse = if c.kind == "flag_win" {
                w.tasks_per_session < o.tasks_per_session
            } else {
                w.error_rate > o.error_rate
            };
            Some(([side("without", o), side("with", w)], worse))
        }
        _ => None,
    }
}

/// The before/after bars: widths are SVG attributes (the CSP forbids
/// `style`), the larger value fills the track.
pub(crate) fn compare(sides: &[Side; 2], worse: bool) -> Markup {
    let max = sides
        .iter()
        .map(|s| s.value)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    html! {
        dl.compare {
            @for (i, s) in sides.iter().enumerate() {
                @let w = (s.value / max * 100.0).clamp(1.5, 100.0);
                @let cls = if i == 0 { "fg" } else if worse { "fg bad" } else { "fg good" };
                dt { (s.label) }
                dd {
                    svg viewBox="0 0 100 10" preserveAspectRatio="none" aria-hidden="true" focusable="false" {
                        rect.bg width="100" height="10" rx="5" {}
                        rect class=(cls) width=(format!("{w:.1}")) height="10" rx="5" {}
                    }
                }
                dd.v { (s.shown) small { (s.base) } }
            }
        }
    }
}

/// The top insight as the lead card: rank, tag, evidence, a comparison
/// when the board holds both values, the next step and its actions.
fn insight_lead(
    pid: &str,
    f: &Filters,
    b: &Board,
    c: &Insight,
    pl: Option<&(String, String)>,
) -> Markup {
    html! {
        article.card.lead {
            div.lead-top {
                span.rank aria-hidden="true" { "01" }
                (tag(c.kind))
                @if !c.subject.is_empty() { span.muted.small.mono { (c.subject) } }
            }
            div.lead-text {
                @if let Some((t, _)) = pl {
                    h2 title={ "Original: " (c.title) } { span.sr-only { "Insight 1: " } (t) } // no_detail: the actions below
                } @else {
                    h2 { span.sr-only { "Insight 1: " } (c.title) } // no_detail: the actions below
                }
                p.evidence { (c.evidence) }
            }
            @if let Some((sides, worse)) = compared(b, c) { (compare(&sides, worse)) }
            div.next {
                (icon("idea"))
                div {
                    strong { "Next step" }
                    p title=[pl.map(|_| format!("Original: {}", c.next_step))] { (pl.map_or(c.next_step.as_str(), |x| x.1.as_str())) }
                }
            }
            @if !shared() {
                div.actions {
                    a.btn.primary href=(ticket(pid, c)) { "Create ticket" }
                    @for (j, (href, label)) in links(pid, f, b, c).iter().enumerate() {
                        a class=(if j == 0 { "btn" } else { "btn ghost" }) href=(href) { (label) }
                    }
                }
            }
        }
    }
}

/// Rows the ranked list shows before "Show all" (#615).
const RANKED_ROWS: usize = 4;

/// One ranked row: its rank on the page, the insight and its plain
/// wording when it has one.
fn row(
    pid: &str,
    f: &Filters,
    b: &Board,
    rank: usize,
    c: &Insight,
    pl: Option<&(String, String)>,
) -> Markup {
    html! {
        li.row {
            span.rank aria-hidden="true" { (format!("{rank:02}")) }
            div {
                @if let Some((t, _)) = pl {
                    h3 title={ "Original: " (c.title) } { span.sr-only { "Insight " (rank) ": " } (t) }
                } @else {
                    h3 { span.sr-only { "Insight " (rank) ": " } (c.title) }
                }
                div.meta { (tag(c.kind)) span { (c.evidence) } }
                @if !shared() {
                    div.links {
                        @for (href, label) in links(pid, f, b, c) { a href=(href) { (label) } }
                        a href=(ticket(pid, c)) { "Create ticket" }
                    }
                }
            }
        }
    }
}

/// The section: an eyebrow with the count, the lead and the ranked list;
/// an explained card while there is nothing to rank. A regressed latest
/// release leads as its verdict (#615), and its regression card is then
/// not repeated; otherwise the top insight leads.
pub(super) fn focus(pid: &str, f: &Filters, b: &Board, plain: &[(String, String)]) -> Markup {
    // The plain wording when it differs from the original (which then
    // stays as the tooltip).
    let wording = |i: usize, c: &Insight| {
        plain
            .get(i)
            .filter(|x| x.0 != c.title || x.1 != c.next_step)
    };
    let verdict = b.release_lead();
    let ranked = b.ranked_insights();
    let (lead, rest) = match (&verdict, ranked.split_first()) {
        (Some(_), _) => (None, ranked.as_slice()),
        (None, Some((first, rest))) => (Some(*first), rest),
        (None, None) => (None, ranked.as_slice()),
    };
    let count = ranked.len() + usize::from(verdict.is_some());
    // Ranks on the page: the lead is 01, the list follows.
    let first_rank = 2;
    html! {
        section.focus aria-labelledby="focus-h" {
            h2.eyebrow id="focus-h" { "What to focus on" @if count > 0 { " " span.count { (count) } } } // no_detail: the cards below carry their links
            @if count == 0 {
                div.card.pad.focus-empty {
                    @if b.readiness.judged_users < b.readiness.needed {
                        p { strong { "Insights need a little history." } " They compare users who stayed with users who left, so they wait until "
                            (b.readiness.needed) " users are older than the churn window (now " (b.readiness.judged_users) ")." }
                        p.muted.small { "Backfilling past users and success events (" code { "openlens-cli import" } ") gets there on day one." }
                    } @else {
                        p { strong { "Nothing stands out right now." } " No feature, role, release or page differs enough from the rest to be worth your attention." }
                    }
                }
            } @else {
                div.focus-grid {
                    @if let Some(l) = &verdict {
                        (super::lead::card(pid, f, l))
                    } @else if let Some((i, c)) = lead {
                        (insight_lead(pid, f, b, c, wording(i, c)))
                    }
                    @if !rest.is_empty() {
                        div.card.ranked {
                            ol start=(first_rank) {
                                @for (n, (i, c)) in rest.iter().take(RANKED_ROWS).enumerate() {
                                    (row(pid, f, b, first_rank + n, c, wording(*i, c)))
                                }
                            }
                            @if rest.len() > RANKED_ROWS {
                                details {
                                    summary { "Show all " (count) }
                                    ol start=(first_rank + RANKED_ROWS) {
                                        @for (n, (i, c)) in rest.iter().enumerate().skip(RANKED_ROWS) {
                                            (row(pid, f, b, first_rank + n, c, wording(*i, c)))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

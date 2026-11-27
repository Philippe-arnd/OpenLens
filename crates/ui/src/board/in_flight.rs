//! In flight (#617): next to the usage chart, whether the last actions
//! worked. Three rows: the goals and how far along they are (or the
//! Goals page's suggested target), the tickets filed from findings with
//! the release that shipped them and the effect measured 14 days later,
//! and the flags running with their cohort result. An empty row says how
//! the loop works in one sentence and offers one action. On a share link
//! the actions and the issue links, which need a login, are left out.

use crate::*;
use openlens_engine::commands::in_flight::{GoalProgress, InFlight, Measured, Running, Stage};

/// The card.
pub(super) fn in_flight(pid: &str, b: &Board) -> Markup {
    let d = &b.in_flight;
    html! {
        section.card.pad aria-labelledby="loop-h" {
            div.section-head {
                h2 id="loop-h" { "In flight" } // no_detail: each row links to its own page
                span.muted.small { "Act, then compare" }
            }
            div.loop {
                (goals(pid, d))
                (tickets(pid, b))
                (flags(pid, d))
            }
        }
    }
}

/// One row: its icon, its title and body, and at most one action (not
/// on a share link).
fn row(glyph: &str, title: &str, body: Markup, action: Option<(String, &str)>) -> Markup {
    html! {
        div.loop-item {
            span.chip-icon { (icon(glyph)) }
            div {
                b { (title) }
                (body)
            }
            @if let (false, Some((href, label))) = (shared(), action) {
                a.btn.sm href=(href) { (label) }
            }
        }
    }
}

/// "and 2 more", under a list cut at three.
fn more(n: usize) -> Markup {
    html! { @if n > 0 { p.muted.small { "And " (n) " more." } } }
}

fn status_tag(status: &str) -> Markup {
    let tone = match status {
        "reached" | "on track" => "good",
        "at risk" => "warn",
        "off track" => "bad",
        _ => "info",
    };
    html! { span class={ "tag tone-" (tone) } { (status) } }
}

fn goal(g: &GoalProgress) -> Markup {
    let fmt = |v| crate::goals::kpi_fmt(v, g.share);
    html! {
        li {
            span { (g.label) ": " }
            @match (g.current, g.progress()) {
                (Some(now), Some(frac)) => {
                    strong { (fmt(now)) } " now, target " (fmt(g.target)) " by " (g.deadline)
                    // From the value when the goal was set to the target.
                    (super::bar_with(frac, &format!("{:.0}% of the way", frac * 100.0)))
                }
                _ => { "target " (fmt(g.target)) " by " (g.deadline) span.muted { " · measured within the hour" } }
            }
            @if let Some(s) = &g.status { " " (status_tag(s)) }
        }
    }
}

fn goals(pid: &str, d: &InFlight) -> Markup {
    let href = format!("/p/{pid}/goals");
    if !d.goals.is_empty() {
        let n = d.goals.len() + d.more_goals;
        let title = format!("{n} {}", if n == 1 { "goal" } else { "goals" });
        let body = html! {
            ul.plain-list { @for g in &d.goals { (goal(g)) } }
            (more(d.more_goals))
        };
        return row("flag", &title, body, Some((href, "Goals")));
    }
    let body = match &d.suggestion {
        Some(s) => html! {
            p.muted.small {
                "Suggested: " (s.label) " at " (crate::goals::kpi_fmt(s.target, s.share))
                ", from " (crate::goals::kpi_fmt(s.current, s.share)) " now."
            }
        },
        None => html! {
            p.muted.small { "Put a target on a key number and the board follows how far along it is." }
        },
    };
    row(
        "flag",
        "No goal yet",
        body,
        Some((format!("{href}#set-goal-h"), "Set goal")),
    )
}

fn ticket(t: &Measured) -> Markup {
    let (n, url) = &t.issue;
    html! {
        li {
            @if shared() || !url.starts_with("https://") { (t.title) " (#" (n) ")" }
            @else { a href=(url) rel="noopener noreferrer" { (t.title) " (#" (n) ")" } }
            br;
            span.muted.small {
                @match &t.stage {
                    Stage::Open => { "Issue open: measured 14 days after the release that ships it." }
                    Stage::Closed { day } => { "Closed on " (day) ", waiting for a release to ship it." }
                    Stage::Shipped { release, measure_on } => {
                        "Shipped in " code { (release) } "; the effect is measured on " (measure_on) "."
                    }
                    Stage::Measured { release, word, evidence } => {
                        @if !release.is_empty() { "Shipped in " code { (release) } ": " }
                        @let tone = match word.as_str() { "improved" => "good", "worse" => "bad", _ => "info" };
                        span class={ "tag tone-" (tone) } { (word) } " " (evidence)
                    }
                }
            }
        }
    }
}

fn tickets(pid: &str, b: &Board) -> Markup {
    let d = &b.in_flight;
    if !d.tickets.is_empty() {
        let n = d.tickets.len() + d.more_tickets;
        let title = format!(
            "{n} {} being measured",
            if n == 1 { "ticket" } else { "tickets" }
        );
        let body = html! {
            ul.plain-list { @for t in &d.tickets { (ticket(t)) } }
            (more(d.more_tickets))
        };
        return row(
            "task",
            &title,
            body,
            Some((format!("/p/{pid}/tickets"), "Tickets")),
        );
    }
    // The first card of the board, when there is one, becomes the ticket.
    let action = match b.insights.first() {
        Some(c) => (super::focus::ticket(pid, c), "Ticket from card 1"),
        None => (format!("/p/{pid}/tickets"), "Tickets"),
    };
    row(
        "task",
        "No ticket being measured",
        html! { p.muted.small { "File a card as a ticket: 14 days after the release that ships it, its effect shows here." } },
        Some(action),
    )
}

fn flag(r: &Running) -> Markup {
    html! {
        li {
            code { (r.key) } " " span.muted.small { (r.rollout) "% rollout" }
            br;
            span.small {
                @match &r.cohort {
                    Some(c) => {
                        "Sessions with errors " (pct(c.with.error_rate)) " with it, " (pct(c.without.error_rate)) " without; "
                        "tasks per session " (format!("{:.2}", c.with.tasks_per_session)) " vs " (format!("{:.2}", c.without.tasks_per_session)) " · "
                        (confidence(r.p()))
                    }
                    None => { span.muted { "No session carried it in the last 30 days yet." } }
                }
            }
        }
    }
}

fn flags(pid: &str, d: &InFlight) -> Markup {
    let href = format!("/p/{pid}/flags");
    if d.flags.is_empty() {
        return row(
            "release",
            "No flag running",
            html! { p.muted.small { "Turn a change on for some users first: each running flag compares their sessions with everyone else's here." } },
            Some((href, "Flags")),
        );
    }
    let n = d.flags.len() + d.more_flags;
    let title = format!("{n} {} running", if n == 1 { "flag" } else { "flags" });
    let body = html! {
        ul.plain-list { @for r in &d.flags { (flag(r)) } }
        (more(d.more_flags))
    };
    row("release", &title, body, Some((href, "Flags")))
}

/// A p-value in words: whether the difference can be trusted yet.
fn confidence(p: Option<f64>) -> Markup {
    match p {
        None => {
            html! { span.muted title="Fewer than 30 sessions on one side" { "too few sessions" } }
        }
        Some(p) if p < openlens_engine::stats::ALPHA => {
            html! { strong title=(format!("p = {p:.3}")) { "significant" } }
        }
        Some(p) => {
            html! { span.muted title=(format!("p = {p:.3}")) { "not yet (p = " (format!("{p:.2}")) ")" } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::commands::in_flight::Suggestion;
    use openlens_engine::{CohortStats, FlagCohort, Insight};

    /// The card's three rows, in order.
    fn rows(html: &str) -> Vec<String> {
        html.split(r#"<div class="loop-item">"#)
            .skip(1)
            .map(str::to_owned)
            .collect()
    }

    fn card(b: &Board) -> String {
        in_flight("p1", b).into_string()
    }

    #[test]
    fn each_empty_row_says_the_loop_in_one_sentence_with_one_action() {
        let b = Board::default();
        let html = card(&b);
        let rs = rows(&html);
        assert_eq!(rs.len(), 3, "{html}");
        for (r, (title, href)) in rs.iter().zip([
            ("No goal yet", "/p/p1/goals#set-goal-h"),
            ("No ticket being measured", "/p/p1/tickets"),
            ("No flag running", "/p/p1/flags"),
        ]) {
            assert!(r.contains(title), "{r}");
            assert_eq!(r.matches("<p ").count(), 1, "one sentence: {r}");
            assert_eq!(r.matches(r#"class="btn sm""#).count(), 1, "one action: {r}");
            assert!(r.contains(&format!(r#"href="{href}""#)), "{r}");
            // An explained empty state, not zeros.
            assert!(!r.contains('%') && !r.contains("<ul"), "{r}");
        }
        // With a card on the board, the ticket action drafts it.
        let b = Board {
            insights: vec![Insight {
                kind: "churn_point",
                title: "t".into(),
                evidence: String::new(),
                next_step: String::new(),
                score: 1.0,
                subject: "billing".into(),
                figure: None,
            }],
            ..Board::default()
        };
        let html = card(&b);
        let rs = rows(&html);
        let r = &rs[1];
        assert!(
            r.contains("/p/p1/tickets/new?kind=insight&amp;subject=churn_point%3Abilling"),
            "{r}"
        );
        // A share link keeps the sentences and drops the actions.
        let shared = render_shared(|| in_flight("p1", &b)).into_string();
        assert_eq!(rows(&shared).len(), 3);
        assert!(!shared.contains("btn"), "{shared}");
        assert!(!shared.contains("/p/p1/"), "{shared}");
    }

    #[test]
    fn the_goal_suggestion_is_the_one_the_goals_page_computed() {
        let tip = Suggestion {
            kpi: "error_rate".into(),
            label: "Sessions with errors".into(),
            share: true,
            current: 0.041,
            target: 0.029,
        };
        let b = Board {
            in_flight: InFlight {
                suggestion: Some(tip.clone()),
                ..InFlight::default()
            },
            ..Board::default()
        };
        let html = card(&b);
        let rs = rows(&html);
        let r = &rs[0];
        assert!(r.contains(&tip.label), "{r}");
        assert!(
            r.contains(&pct(tip.target)) && r.contains(&pct(tip.current)),
            "{r}"
        );
        assert!(r.contains("Set goal"), "{r}");
    }

    #[test]
    fn populated_rows_show_progress_release_effect_and_cohort_from_the_data() {
        let goal = |current: f64| GoalProgress {
            label: "Sessions with errors".into(),
            share: true,
            baseline: 0.10,
            target: 0.05,
            deadline: "2027-01-31".into(),
            current: Some(current),
            status: Some("on track".into()),
        };
        let measured = |stage: Stage| Measured {
            title: "Fix checkout".into(),
            issue: (12, "https://github.com/o/r/issues/12".into()),
            stage,
        };
        let stats = |error_rate: f64, tasks: f64| CohortStats {
            error_rate,
            tasks_per_session: tasks,
            ..CohortStats::default()
        };
        let cohort = |flag: &str, p: f64| FlagCohort {
            flag: flag.into(),
            with: stats(0.021, 2.6),
            without: stats(0.034, 2.4),
            p_errors: Some(p),
            p_tasks: None,
        };
        let b = Board {
            in_flight: InFlight {
                goals: vec![goal(0.09), goal(0.06)],
                tickets: vec![
                    measured(Stage::Shipped {
                        release: "v2.1".into(),
                        measure_on: "2026-12-05".into(),
                    }),
                    measured(Stage::Measured {
                        release: "v2.0".into(),
                        word: "improved".into(),
                        evidence: "4.0% before → 2.9% after".into(),
                    }),
                ],
                more_tickets: 2,
                flags: vec![
                    Running {
                        key: "new-cart".into(),
                        rollout: 20,
                        cohort: Some(cohort("new-cart", 0.001)),
                    },
                    Running {
                        key: "dark-nav".into(),
                        rollout: 50,
                        cohort: Some(cohort("dark-nav", 0.4)),
                    },
                ],
                ..InFlight::default()
            },
            ..Board::default()
        };
        let html = card(&b);
        let rs = rows(&html);
        // Goals: the values and a bar each; the goal further along has
        // the wider bar.
        let g = &rs[0];
        assert!(
            g.contains("2 goals") && g.contains(&pct(0.09)) && g.contains(&pct(0.06)),
            "{g}"
        );
        assert!(g.contains(&pct(0.05)) && g.contains("on track"), "{g}");
        let widths: Vec<f64> = g
            .split(r#"class="fg" width=""#)
            .skip(1)
            .filter_map(|s| s.split('"').next()?.parse().ok())
            .collect();
        assert_eq!(widths.len(), 2, "{g}");
        assert!(widths[1] > widths[0], "{widths:?}");
        // Tickets: the release, the day it is measured, then the effect.
        let t = &rs[1];
        assert!(
            t.contains("4 tickets being measured") && t.contains("And 2 more"),
            "{t}"
        );
        assert!(t.contains("v2.1") && t.contains("2026-12-05"), "{t}");
        assert!(
            t.contains("v2.0") && t.contains("improved") && t.contains("2.9% after"),
            "{t}"
        );
        assert!(
            t.contains(r#"href="https://github.com/o/r/issues/12""#),
            "{t}"
        );
        // Flags: both sides of each cohort and whether it can be trusted.
        let f = &rs[2];
        let (first, second) = f.split_at(f.find("dark-nav").unwrap());
        assert!(
            first.contains(&pct(0.021)) && first.contains(&pct(0.034)),
            "{first}"
        );
        assert!(
            first.contains("significant") && !first.contains("not yet"),
            "{first}"
        );
        assert!(second.contains("not yet"), "{second}");
        // Populated rows keep their sentences out: no empty-state titles.
        assert!(
            !html.contains("No goal yet") && !html.contains("No flag running"),
            "{html}"
        );
        // On a share link the issue links go.
        let shared = render_shared(|| in_flight("p1", &b)).into_string();
        assert!(
            !shared.contains("github.com") && shared.contains("Fix checkout"),
            "{shared}"
        );
    }
}

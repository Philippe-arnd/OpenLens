//! Goals and objectives, the audit log, forms and the timeline.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// The rows the Goals page shows, as `goals.list` returns them (#494).
pub use openlens_engine::commands::goals::{GoalChoice, GoalRow, GuardRow, ObjectiveRow};

pub(crate) fn kpi_fmt(v: f64, share: bool) -> String {
    if share {
        pct(v)
    } else if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// The KPI per release with the target as a line (one axis, SVG).
pub(crate) fn goal_chart(r: &GoalRow) -> Markup {
    let (w, h) = (320.0_f64, 90.0_f64);
    let vals: Vec<f64> = r
        .series
        .iter()
        .map(|x| x.1)
        .chain([r.goal.target])
        .chain(r.current)
        .collect();
    let max = vals.iter().copied().fold(f64::MIN, f64::max).max(1e-9);
    let y = |v: f64| h - 6.0 - (v / max) * (h - 16.0);
    let n = r.series.len().max(1);
    let x = |i: usize| {
        if n == 1 {
            w / 2.0
        } else {
            8.0 + i as f64 * (w - 16.0) / (n - 1) as f64
        }
    };
    let line: String = r
        .series
        .iter()
        .enumerate()
        .map(|(i, (_, v))| format!("{}{:.1} {:.1}", if i == 0 { "M" } else { "L" }, x(i), y(*v)))
        .collect();
    let ty = y(r.goal.target);
    html! {
        svg.goal-chart viewBox={ "0 0 " (w) " " (h) } role="img" aria-label={ (r.label) " per release, target " (kpi_fmt(r.goal.target, r.share)) } {
            line.target x1="0" x2=(w) y1=(format!("{ty:.1}")) y2=(format!("{ty:.1}")) {}
            text.tlabel x=(w - 4.0) y=(format!("{:.1}", ty - 3.0)) text-anchor="end" { "target " (kpi_fmt(r.goal.target, r.share)) }
            @if r.series.len() > 1 { path.series d=(line) {} }
            @for (i, (d, v)) in r.series.iter().enumerate() {
                circle.dot cx=(format!("{:.1}", x(i))) cy=(format!("{:.1}", y(*v))) r="3.5" { title { (d) ": " (kpi_fmt(*v, r.share)) } }
            }
        }
    }
}

/// How far a goal has come from its baseline towards its target (0–1),
/// once the KPI has a value now.
pub(crate) fn travelled(g: &openlens_engine::goals::Goal, current: Option<f64>) -> Option<f64> {
    current.map(|c| {
        let total = g.target - g.baseline;
        if total.abs() < 1e-12 {
            1.0
        } else {
            ((c - g.baseline) / total).clamp(0.0, 1.0)
        }
    })
}

/// A goal's progress bar (#550): the bar's width is an SVG attribute (the
/// CSP forbids `style`), never wider than its track.
pub(crate) fn goal_progress(frac: f64) -> Markup {
    let frac = if frac.is_finite() {
        frac.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let w = if frac > 0.0 {
        (frac * 100.0).max(1.5)
    } else {
        0.0
    };
    html! {
        svg.progress role="img" aria-label={ (pct(frac)) " of the way to the target" } viewBox="0 0 100 6" preserveAspectRatio="none" {
            rect.track width="100" height="6" {}
            rect.bar width=(format!("{w:.1}")) height="6" {}
        }
    }
}

/// The classes of a status tag.
fn goal_tone(good: bool, bad: bool) -> &'static str {
    if good {
        "tag tone-good"
    } else if bad {
        "tag tone-bad"
    } else {
        "tag tone-warn"
    }
}

/// One goal as a card: status, now against the target, the progress bar,
/// the trend, guardrails and what moved it.
fn goal_card(pid: &str, r: &GoalRow, admin: bool) -> Markup {
    let g = &r.goal;
    let gap = r.current.map(|c| g.target - c);
    let id = format!("goal-{}", g.id);
    html! {
        article.card.pad.stack.goal aria-labelledby=(id) {
            div.section-head {
                h2 id=(id) { (r.label) }
                span class=(goal_tone(r.good, r.bad)) { (r.status) }
            }
            div.goal-now {
                span.kpi-value { (r.current.map_or_else(|| "—".into(), |c| kpi_fmt(c, r.share))) }
                span.muted { "target " strong { (kpi_fmt(g.target, r.share)) } " by " (g.deadline) @if r.days_left > 0 { " (" (r.days_left) " days)" } }
            }
            @if let Some(t) = travelled(g, r.current) {
                div.goal-progress {
                    (goal_progress(t))
                    span.small.muted { (pct(t)) " of the way from " (kpi_fmt(g.baseline, r.share)) }
                }
            }
            @if gap.is_some() || r.projected.is_some() {
                p.small.muted {
                    @if let Some(gp) = gap { "Gap " (kpi_fmt(gp.abs(), r.share)) }
                    @if let Some(pj) = r.projected { @if gap.is_some() { " · " } "trend at the deadline " (kpi_fmt(pj, r.share)) }
                }
            }
            (goal_chart(r))
            @for gr in &r.guards {
                p.small {
                    @if gr.breached { span.tag.tone-bad { "guardrail breached" } } @else { span.tag.tone-good { "guardrail ok" } }
                    " " (gr.label) " " strong { (gr.current.map_or_else(|| "—".into(), |c| kpi_fmt(c, gr.share))) }
                    span.muted { " (limit " (kpi_fmt(gr.limit, gr.share)) ")" }
                }
            }
            @if !r.moves.is_empty() || !r.marks.is_empty() {
                details {
                    summary.small { "What moved it" }
                    ul.plain-list.small {
                        @for m in &r.moves {
                            li { "Release " code { (m.release) } " (" (m.day) "): "
                                span class={ "status " (if m.good { "good" } else { "bad" }) } { (if m.delta > 0.0 { "+" } else { "−" }) (kpi_fmt(m.delta.abs(), r.share)) } }
                        }
                        @for m in &r.marks { li { span.muted { (m.day) } " " (m.text) } }
                    }
                }
            }
            @if let Some(n) = g.releases { p.muted.small { "Deadline set as " (n) " releases from " (g.created) "." } }
            @if !g.owner.is_empty() { p.muted.small { "Owner: " (g.owner) } }
            @if !r.good || admin {
                div.actions {
                    @if !r.good { a.btn href={ "/p/" (pid) "/tickets/new?kind=goal&subject=" (g.id) } { "Create ticket" } }
                    @if admin {
                        form.inline method="post" action={ "/p/" (pid) "/goals/" (g.id) "/delete" } { button.btn.ghost type="submit" { "Remove" } }
                    }
                }
            }
        }
    }
}

/// "Set a goal" (#550): a form card holding one folded form per KPI, its
/// suggested target filled in (same action, method and fields as before).
fn set_goal(pid: &str, choices: &[GoalChoice], deadline: &str) -> Markup {
    html! {
        section.card.wide.form-card aria-labelledby="set-goal-h" {
            div.section-head {
                h2 id="set-goal-h" { (icon("plus")) "Set a goal" }
                p { "Pick a KPI: its suggested target is filled in." }
            }
            div.goal-choices {
                @for c in choices {
                    details.goal-choice open[choices.len() == 1] {
                        summary {
                            strong { (c.label) }
                            span.muted.small { "now " (kpi_fmt(c.current, c.share)) " · " (if c.up { "higher is better" } else { "lower is better" }) }
                        }
                        form method="post" action={ "/p/" (pid) "/goals" } {
                            input type="hidden" name="kpi" value=(c.kpi);
                            div.form-grid {
                                label.field { "Target" @if c.share { " (%)" }
                                    input type="number" name="target" step="any" min="0" value=(if c.share { format!("{:.1}", c.suggested * 100.0) } else { format!("{:.2}", c.suggested) });
                                    small { "Suggested: the best release so far or a 10% step, whichever is closer." }
                                }
                                label.field { "By"
                                    input type="date" name="deadline" value=(deadline);
                                    small { "The deadline (YYYY-MM-DD)." }
                                }
                                label.field { "Or in releases"
                                    input type="number" name="releases" min="1" max="50" placeholder="N releases";
                                    small { "Uses the median time between releases so far." }
                                }
                                label.field { "Guardrail"
                                    select name="guard1" {
                                        option value="" { "no guardrail" }
                                        @for o in choices.iter().filter(|o| o.kpi != c.kpi) { option value=(o.kpi) { "guard: " (o.label) } }
                                    }
                                    small { "A KPI that must not get worse meanwhile: its limit is 5% worse than now, and a breach is an alert." }
                                }
                                label.field { "Owner"
                                    input type="text" name="owner" placeholder="owner" maxlength="80";
                                }
                            }
                            (crate::project_settings::form_foot("Set the goal", html! { "On " (c.label) "." }))
                        }
                    }
                }
            }
            p.muted.small { "Status: on track when the straight line through the releases (or the weeks, for KPIs without a per-release value) reaches the target by the deadline, at risk when it misses by less than 10% of the target, not enough data under three points." }
        }
    }
}

/// Goals on KPIs (#235): target, gap, progress, trajectory; in the v2
/// parts (#550): goal cards with progress bars, the forms as form cards.
pub fn goals(
    p: &ProjectInfo<'_>,
    rows: &[GoalRow],
    choices: &[GoalChoice],
    objectives: &[ObjectiveRow],
    admin: bool,
    deadline: &str,
) -> Markup {
    let open: Vec<&ObjectiveRow> = objectives
        .iter()
        .filter(|o| o.objective.archived.is_none())
        .collect();
    let past: Vec<&ObjectiveRow> = objectives
        .iter()
        .filter(|o| o.objective.archived.is_some())
        .collect();
    layout(
        "Goals",
        Some(p),
        html! {
            div.board.people {
                div.page-head { div {
                    h1 { "Goals" }
                    p { "Where we want each number to be, by when, and whether the trend gets there. Targets are suggested from the releases so far; one click sets them." }
                } }
                @if !open.is_empty() {
                    section.card.pad.stack aria-labelledby="obj-h" {
                        div.section-head { h2 id="obj-h" { "Objectives" } }
                        @for o in &open {
                            div.objective {
                                p {
                                    strong { (o.objective.title) } " "
                                    span class=(goal_tone(o.good, o.bad)) { (o.status) }
                                    span.muted.small { " · until " (o.objective.end) }
                                }
                                ul.plain-list.small { @for (label, now, target, share, st) in &o.results {
                                    li { (label) ": " strong { (now.map_or_else(|| "—".into(), |c| kpi_fmt(c, *share))) } " → " (kpi_fmt(*target, *share)) " · " (st) }
                                } }
                                @if admin { form.inline method="post" action={ "/p/" (p.id) "/objectives/" (o.objective.id) "/delete" } { button.btn.ghost type="submit" { "Remove" } } }
                            }
                        }
                        p.muted.small { "An objective takes the status of its weakest key result, and is archived with the final values when its period ends." }
                    }
                }
                @if rows.is_empty() {
                    div.card.pad { p.muted { "No goal yet. Pick a KPI below: the suggested target is the best release so far or a 10% step, whichever is closer." } }
                } @else {
                    section aria-labelledby="goals-h" {
                        h2.eyebrow id="goals-h" { "Goals" span.count { (rows.len()) } }
                        div.people-grid { @for r in rows { (goal_card(p.id, r, admin)) } }
                    }
                }
                @if admin && !choices.is_empty() { (set_goal(p.id, choices, deadline)) }
                @if admin && !rows.is_empty() {
                    form.card.wide.form-card method="post" action={ "/p/" (p.id) "/objectives" } aria-labelledby="set-obj-h" {
                        div.section-head {
                            h2 id="set-obj-h" { (icon("flag")) "Set an objective" }
                            p { "A few goals under one title, until a date." }
                        }
                        div.form-grid {
                            label.field { "Objective"
                                input type="text" name="title" required maxlength="140" placeholder="Make onboarding effortless";
                            }
                            @for (i, name) in ["goal1", "goal2", "goal3"].iter().enumerate() {
                                label.field { "Key result " (i + 1)
                                    select name=(name) {
                                        option value="" { (if i == 0 { "key result…" } else { "(optional)" }) }
                                        @for r in rows { option value=(r.goal.id) { (r.label) " → " (kpi_fmt(r.goal.target, r.share)) } }
                                    }
                                }
                            }
                            label.field { "Until"
                                input type="date" name="end" value=(deadline) required;
                                small { "The end of the period (YYYY-MM-DD)." }
                            }
                        }
                        (crate::project_settings::form_foot("Set the objective", html! { "It takes the status of its weakest key result." }))
                    }
                }
                @if !past.is_empty() {
                    section.card.pad.stack aria-labelledby="past-h" {
                        div.section-head { h2 id="past-h" { "Past objectives" } }
                        ul.plain-list.small { @for o in &past {
                            @if let Some(a) = &o.objective.archived {
                                li {
                                    strong { (o.objective.title) } " "
                                    span class=(goal_tone(a.achieved, !a.achieved)) { (if a.achieved { "achieved" } else { "missed" }) }
                                    span.muted { " · " (o.objective.start) " – " (o.objective.end) }
                                    @for (_, label, v, t, ok) in &a.results { br; (label) ": " (v.map_or_else(|| "—".into(), |x| format!("{x:.3}"))) " (target " (format!("{t:.3}")) ") " (if *ok { "✓" } else { "✗" }) }
                                }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// The audit log (#230): who changed what, newest first.
pub fn audit(rows: &[(String, String, Option<String>, String, String)]) -> Markup {
    layout(
        "Audit log",
        None,
        html! {
            div.heading { div {
                h1 { "Audit log" }
                p.muted { "Every admin action: users, keys, settings, flags, surveys, feature maps, storage, exports and erasures. It can't be edited or deleted." }
            } }
            @if rows.is_empty() { section.card.wide { p.muted { "Nothing yet." } } }
            @else {
                // The times are UTC (the log spans every project and its zone).
                (crate::tables::TableCard {
                    label: "Audit log",
                    head: html! {},
                    cols: &[
                        crate::tables::Col::Shown("Time (UTC)"), crate::tables::Col::Shown("Who"),
                        crate::tables::Col::Shown("Project"), crate::tables::Col::Shown("Action"),
                        crate::tables::Col::Shown("Detail"),
                    ],
                    body: crate::tables::day_rows(rows, |r| r.0.as_str(), "UTC", 5, |(_, who, proj, act, d), time| html! {
                        tr {
                            td.when.muted { (time) }
                            td.small { (who) }
                            td.small { (proj.as_deref().unwrap_or("—")) }
                            td { span.ev.ev-other.action { (act) } }
                            td.small { (d) }
                        }
                    }),
                    foot: html! { div.pager { span { (rows.len()) " entries · newest first · times in UTC" } } },
                }
                .render())
            }
        },
    )
}

/// Forms (#232): which field makes people give up.
pub fn forms(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    list: &[openlens_engine::forms::FormStats],
) -> Markup {
    layout_in(
        "Forms",
        Some(p),
        context_bar(p.id, b, f, "/forms", false),
        html! {
            div.heading { div {
                h1 { "Forms" }
                p.muted { "Last 30 days, " strong { (f.environment) } ". A form starts when someone leaves one of its fields, and is abandoned when the session ends without sending it. Field values are never sent; passwords and masked fields send nothing." }
            } }
            @if list.is_empty() { section.card.wide { p.muted { "No form filled yet (SDK builds from 2026-11-23 on send field events)." } } }
            @for fm in list {
                // One table per form (#548): each field's funnel bar is the
                // share of the starts that reached it.
                (crate::tables::TableCard {
                    label: &fm.form,
                    head: html! {
                        h2 { (icon("task")) span.mono { (fm.form) } span.muted.small { " · " (fm.path) } }
                        p.form-sum {
                            strong { (compact(fm.starts)) } " started · " strong { (compact(fm.submits)) } " sent · "
                            span class={ "status " (if fm.abandonment() > 0.5 { "bad" } else if fm.abandonment() > 0.25 { "warn" } else { "good" }) } { (pct(fm.abandonment())) " abandoned" }
                            @if fm.submits > 0 { " · " (duration(fm.median_secs)) " to fill (median)" }
                            @if fm.starts < 20 { " " span.chip title="Under 20 starts: the rates move a lot" { "low sample" } }
                        }
                    },
                    cols: &[
                        crate::tables::Col::Shown("Field"), crate::tables::Col::Shown("Reached"),
                        crate::tables::Col::Num("Last before leaving"), crate::tables::Col::Num("Came back to it"),
                        crate::tables::Col::Num("Time spent"),
                    ],
                    body: html! { @for fl in &fm.fields {
                        @let left = fl.last_before_leaving as f64 / (fm.starts - fm.submits).max(1) as f64;
                        tr {
                            th scope="row" { span.mono { (fl.name) } }
                            td.funnel title={ (compact(fl.sessions)) " sessions" } {
                                (crate::board::bar_with(fl.sessions as f64 / fm.starts.max(1) as f64, &pct((fl.sessions as f64 / fm.starts.max(1) as f64).min(1.0))))
                            }
                            td.num { (compact(fl.last_before_leaving)) " " span.muted { "(" (pct(left)) ")" } }
                            td.num { (compact(fl.revisited)) }
                            td.num { (duration(fl.median_ms as f64 / 1000.0)) }
                        }
                    } },
                    foot: html! {
                        @if let Some(s) = &fm.abandoned_session { p.small.pad { a href={ "/p/" (p.id) "/replays/" (s) } { (icon("play")) " Replay a session that gave up" } } }
                    },
                }
                .render())
            }
        },
    )
}

/// The timeline of changes (#267).
pub fn timeline(p: &ProjectInfo<'_>, marks: &[Mark], me: &str, admin: bool, today: &str) -> Markup {
    layout(
        "Timeline",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Timeline" }
                p.muted { "What changed, last 90 days: releases, flag and survey changes, feature map pushes, alerts, and notes. Each day with a change is marked on the active users chart." }
            } }
            form.card.form-card method="post" action={ "/p/" (p.id) "/timeline" } aria-labelledby="note-h" {
                div.section-head {
                    h2 #note-h { (icon("idea")) "Add a note" }
                    p { "Anyone signed in can add one. It shows here and on the active users chart." }
                }
                div.form-grid {
                    label.field { "Day" input type="date" name="day" value=(today) required; }
                    label.field.wide { "Note" input type="text" name="body" required maxlength="500" placeholder="Pricing page redesign shipped"; small { "Up to 500 characters." } }
                }
                div.form-foot { button.btn.primary type="submit" { "Add the note" } }
            }
            (crate::live_screens::timeline_table(p.id, marks, me, admin))
        },
    )
}

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

/// Goals on KPIs (#235): target, gap, progress, trajectory.
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
            div.heading { div {
                h1 { "Goals" }
                p.muted { "Where we want each number to be, by when, and whether the trend gets there. Targets are suggested from the releases so far; one click sets them." }
            } }
            @if !open.is_empty() {
                section.card.wide {
                    h2.sun { (icon("flag")) "Objectives" }
                    @for o in &open {
                        div.objective {
                            p {
                                strong { (o.objective.title) } " "
                                span class={ "status " (if o.good { "good" } else if o.bad { "bad" } else { "warn" }) } { (o.status) }
                                span.muted.small { " · until " (o.objective.end) }
                            }
                            ul.plain-list.small { @for (label, now, target, share, st) in &o.results {
                                li { (label) ": " strong { (now.map_or_else(|| "—".into(), |c| kpi_fmt(c, *share))) } " → " (kpi_fmt(*target, *share)) " · " (st) }
                            } }
                            @if admin { form.inline method="post" action={ "/p/" (p.id) "/objectives/" (o.objective.id) "/delete" } { button.ghost type="submit" { "Remove" } } }
                        }
                    }
                    p.muted.small { "An objective takes the status of its weakest key result, and is archived with the final values when its period ends." }
                }
            }
            @if rows.is_empty() { section.card.wide { p.muted { "No goal yet. Pick a KPI below: the suggested target is the best release so far or a 10% step, whichever is closer." } } }
            div.grid {
                @for r in rows {
                    @let g = &r.goal;
                    @let gap = r.current.map(|c| g.target - c);
                    @let travelled = r.current.map(|c| { let total = g.target - g.baseline; if total.abs() < 1e-12 { 1.0 } else { ((c - g.baseline) / total).clamp(0.0, 1.0) } });
                    section.card {
                        h2.violet { (icon("star")) (r.label) }
                        p {
                            span class={ "status " (if r.good { "good" } else if r.bad { "bad" } else { "warn" }) } { (r.status) }
                            " · target " strong { (kpi_fmt(g.target, r.share)) } " by " (g.deadline)
                            @if r.days_left > 0 { span.muted { " (" (r.days_left) " days)" } }
                        }
                        p.small {
                            "Now " strong { (r.current.map_or_else(|| "—".into(), |c| kpi_fmt(c, r.share))) }
                            " · from " (kpi_fmt(g.baseline, r.share))
                            @if let Some(gp) = gap { " · gap " (kpi_fmt(gp.abs(), r.share)) }
                            @if let Some(pj) = r.projected { " · trend at the deadline " (kpi_fmt(pj, r.share)) }
                        }
                        @if let Some(t) = travelled { div.meter-row { span.small.muted { "way travelled" } (meter(t)) span.small { (pct(t)) } } }
                        (goal_chart(r))
                        @for gr in &r.guards {
                            p.small {
                                @if gr.breached { span.status.bad { "guardrail breached" } } @else { span.status.good { "guardrail ok" } }
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
                        @if !r.good { p.small { a href={ "/p/" (p.id) "/tickets/new?kind=goal&subject=" (g.id) } { "Create ticket →" } } }
                        @if admin {
                            form.inline method="post" action={ "/p/" (p.id) "/goals/" (g.id) "/delete" } { button.ghost type="submit" { "Remove" } }
                        }
                    }
                }
            }
            @if admin && !choices.is_empty() {
                section.card.wide {
                    h2.mint { (icon("plus")) "Set a goal" }
                    div.table-scroll { table.plain {
                        thead { tr { th { "KPI" } (th_num("Now")) th { "Target" } th { "By" } th { "Owner" } th { span.sr-only { "Actions" } } } }
                        tbody { @for c in choices {
                            tr {
                                th scope="row" { (c.label) span.muted.small { " · " (if c.up { "higher is better" } else { "lower is better" }) } }
                                td.num { (kpi_fmt(c.current, c.share)) }
                                td colspan="4" {
                                    form.inline method="post" action={ "/p/" (p.id) "/goals" } {
                                        input type="hidden" name="kpi" value=(c.kpi);
                                        label { span.sr-only { "Target" } input type="number" name="target" step="any" min="0" value=(if c.share { format!("{:.1}", c.suggested * 100.0) } else { format!("{:.2}", c.suggested) }) aria-label={ "Target for " (c.label) }; @if c.share { "%" } }
                                        label { span.sr-only { "Deadline" } input type="date" name="deadline" value=(deadline) aria-label={ "Deadline for " (c.label) }; }
                                        label { span.sr-only { "Or in releases" } input.narrow type="number" name="releases" min="1" max="50" placeholder="or N releases" aria-label={ "Or a deadline in releases for " (c.label) }; }
                                        label { span.sr-only { "Guardrail" }
                                            select name="guard1" aria-label={ "Guardrail for " (c.label) } {
                                                option value="" { "no guardrail" }
                                                @for o in choices.iter().filter(|o| o.kpi != c.kpi) { option value=(o.kpi) { "guard: " (o.label) } }
                                            }
                                        }
                                        label { span.sr-only { "Owner" } input type="text" name="owner" placeholder="owner" maxlength="80" aria-label={ "Owner of the goal on " (c.label) }; }
                                        button.soft type="submit" { "Set" }
                                    }
                                }
                            }
                        } }
                    } }
                    p.muted.small { "The suggested target is filled in. Status: on track when the straight line through the releases (or the weeks, for KPIs without a per-release value) reaches the target by the deadline, at risk when it misses by less than 10% of the target, not enough data under three points. A deadline in releases uses the median time between releases so far. A guardrail is a KPI that must not get worse meanwhile: its limit is 5% worse than now, and a breach is an alert." }
                }
            }
            @if admin && !rows.is_empty() {
                section.card.wide {
                    h2.sun { (icon("flag")) "Set an objective" }
                    form.inline method="post" action={ "/p/" (p.id) "/objectives" } {
                        label { span.sr-only { "Objective" } input type="text" name="title" required maxlength="140" placeholder="Make onboarding effortless" aria-label="Objective"; }
                        @for (i, name) in ["goal1", "goal2", "goal3"].iter().enumerate() {
                            label { span.sr-only { "Key result " (i + 1) }
                                select name=(name) aria-label={ "Key result " (i + 1) } {
                                    option value="" { (if i == 0 { "key result…" } else { "(optional)" }) }
                                    @for r in rows { option value=(r.goal.id) { (r.label) " → " (kpi_fmt(r.goal.target, r.share)) } }
                                }
                            }
                        }
                        label { span.sr-only { "Until" } input type="date" name="end" value=(deadline) required aria-label="Period end"; }
                        button.soft type="submit" { "Set" }
                    }
                }
            }
            @if !past.is_empty() {
                section.card.wide {
                    h2 { (icon("clock")) "Past objectives" }
                    ul.plain-list.small { @for o in &past {
                        @if let Some(a) = &o.objective.archived {
                            li {
                                strong { (o.objective.title) } " "
                                span class={ "status " (if a.achieved { "good" } else { "bad" }) } { (if a.achieved { "achieved" } else { "missed" }) }
                                span.muted { " · " (o.objective.start) " – " (o.objective.end) }
                                @for (_, label, v, t, ok) in &a.results { br; (label) ": " (v.map_or_else(|| "—".into(), |x| format!("{x:.3}"))) " (target " (format!("{t:.3}")) ") " (if *ok { "✓" } else { "✗" }) }
                            }
                        }
                    } }
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
            section.card.wide {
                @if rows.is_empty() { p.muted { "Nothing yet." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "When" } th { "Who" } th { "Project" } th { "Action" } th { "Detail" } } }
                        tbody { @for (at, who, proj, act, d) in rows {
                            tr { td.small.mono { (at) } td.small { (who) } td.small { (proj.as_deref().unwrap_or("—")) } td { span.chip { (act) } } td.small { (d) } }
                        } }
                    } }
                }
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
                section.card.wide {
                    h2.violet { (icon("task")) span.mono { (fm.form) } span.muted.small { " · " (fm.path) } }
                    p {
                        strong { (compact(fm.starts)) } " started · " strong { (compact(fm.submits)) } " sent · "
                        span class={ "status " (if fm.abandonment() > 0.5 { "bad" } else if fm.abandonment() > 0.25 { "warn" } else { "good" }) } { (pct(fm.abandonment())) " abandoned" }
                        @if fm.submits > 0 { " · " (duration(fm.median_secs)) " to fill (median)" }
                        @if fm.starts < 20 { " " span.chip title="Under 20 starts: the rates move a lot" { "low sample" } }
                    }
                    div.table-scroll { table.plain {
                        thead { tr { th { "Field" } (th_num("Sessions")) (th_num("Last before leaving")) th { span.sr-only { "Share" } } (th_num("Came back to it")) (th_num("Time spent")) } }
                        tbody { @for fl in &fm.fields {
                            @let left = fl.last_before_leaving as f64 / (fm.starts - fm.submits).max(1) as f64;
                            tr {
                                th scope="row" { span.mono { (fl.name) } }
                                td.num { (compact(fl.sessions)) }
                                td.num { (compact(fl.last_before_leaving)) }
                                td.meter { (meter(left)) }
                                td.num { (compact(fl.revisited)) }
                                td.num { (duration(fl.median_ms as f64 / 1000.0)) }
                            }
                        } }
                    } }
                    @if let Some(s) = &fm.abandoned_session { p.small { a href={ "/p/" (p.id) "/replays/" (s) } { (icon("play")) " Replay a session that gave up" } } }
                }
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
            section.card.wide {
                form.settings method="post" action={ "/p/" (p.id) "/timeline" } {
                    label { "Day " input type="date" name="day" value=(today) required; }
                    label.wide { "Note " input type="text" name="body" required maxlength="500" placeholder="Pricing page redesign shipped"; }
                    button type="submit" { "Add" }
                }
            }
            section.card.wide {
                @if marks.is_empty() { p.muted { "Nothing yet." } }
                @else {
                    ol.timeline { @for m in marks {
                        li class={ "tl " (m.kind) } {
                            span.tl-day.mono { (m.day) }
                            span.tl-icon { (icon(mark_icon(&m.kind))) }
                            span.tl-text { (m.text) @if let Some(b) = &m.by { span.muted.small { " — " (b) } } }
                            @if let (Some(id), Some(a)) = (m.id, &m.author_id) {
                                @if admin || a == me {
                                    form.inline method="post" action={ "/p/" (p.id) "/timeline/" (id) "/delete" } { button.ghost type="submit" aria-label="Delete this note" { "Delete" } }
                                }
                            }
                        }
                    } }
                }
            }
        },
    )
}

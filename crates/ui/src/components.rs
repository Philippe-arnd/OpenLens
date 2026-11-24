//! Shared page parts: the filter bar, tiles, sparklines, bars, release comparison, insight cards, the empty state.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

pub(crate) fn filter_bar(project: &str, b: &Board, f: &Filters, action: &str) -> Markup {
    if shared() {
        return html! {};
    }
    let mut envs = b.environments.clone();
    if !envs.iter().any(|e| e == "production") {
        envs.insert(0, "production".into());
    }
    html! {
        form.filters method="get" action={ "/p/" (project) (action) } aria-label="Filters" {
            (filter_fields(&envs, b, f))
            button type="submit" { "Apply" }
        }
    }
}

pub(crate) fn filter_fields(envs: &[String], b: &Board, f: &Filters) -> Markup {
    html! {
            label { "Environment "
                select name="env" {
                    @for e in envs { option value=(e) selected[*e == f.environment] { (e) } }
                }
            }
            label { "Role "
                select name="role" {
                    option value="" { "All roles" }
                    @for r in b.roles.iter().filter_map(|r| r.role.as_deref()) {
                        option value=(r) selected[f.role.as_deref() == Some(r)] { (r) }
                    }
                }
            }
            label { "Release "
                select name="release" {
                    option value="" { "All releases" }
                    @for r in &b.releases {
                        option value=(r.release) selected[f.release.as_deref() == Some(r.release.as_str())] { (r.release) }
                    }
                }
            }
            label.check { input type="checkbox" name="internal" value="1" checked[f.internal]; " Include the team" }
    }
}

pub(crate) fn tile(accent: &str, glyph: &str, label: &str, value: &str, hint: &str) -> Markup {
    html! {
        div class={ "tile " (accent) } {
            div.label { span.chip-icon { (icon(glyph)) } (label) (crate::glossary::info(label)) }
            div.value { (value) }
            div.hint { (hint) }
        }
    }
}

/// A row of tabs (#402, reused by the feature pages #411): plain links,
/// the current one with `aria-current="page"`, so they work with the
/// keyboard and without script, and wrap on narrow screens.
/// `items` are (href, label, current).
pub(crate) fn tabs(aria_label: &str, items: &[(String, &str, bool)]) -> Markup {
    html! {
        nav.section-tabs aria-label=(aria_label) {
            @for (href, label, current) in items {
                a href=(href) aria-current=[current.then_some("page")] { (label) }
            }
        }
    }
}

/// One entry of the timeline of changes (#267).
#[derive(Debug, Clone)]
pub struct Mark {
    /// Day (YYYY-MM-DD).
    pub day: String,
    /// `release`, `change`, `alert` or `note`.
    pub kind: String,
    /// What happened.
    pub text: String,
    /// Note id (notes only).
    pub id: Option<i64>,
    /// Who (changes and notes).
    pub by: Option<String>,
    /// The note author's id (to allow deleting).
    pub author_id: Option<String>,
}

pub(crate) fn mark_icon(kind: &str) -> &'static str {
    match kind {
        "release" => "release",
        "alert" => "alert",
        "note" => "idea",
        _ => "flag",
    }
}

/// The active users bars with the timeline's days marked under them.
pub(crate) fn sparkline_marked(series: &[(String, i64)], marks: &[Mark]) -> Markup {
    let n = series.len().max(1) as f64;
    let w = 280.0;
    let gap = 2.0;
    let bw = (w - gap * (n - 1.0)) / n;
    let on: Vec<(usize, Vec<&Mark>)> = series
        .iter()
        .enumerate()
        .map(|(i, (d, _))| (i, marks.iter().filter(|m| &m.day == d).collect::<Vec<_>>()))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    html! {
        (sparkline(series))
        @if !on.is_empty() {
            svg.marks viewBox={ "0 0 " (w) " 12" } role="img" aria-label="Changes on these days (the timeline lists them)" preserveAspectRatio="none" {
                @for (i, ms) in &on {
                    @let kind = if ms.iter().any(|m| m.kind == "alert") { "alert" } else if ms.iter().any(|m| m.kind == "release") { "release" } else { "change" };
                    circle class={ "mk " (kind) } cx=(format!("{:.1}", *i as f64 * (bw + gap) + bw / 2.0)) cy="6" r="4" {
                        title { @for (j, m) in ms.iter().enumerate() { @if j > 0 { " · " } (m.day) ": " (m.text) } }
                    }
                }
            }
        }
    }
}

pub(crate) fn sparkline(series: &[(String, i64)]) -> Markup {
    let max = series.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1) as f64;
    let n = series.len().max(1) as f64;
    let w = 280.0;
    let h = 56.0;
    let gap = 2.0;
    let bw = (w - gap * (n - 1.0)) / n;
    html! {
        svg.spark viewBox={ "0 0 " (w) " " (h) } role="img" aria-label="Daily active users, last 14 days" preserveAspectRatio="none" {
            @for (i, (date, v)) in series.iter().enumerate() {
                @let bh = ((*v as f64) / max * (h - 4.0)).max(2.0);
                @let last = i + 1 == series.len();
                rect class=(if last { "bar now" } else { "bar" })
                    x=(i as f64 * (bw + gap)) y=(h - bh) width=(bw) height=(bh) rx="2" {
                    title { (date) ": " (compact(*v)) " users" }
                }
            }
        }
    }
}

pub(crate) fn status(rating: &str) -> Markup {
    let (cls, glyph, text) = match rating {
        "good" => ("good", "task", "good"),
        "needs-improvement" => ("warn", "alert", "needs work"),
        "poor" => ("bad", "ban", "poor"),
        _ => ("muted", "alert", "unknown"),
    };
    html! { span class={ "status " (cls) } { (self::icon(glyph)) (text) } }
}

/// A horizontal meter filled to `frac` (0..1). Drawn as SVG so the length
/// is an attribute, not an inline style: the dashboard CSP forbids those.
pub(crate) fn meter(frac: f64) -> Markup {
    let w = format!("{:.1}%", (frac.clamp(0.0, 1.0) * 100.0).max(1.5));
    html! {
        svg.fill aria-hidden="true" focusable="false" { rect width=(w) height="6" rx="3" {} }
    }
}

pub(crate) fn bars(rows: &[(String, i64, String)]) -> Markup {
    let max = rows.iter().map(|r| r.1).max().unwrap_or(0).max(1) as f64;
    html! {
        table.bars {
            tbody {
                @for (name, v, extra) in rows {
                    tr {
                        th scope="row" { (name) }
                        td.meter {
                            (meter((*v as f64) / max))
                        }
                        td.num { (compact(*v)) }
                        td.num.muted { (extra) }
                    }
                }
            }
        }
    }
}

/// One compared metric: label, new value, old value, whether higher is
/// better, and how to format it.
pub(crate) type Compared = (&'static str, f64, f64, bool, fn(f64) -> String);

/// "Latest vs previous release" chips: arrow, change, and whether the change
/// is good (green) or bad (red) for that metric — never colour alone.
pub(crate) fn release_compare(rel: &[openlens_engine::ReleaseRow]) -> Markup {
    let [new, old, ..] = rel else { return html! {} };
    if new.sessions < 8 || old.sessions < 8 {
        return html! { p.muted.small { "Comparison appears once both of the latest releases have at least 8 sessions." } };
    }
    // (label, new, old, higher_is_better, formatter)
    let rows: [Compared; 4] = [
        ("Avg session", new.avg_secs, old.avg_secs, true, duration),
        (
            "Tasks / session",
            new.tasks_per_session,
            old.tasks_per_session,
            true,
            |v| format!("{v:.2}"),
        ),
        ("Bounce", new.bounce_rate, old.bounce_rate, false, pct),
        (
            "Sessions with errors",
            new.error_rate,
            old.error_rate,
            false,
            pct,
        ),
    ];
    html! {
        p.small { strong { (new.release) } " compared with " strong { (old.release) } ":" }
        ul.deltas {
            @for (label, n, o, up_good, fmt) in rows {
                @let change = if o.abs() > f64::EPSILON { (n - o) / o } else { 0.0 };
                @let flat = change.abs() < 0.05;
                @let good = (n > o) == up_good;
                @let (cls, arrow, word) = if flat { ("flat", "release", "about the same") } else if good { ("up", if n > o { "up" } else { "down" }, "better") } else { ("down", if n > o { "up" } else { "down" }, "worse") };
                li class=(cls) {
                    span.label { (label) }
                    " " strong { (fmt(n)) } " "
                    span.change { (icon(arrow)) (format!("{:+.0}%", change * 100.0)) " · " (word) }
                    span.muted { " (was " (fmt(o)) ")" }
                }
            }
        }
    }
}

pub(crate) fn insight_cards(pid: &str, b: &Board, plain: &[(String, String)]) -> Markup {
    let accent = |kind: &str| match kind {
        "stick_driver" => ("mint", "star"),
        "value_moment" => ("mint", "spark"),
        "flag_win" => ("mint", "up"),
        "churn_point" => ("coral", "down"),
        "friction" | "funnel_drop" => ("coral", "funnel"),
        "role_blind_spot" => ("violet", "shield"),
        "release_regression" | "flag_regression" => ("sun", "alert"),
        _ => ("sky", "bolt"),
    };

    html! {
        section.insights aria-label="What to focus on" {
            h2.violet { (icon("idea")) "What to focus on" }
            @if b.insights.is_empty() {
                div.insight.waiting {
                    @if b.readiness.judged_users < b.readiness.needed {
                        p { strong { "Insights need a little history." } " They compare users who stayed with users who left, so they wait until "
                            (b.readiness.needed) " users are older than the churn window (now " (b.readiness.judged_users) ")." }
                        p.muted.small { "Backfilling past users and success events (" code { "openlens-cli import" } ") gets there on day one." }
                    } @else {
                        p { strong { "Nothing stands out right now." } " No feature, role, release or page differs enough from the rest to be worth your attention." }
                    }
                }
            } @else {
                ol.cards {
                    @for (i, c) in b.insights.iter().enumerate() {
                        @let (cls, glyph) = accent(c.kind);
                        li class={ "insight " (cls) } {
                            span.rank aria-hidden="true" { (icon(glyph)) }
                            div {
                                @let pl = plain.get(i).filter(|x| x.0 != c.title || x.1 != c.next_step);
                                @if let Some((t, _)) = pl {
                                    h3 title={ "Original: " (c.title) } { span.sr-only { "Insight " (i + 1) ": " } (t) }
                                } @else {
                                    h3 { span.sr-only { "Insight " (i + 1) ": " } (c.title) }
                                }
                                p.evidence { (c.evidence) }
                                p.next title=[pl.map(|_| format!("Original: {}", c.next_step))] { span.label { "Next step" } " " (pl.map_or(c.next_step.as_str(), |x| x.1.as_str())) }
                                @if c.kind == "churn_point" {
                                    @if !shared() { p { a.button href={ "/p/" (pid) "/watch?churned_at=" (url_enc(&c.subject)) } { (icon("play")) "Watch a session that churned here" } } }
                                }
                                @if !shared() { p.small { a href={ "/p/" (pid) "/tickets/new?kind=insight&subject=" (url_enc(&format!("{}:{}", c.kind, c.subject))) } { "Create ticket →" } } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Empty state: no events yet. Shows exactly what the agent (or a human) must add.
pub(crate) fn empty(p: &ProjectInfo<'_>, host: &str) -> Markup {
    let tag = format!(
        r#"<script src="{host}/c.js" data-key="{}" defer></script>"#,
        p.key
    );
    let ask = format!(
        "Connect this app to OpenLens. Follow {host}/agents.md — project key {}.",
        p.key
    );
    html! {
        section.empty {
            div.blob aria-hidden="true" {}
            h1 { "Your board is warming up" }
            p { "No events yet for " strong { (p.name) } ". Nothing to configure here: your coding agent wires the app, drafts the feature map and backfills history." }
            h2 { "Ask your agent" }
            pre.copy { (ask) }
            h2 { "Or add one tag yourself" }
            pre.copy { (tag) }
            p.muted { "Then call " code { "openlens.identify(user.id, { role })" } " once the user is known. This page fills in as soon as the first events land." }
        }
    }
}

//! Web dashboard: server-rendered HTML (maud) and SVG charts, no client-side
//! JavaScript (D-011).
//!
//! Minimal, colourful and efficient, with a playful touch: stat tiles that
//! tilt a little on hover, a lens logo that blinks, friendly copy. Motion
//! respects `prefers-reduced-motion`; dark mode follows the system.
//!
//! Layer: L4 (see `docs/architecture.md`).

use maud::{DOCTYPE, Markup, PreEscaped, html};

mod icons;
pub use icons::icon;
use openlens_engine::{Board, EventRow, Filters};

/// Stylesheet, served at `/assets/app.css`.
pub const CSS: &str = include_str!("../assets/app.css");

/// Project summary used in the header and empty states.
pub struct ProjectInfo<'a> {
    /// Id.
    pub id: &'a str,
    /// Display name.
    pub name: &'a str,
    /// Capture key.
    pub key: &'a str,
}

/// Format a count compactly: 1,284 / 12.9K / 4.2M.
pub fn compact(n: i64) -> String {
    let a = n.unsigned_abs() as f64;
    let sign = if n < 0 { "-" } else { "" };
    if a < 10_000.0 {
        let s = n.unsigned_abs().to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        format!("{sign}{out}")
    } else if a < 1_000_000.0 {
        format!("{sign}{:.1}K", a / 1_000.0)
    } else {
        format!("{sign}{:.1}M", a / 1_000_000.0)
    }
}

/// Format a ratio as a percentage.
pub fn pct(r: f64) -> String {
    if r > 0.0 && r < 0.001 {
        "<0.1%".into()
    } else {
        format!("{:.1}%", r * 100.0)
    }
}

/// Format seconds as "4m 12s".
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

fn layout(title: &str, project: Option<&ProjectInfo<'_>>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) " · OpenLens" }
                link rel="stylesheet" href="/assets/app.css";
            }
            body {
                (icons::sprite())
                header.top {
                    a.brand href="/" aria-label="OpenLens home" {
                        span.lens aria-hidden="true" { span.pupil {} }
                        span { "OpenLens" }
                    }
                    @if let Some(p) = project {
                        nav aria-label="Project" {
                            a href={ "/p/" (p.id) } { (icon("board")) "Focus board" }
                            a href={ "/p/" (p.id) "/events" } { (icon("events")) "Events" }
                            a href={ "/p/" (p.id) "/pages" } { (icon("layers")) "Pages" }
                            a href={ "/p/" (p.id) "/errors" } { (icon("alert")) "Errors" }
                            a href={ "/p/" (p.id) "/flags" } { (icon("flag")) "Flags" }
                        }
                        span.project { (p.name) }
                    }
                    nav.theme aria-label="Theme" style=(if project.is_none() { "margin-left:auto" } else { "" }) {
                        a.auto href="/theme/auto" title="Follow the system" aria-label="Theme: follow the system" { (icon("auto")) }
                        a.light href="/theme/light" title="Light" aria-label="Theme: light" { (icon("sun")) }
                        a.dark href="/theme/dark" title="Dark" aria-label="Theme: dark" { (icon("moon")) }
                    }
                    @if project.is_some() {
                        form method="post" action="/logout" { button.ghost type="submit" { (icon("logout")) "Log out" } }
                    }
                }
                main { (body) }
                footer { "OpenLens · map → measure → focus → act → compare" }
            }
        }
    }
}

/// Login page.
pub fn login(failed: bool) -> Markup {
    layout(
        "Log in",
        None,
        html! {
            section.login {
                h1 { "Hello again " span.wave aria-hidden="true" { "👋" } }
                p.muted { "Paste the admin token, or open the login link your agent sent you." }
                @if failed { p.alert role="alert" { "That token didn't work. Try again?" } }
                form method="post" action="/login" {
                    label for="token" { "Admin token" }
                    input #token name="token" type="password" autocomplete="current-password" required autofocus;
                    button type="submit" { "Open my board" }
                }
            }
        },
    )
}

fn filter_bar(project: &str, b: &Board, f: &Filters, action: &str) -> Markup {
    let mut envs = b.environments.clone();
    if !envs.iter().any(|e| e == "production") {
        envs.insert(0, "production".into());
    }
    html! {
        form.filters method="get" action={ "/p/" (project) (action) } aria-label="Filters" {
            label { "Environment "
                select name="env" {
                    @for e in &envs { option value=(e) selected[*e == f.environment] { (e) } }
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
            button type="submit" { "Apply" }
        }
    }
}

fn tile(accent: &str, glyph: &str, label: &str, value: &str, hint: &str) -> Markup {
    html! {
        div class={ "tile " (accent) } {
            div.label { span.chip-icon { (icon(glyph)) } (label) }
            div.value { (value) }
            div.hint { (hint) }
        }
    }
}

fn sparkline(series: &[(String, i64)]) -> Markup {
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

fn status(rating: &str) -> Markup {
    let (cls, glyph, text) = match rating {
        "good" => ("good", "task", "good"),
        "needs-improvement" => ("warn", "alert", "needs work"),
        "poor" => ("bad", "ban", "poor"),
        _ => ("muted", "alert", "unknown"),
    };
    html! { span class={ "status " (cls) } { (self::icon(glyph)) (text) } }
}

fn bars(rows: &[(String, i64, String)]) -> Markup {
    let max = rows.iter().map(|r| r.1).max().unwrap_or(0).max(1) as f64;
    html! {
        table.bars {
            tbody {
                @for (name, v, extra) in rows {
                    tr {
                        th scope="row" { (name) }
                        td.meter {
                            span.fill style={ "width:" (format!("{:.1}", (*v as f64) / max * 100.0)) "%" } {}
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
type Compared = (&'static str, f64, f64, bool, fn(f64) -> String);

/// "Latest vs previous release" chips: arrow, change, and whether the change
/// is good (green) or bad (red) for that metric — never colour alone.
fn release_compare(rel: &[openlens_engine::ReleaseRow]) -> Markup {
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

fn insight_cards(b: &Board) -> Markup {
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
                                h3 { span.sr-only { "Insight " (i + 1) ": " } (c.title) }
                                p.evidence { (c.evidence) }
                                p.next { span.label { "Next step" } " " (c.next_step) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Empty state: no events yet. Shows exactly what the agent (or a human) must add.
fn empty(p: &ProjectInfo<'_>, host: &str) -> Markup {
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

/// The Focus board (M1: KPI strip, activity, features, roles, releases).
pub fn board(p: &ProjectInfo<'_>, b: &Board, f: &Filters, host: &str) -> Markup {
    if b.total_events == 0 {
        return layout("Focus board", Some(p), empty(p, host));
    }
    let feats: Vec<_> = b
        .features
        .iter()
        .filter(|x| x.users > 0)
        .map(|x| {
            let extra = if x.declared && x.completed > 0 {
                format!("{} uses · {} done", compact(x.uses), compact(x.completed))
            } else {
                format!("{} uses", compact(x.uses))
            };
            (
                x.name.clone().unwrap_or_else(|| x.key.clone()),
                x.users,
                extra,
            )
        })
        .collect();
    let unused: Vec<_> = b
        .features
        .iter()
        .filter(|x| x.declared && x.users == 0)
        .collect();
    let roles: Vec<_> = b
        .roles
        .iter()
        .map(|r| {
            (
                r.role.clone().unwrap_or_else(|| "not identified".into()),
                r.users,
                String::new(),
            )
        })
        .collect();
    let scope = match (&f.role, &f.release) {
        (Some(r), Some(v)) => format!("{r} · {v}"),
        (Some(r), None) => r.clone(),
        (None, Some(v)) => v.clone(),
        (None, None) => "everyone".into(),
    };
    layout(
        "Focus board",
        Some(p),
        html! {
            div.heading {
                h1 { "Focus board" }
                p.muted { "What " (scope) " did lately in " strong { (f.environment) } "." }
            }
            (filter_bar(p.id, b, f, ""))
            (insight_cards(b))
            section.tiles aria-label="Key numbers" {
                (tile("violet", "users", "Daily active users", &compact(b.users.dau), &format!("{} weekly · {} monthly", compact(b.users.wau), compact(b.users.mau))))
                (tile("mint", "spark", "Stickiness", &pct(b.users.stickiness), "DAU ÷ MAU"))
                (tile("sky", "clock", "Avg session", &duration(b.sessions.avg_secs), &format!("{} sessions · {} bounce", compact(b.sessions.count), pct(b.sessions.bounce_rate))))
                (tile("sun", "task", "Tasks per user per day", &format!("{:.1}", b.sessions.tasks_per_user_day), "feature uses, last 7 days"))
                (tile("coral", "alert", "Sessions with errors", &pct(b.sessions.error_rate), "last 7 days"))
            }
            div.grid {
                section.card {
                    h2.violet { (icon("pulse")) "Active users" }
                    (sparkline(&b.dau_series))
                    p.muted.small { "Last 14 days. Hover a bar for the day." }
                }
                section.card {
                    h2.sky { (icon("bolt")) "Web Vitals (p75)" }
                    @if b.vitals.is_empty() { p.muted { "No measurements yet. They arrive when visitors leave a page." } }
                    @else {
                        table.plain {
                            tbody { @for v in &b.vitals {
                                tr {
                                    th scope="row" { (v.name) }
                                    td.num { @if v.name == "CLS" { (format!("{:.3}", v.p75)) } @else { (format!("{:.0} ms", v.p75)) } }
                                    td { (status(&v.rating)) }
                                    td.num.muted { (compact(v.samples)) " samples" }
                                }
                            } }
                        }
                    }
                }
                section.card {
                    h2.sun { (icon("layers")) "Most used features" }
                    @if feats.is_empty() { p.muted { "No feature has been used yet. Features come from " code { "data-feature" } " attributes and the feature map." } }
                    @else { (bars(&feats)) p.muted.small { "By distinct users, last 30 days." } }
                }
                @if !unused.is_empty() {
                    section.card {
                        h2.coral { (icon("ban")) "Never used (last 30 days)" }
                        ul.tags { @for x in &unused { li { (x.name.as_deref().unwrap_or(&x.key)) } } }
                        p.muted.small { "Declared in the feature map, but nobody reached them. Hidden, broken, or not needed?" }
                    }
                }
                @if !b.role_gaps.is_empty() {
                    section.card {
                        h2.sun { (icon("gap")) "Role gaps" }
                        p.small { "These raw role values match no role in the feature map, so they are counted apart:" }
                        ul.tags { @for (raw, n) in &b.role_gaps { li { code { (raw) } " · " (compact(*n)) " users" } } }
                        p.muted.small { "Ask your agent to map them under " code { "roles:" } " in openlens.features.yaml." }
                    }
                }
                section.card {
                    h2.mint { (icon("shield")) "Who uses the app" }
                    (bars(&roles))
                    p.muted.small { "Distinct users per role, last 30 days." }
                }
                @if !b.funnels.is_empty() {
                    section.card.wide {
                        h2.sky { (icon("funnel")) "Funnels" }
                        @for fun in &b.funnels {
                            h3.small { (fun.feature) }
                            table.bars {
                                tbody { @for (i, st) in fun.steps.iter().enumerate() {
                                    tr {
                                        th scope="row" { span.muted { (i + 1) ". " } (st.event) }
                                        td.meter { span.fill style={ "width:" (format!("{:.1}", st.conversion * 100.0)) "%" } {} }
                                        td.num { (compact(st.users)) }
                                        td.num.muted { (pct(st.conversion)) @if i > 0 { " · " (duration(st.median_secs)) } }
                                    }
                                } }
                            }
                        }
                        p.muted.small { "From each feature's declared steps, in order, within a day, last 30 days. Time is the median from the previous step." }
                    }
                }
                @if !b.flag_cohorts.is_empty() {
                    section.card.wide {
                        h2.violet { (icon("flag")) "Flags: with vs without" }
                        table.plain {
                            thead { tr { th { "Flag" } th.num { "Users with / without" } th.num { "Avg session" } th.num { "Tasks / session" } th.num { "Sessions with errors" } } }
                            tbody { @for c in &b.flag_cohorts {
                                tr {
                                    th scope="row" { code { (c.flag) } }
                                    td.num { (compact(c.with.users)) " / " (compact(c.without.users)) }
                                    td.num { (duration(c.with.avg_secs)) " / " (duration(c.without.avg_secs)) }
                                    td.num { (format!("{:.2}", c.with.tasks_per_session)) " / " (format!("{:.2}", c.without.tasks_per_session)) }
                                    td.num { (pct(c.with.error_rate)) " / " (pct(c.without.error_rate)) }
                                }
                            } }
                        }
                        p.muted.small { "Sessions where the flag was on, against all other sessions, last 30 days." }
                    }
                }
                @if !b.journeys.is_empty() {
                    section.card.wide {
                        h2.mint { (icon("route")) "Paths to value" }
                        p.muted.small { "What users did just before reaching each success for the first time (last 30 days)." }
                        ul.journeys {
                            @for j in &b.journeys {
                                li {
                                    @for step in &j.path { span.step { (step) } span.arrow aria-hidden="true" { "→" } }
                                    span.step.done { "✓ " (j.success) }
                                    span.muted { " " (compact(j.users)) " users · " (pct(j.share)) " of those who got there" }
                                }
                            }
                        }
                    }
                }
                section.card.wide {
                    h2.coral { (icon("release")) "Releases" }
                    @if b.releases.is_empty() { p.muted { "No release tagged yet. Pass " code { "release" } " to the SDK to compare releases." } }
                    @else {
                        (release_compare(&b.releases))
                        table.plain {
                            thead { tr { th { "Release" } th.num { "Users" } th.num { "Sessions" } th.num { "Avg session" } th.num { "Bounce" } th.num { "Tasks / session" } th.num { "Sessions with errors" } th { "First seen" } } }
                            tbody { @for (i, r) in b.releases.iter().enumerate() {
                                tr {
                                    th scope="row" { (r.release) @if i == 0 { " " span.badge { "latest" } } }
                                    td.num { (compact(r.users)) }
                                    td.num { (compact(r.sessions)) }
                                    td.num { (duration(r.avg_secs)) }
                                    td.num { (pct(r.bounce_rate)) }
                                    td.num { (format!("{:.2}", r.tasks_per_session)) }
                                    td.num { (pct(r.error_rate)) }
                                    td.muted { (r.first_seen.get(..16).unwrap_or(&r.first_seen)) }
                                }
                            } }
                        }
                    }
                }
            }
            p.muted.small.center {
                @match b.map_version {
                    Some(v) => { "Feature map v" (v) " in use. Cards compare users who stayed with users who churned; each shows the cohort sizes behind it." }
                    None => { "No feature map yet: roles are raw and features come only from data-feature. Ask your agent to push openlens.features.yaml for sharper insights." }
                }
            }
        },
    )
}

/// Recent events.
pub fn events(p: &ProjectInfo<'_>, b: &Board, rows: &[EventRow], f: &Filters) -> Markup {
    layout(
        "Events",
        Some(p),
        html! {
            div.heading { h1 { "Recent events" } p.muted { "The latest 100, newest first." } }
            (filter_bar(p.id, b, f, "/events"))
            section.card.wide {
                @if rows.is_empty() { p.muted { "Nothing here yet." } }
                @else {
                    table.plain.events {
                        thead { tr { th { "When (UTC)" } th { "Event" } th { "User" } th { "Role" } th { "Page" } th { "Feature" } th { "Release" } } }
                        tbody { @for r in rows {
                            tr {
                                td.muted.mono { (r.timestamp.get(..19).unwrap_or(&r.timestamp)) }
                                td { span.chip { (r.event) } }
                                td.mono { (r.distinct_id) }
                                td { (r.role.as_deref().unwrap_or("—")) }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.feature.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// Feature flags: switch on/off and change the rollout. Creating flags is
/// done by agents (API/CLI/MCP) with the code that uses them.
pub fn flags(p: &ProjectInfo<'_>, flags: &[openlens_flags::Flag]) -> Markup {
    layout(
        "Flags",
        Some(p),
        html! {
            div.heading {
                h1 { "Flags" }
                p.muted { "Ship a change to some users first, then compare. Each flag's users show up as their own cohort." }
            }
            @if flags.is_empty() {
                section.card { p { "No flags yet. Your agent creates them together with the code that checks them: "
                    code { "openlens-cli flags set new-editor --rollout 10" } "." } }
            } @else {
                section.card.wide {
                    table.plain {
                        thead { tr { th { "Flag" } th { "Who" } th { "Rollout" } th { "On" } th {} } }
                        tbody { @for f in flags {
                            tr {
                                th scope="row" { code { (f.key) } @if !f.description.is_empty() { br; span.muted.small { (f.description) } } }
                                td.small { @if f.roles.is_empty() { "every role" } @else { (f.roles.join(", ")) }
                                    @if !f.accounts.is_empty() { br; span.muted { "+ " (f.accounts.len()) " accounts" } } }
                                td { form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) } {
                                    @if f.enabled { input type="hidden" name="enabled" value="on"; }
                                    input type="number" name="rollout" min="0" max="100" value=(f.rollout) aria-label={ "Rollout percentage of " (f.key) };
                                    " % " button.ghost type="submit" { "Save" }
                                } }
                                td { span class=(if f.enabled { "status good" } else { "status muted" }) {
                                    span aria-hidden="true" { (if f.enabled { "●" } else { "○" }) } " " (if f.enabled { "on" } else { "off" }) } }
                                td { form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) } {
                                    @if !f.enabled { input type="hidden" name="enabled" value="on"; }
                                    input type="hidden" name="rollout" value=(f.rollout);
                                    button type="submit" { (if f.enabled { "Switch off" } else { "Switch on" }) }
                                } }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// Per-page hotspots: scroll depth, most clicked elements, rage clicks.
pub fn pages(
    p: &ProjectInfo<'_>,
    b: &Board,
    list: &[openlens_engine::PageStats],
    f: &Filters,
) -> Markup {
    layout(
        "Pages",
        Some(p),
        html! {
            div.heading { div { h1 { "Pages" } p.muted { "How far people scroll, what they click, where they get frustrated. Last 30 days." } } }
            (filter_bar(p.id, b, f, "/pages"))
            @if list.is_empty() { section.card { p.muted { "No page views yet." } } }
            div.grid {
                @for pg in list {
                    section.card {
                        h2.sky { (icon("layers")) span.mono { (pg.path) } }
                        p.small.muted { (compact(pg.views)) " views · " (compact(pg.users)) " users" }
                        @if pg.median_scroll >= 0.0 {
                            table.bars { tbody {
                                tr { th scope="row" { "Reach half the page" } td.meter { span.fill style={ "width:" (format!("{:.0}", pg.reach_half * 100.0)) "%" } {} } td.num { (pct(pg.reach_half)) } }
                                tr { th scope="row" { "Reach the end" } td.meter { span.fill style={ "width:" (format!("{:.0}", pg.reach_end * 100.0)) "%" } {} } td.num { (pct(pg.reach_end)) } }
                            } }
                        }
                        @if !pg.hotspots.is_empty() {
                            h3.small { "Most clicked" }
                            table.plain { tbody { @for h in &pg.hotspots {
                                tr {
                                    th scope="row" { (h.label) }
                                    td.num { (compact(h.clicks)) }
                                    td.num { @if h.rage > 0 { span.status.bad { (icon("alert")) (compact(h.rage)) " rage" } } }
                                }
                            } } }
                        }
                    }
                }
            }
        },
    )
}

/// Error groups, most users first.
pub fn errors(
    p: &ProjectInfo<'_>,
    b: &Board,
    groups: &[openlens_engine::ErrorGroup],
    f: &Filters,
) -> Markup {
    let list = |v: &[(String, i64)]| {
        v.iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    layout(
        "Errors",
        Some(p),
        html! {
            div.heading {
                div { h1 { "Errors" } p.muted { "Uncaught errors grouped by cause, most users first, last 30 days." } }
            }
            (filter_bar(p.id, b, f, "/errors"))
            @if groups.is_empty() {
                section.card { p { (icon("task")) " No errors in the last 30 days. Nice." } }
            } @else {
                section.card.wide {
                    table.plain {
                        thead { tr { th { "Error" } th.num { "Users" } th.num { "Sessions" } th.num { "Times" } th { "Roles" } th { "Releases" } th { "Where" } th { "Last seen" } } }
                        tbody { @for g in groups {
                            tr {
                                th scope="row" {
                                    span.mono { (g.sample.get(..160).unwrap_or(&g.sample)) }
                                    br; span.muted.small.mono { (g.fingerprint.rsplit(" @ ").next().unwrap_or("")) }
                                }
                                td.num { strong { (compact(g.users)) } }
                                td.num { (compact(g.sessions)) }
                                td.num.muted { (compact(g.count)) }
                                td.small { (list(&g.roles)) }
                                td.small { (list(&g.releases)) }
                                td.small { (list(&g.places)) }
                                td.small.muted { (g.last_seen.get(..16).unwrap_or(&g.last_seen)) }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// Generic message page (errors, not found).
pub fn message(title: &str, text: &str) -> Markup {
    layout(
        title,
        None,
        html! { section.login { h1 { (title) } p { (text) } p { a href="/" { "Back to the board" } } } },
    )
}

/// Raw HTML escape hatch for tests.
pub fn render(m: Markup) -> String {
    let PreEscaped(s) = m;
    s
}

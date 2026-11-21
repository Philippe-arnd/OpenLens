//! Web dashboard: server-rendered HTML (maud) and SVG charts, no client-side
//! JavaScript (D-011).
//!
//! Minimal, colourful and efficient, with a playful touch: stat tiles that
//! tilt a little on hover, a lens logo that blinks, friendly copy. Motion
//! respects `prefers-reduced-motion`; dark mode follows the system.
//!
//! Layer: L4 (see `docs/architecture.md`).

use maud::{DOCTYPE, Markup, PreEscaped, html};
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
                header.top {
                    a.brand href="/" aria-label="OpenLens home" {
                        span.lens aria-hidden="true" { span.pupil {} }
                        span { "OpenLens" }
                    }
                    @if let Some(p) = project {
                        nav aria-label="Project" {
                            a href={ "/p/" (p.id) } { "Focus board" }
                            a href={ "/p/" (p.id) "/events" } { "Events" }
                        }
                        span.project { (p.name) }
                        form method="post" action="/logout" { button.ghost type="submit" { "Log out" } }
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

fn tile(accent: &str, label: &str, value: &str, hint: &str) -> Markup {
    html! {
        div class={ "tile " (accent) } {
            div.label { (label) }
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
    let (cls, icon, text) = match rating {
        "good" => ("good", "✓", "good"),
        "needs-improvement" => ("warn", "!", "needs work"),
        "poor" => ("bad", "✕", "poor"),
        _ => ("muted", "?", "unknown"),
    };
    html! { span class={ "status " (cls) } { span aria-hidden="true" { (icon) } " " (text) } }
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

fn insight_cards(b: &Board) -> Markup {
    let accent = |kind: &str| match kind {
        "stick_driver" | "value_moment" => ("mint", "★"),
        "churn_point" | "friction" => ("coral", "↘"),
        "role_blind_spot" => ("violet", "◐"),
        "release_regression" => ("sun", "!"),
        _ => ("sky", "⏱"),
    };
    html! {
        section.insights aria-label="What to focus on" {
            h2 { span.dot.violet {} "What to focus on" }
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
                        @let (cls, icon) = accent(c.kind);
                        li class={ "insight " (cls) } {
                            span.rank aria-hidden="true" { (icon) }
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
                (tile("violet", "Daily active users", &compact(b.users.dau), &format!("{} weekly · {} monthly", compact(b.users.wau), compact(b.users.mau))))
                (tile("mint", "Stickiness", &pct(b.users.stickiness), "DAU ÷ MAU"))
                (tile("sky", "Avg session", &duration(b.sessions.avg_secs), &format!("{} sessions · {} bounce", compact(b.sessions.count), pct(b.sessions.bounce_rate))))
                (tile("sun", "Tasks per user per day", &format!("{:.1}", b.sessions.tasks_per_user_day), "feature uses, last 7 days"))
                (tile("coral", "Sessions with errors", &pct(b.sessions.error_rate), "last 7 days"))
            }
            div.grid {
                section.card {
                    h2 { span.dot.violet {} "Active users" }
                    (sparkline(&b.dau_series))
                    p.muted.small { "Last 14 days. Hover a bar for the day." }
                }
                section.card {
                    h2 { span.dot.sky {} "Web Vitals (p75)" }
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
                    h2 { span.dot.sun {} "Most used features" }
                    @if feats.is_empty() { p.muted { "No feature has been used yet. Features come from " code { "data-feature" } " attributes and the feature map." } }
                    @else { (bars(&feats)) p.muted.small { "By distinct users, last 30 days." } }
                }
                @if !unused.is_empty() {
                    section.card {
                        h2 { span.dot.coral {} "Never used (last 30 days)" }
                        ul.tags { @for x in &unused { li { (x.name.as_deref().unwrap_or(&x.key)) } } }
                        p.muted.small { "Declared in the feature map, but nobody reached them. Hidden, broken, or not needed?" }
                    }
                }
                @if !b.role_gaps.is_empty() {
                    section.card {
                        h2 { span.dot.sun {} "Role gaps" }
                        p.small { "These raw role values match no role in the feature map, so they are counted apart:" }
                        ul.tags { @for (raw, n) in &b.role_gaps { li { code { (raw) } " · " (compact(*n)) " users" } } }
                        p.muted.small { "Ask your agent to map them under " code { "roles:" } " in openlens.features.yaml." }
                    }
                }
                section.card {
                    h2 { span.dot.mint {} "Who uses the app" }
                    (bars(&roles))
                    p.muted.small { "Distinct users per role, last 30 days." }
                }
                section.card.wide {
                    h2 { span.dot.coral {} "Releases" }
                    @if b.releases.is_empty() { p.muted { "No release tagged yet. Pass " code { "release" } " to the SDK to compare releases." } }
                    @else {
                        table.plain {
                            thead { tr { th { "Release" } th.num { "Users" } th.num { "Sessions" } th.num { "Sessions with errors" } th { "First seen" } } }
                            tbody { @for (i, r) in b.releases.iter().enumerate() {
                                tr {
                                    th scope="row" { (r.release) @if i == 0 { " " span.badge { "latest" } } }
                                    td.num { (compact(r.users)) }
                                    td.num { (compact(r.sessions)) }
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

//! Web dashboard: server-rendered HTML (maud) and SVG charts, no client-side
//! JavaScript (D-011).
//!
//! Minimal, colourful and efficient, with a playful touch: stat tiles that
//! tilt a little on hover, a colour-blob logo that slowly changes shape, friendly copy. Motion
//! respects `prefers-reduced-motion`; dark mode follows the system.
//!
//! Layer: L4 (see `docs/architecture.md`).

use maud::{DOCTYPE, Markup, PreEscaped, html};

mod icons;
mod world;
pub use icons::icon;
use openlens_engine::{Board, EventRow, Filters};

/// Stylesheet, served at `/assets/app.css`.
pub const CSS: &str = include_str!("../assets/app.css");

/// The favicon: the logo blob, still.
pub const FAVICON: &str = include_str!("../assets/favicon.svg");

/// Project summary used in the header and empty states.
pub struct ProjectInfo<'a> {
    /// Id.
    pub id: &'a str,
    /// Display name.
    pub name: &'a str,
    /// Capture key.
    pub key: &'a str,
}

static UPDATE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Record the newest release when it is newer than this one (the server
/// checks once a day), so every page's footer can say so.
pub fn set_update_available(tag: Option<String>) {
    if let Ok(mut u) = UPDATE.write() {
        *u = tag;
    }
}

fn update_available() -> Option<String> {
    UPDATE.read().ok().and_then(|u| u.clone())
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

/// Percent-encode a query parameter value.
pub fn url_enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Format a ratio as a percentage.
pub fn pct(r: f64) -> String {
    if r > 0.0 && r < 0.001 {
        "<0.1%".into()
    } else {
        format!("{:.1}%", r * 100.0)
    }
}

/// A UTC timestamp (`2026-11-23 10:04:05.123`, `T` or space, optional
/// `Z`) shown in the reader's time zone as `YYYY-MM-DD HH:MM[:SS]` (#215).
/// Unparseable input is shown as it came.
pub fn local_time(ts: &str, tz: &str, seconds: bool) -> String {
    let fmt = if seconds {
        "%Y-%m-%d %H:%M:%S"
    } else {
        "%Y-%m-%d %H:%M"
    };
    let t = ts.trim().trim_end_matches('Z').replace('T', " ");
    let t = t.split('+').next().unwrap_or(&t).to_owned();
    let parsed = chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%d %H:%M"));
    match (parsed, tz.parse::<chrono_tz::Tz>()) {
        (Ok(n), Ok(z)) => n.and_utc().with_timezone(&z).format(fmt).to_string(),
        (Ok(n), Err(_)) => n.format(fmt).to_string(),
        _ => ts
            .get(..if seconds { 19 } else { 16 })
            .unwrap_or(ts)
            .to_owned(),
    }
}

/// Short label of a time zone for headers ("Europe/Paris" → "Paris").
fn tz_label(tz: &str) -> &str {
    tz.rsplit('/').next().unwrap_or(tz)
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

thread_local! {
    static SHARED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Is the page being rendered for a share link (#229)?
fn shared() -> bool {
    SHARED.with(std::cell::Cell::get)
}

/// Render a page for a read-only share link (#229): no sidebar, no
/// filters, no links that need a login; a banner says what it is.
pub fn render_shared(f: impl FnOnce() -> Markup) -> Markup {
    SHARED.with(|s| s.set(true));
    let m = f();
    SHARED.with(|s| s.set(false));
    m
}

fn shared_layout(title: &str, project: Option<&ProjectInfo<'_>>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                meta name="robots" content="noindex";
                title { (title) @if let Some(p) = project { " · " (p.name) } " · OpenLens" }
                link rel="stylesheet" href="/assets/app.css";
                link rel="icon" type="image/svg+xml" href="/assets/favicon.svg";
            }
            body.shared {
                (icons::sprite())
                div.content {
                    p.shared-banner { span.blob aria-hidden="true" {} " Read-only view" @if let Some(p) = project { " of " strong { (p.name) } } " · OpenLens" }
                    main { (body) }
                }
            }
        }
    }
}

fn layout(title: &str, project: Option<&ProjectInfo<'_>>, body: Markup) -> Markup {
    if shared() {
        return shared_layout(title, project, body);
    }
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) " · OpenLens" }
                link rel="stylesheet" href="/assets/app.css";
                link rel="icon" type="image/svg+xml" href="/assets/favicon.svg";
            }
            body {
                (icons::sprite())
                div.shell {
                    aside.side aria-label="OpenLens" {
                        a.brand href="/" title="OpenLens home" aria-label="OpenLens home" {
                            span.blob aria-hidden="true" {}
                            span.label { "OpenLens" }
                        }
                        @if let Some(p) = project {
                            a.switch href="/projects" title={ "Project: " (p.name) " (switch)" } {
                                img.picon src={ "/p/" (p.id) "/icon" } alt="" width="24" height="24";
                                span.label { (p.name) }
                            }
                            nav.side-nav aria-label="Project" {
                                @for (href, glyph, label) in [
                                    ("", "board", "Focus board"),
                                    ("/live", "pulse", "Live"),
                                    ("/uptime", "shield", "Uptime"),
                                    ("/events", "events", "Events"),
                                    ("/pages", "layers", "Pages"),
                                    ("/errors", "alert", "Errors"),
                                    ("/replays", "play", "Replays"),
                                    ("/flags", "flag", "Flags"),
                                    ("/surveys", "idea", "Surveys"),
                                    ("/audience", "globe", "Audience"),
                                    ("/acquisition", "route", "Acquisition"),
                                    ("/forms", "task", "Forms"),
                                    ("/churn", "ban", "Churn"),
                                    ("/goals", "star", "Goals"),
                                    ("/timeline", "clock", "Timeline"),
                                    ("/flows", "funnel", "Feature flow"),
                                    ("/features", "route", "Feature map"),
                                    ("/digest", "mail", "Digest"),
                                    ("/settings", "gear", "Settings"),
                                ] {
                                    a href={ "/p/" (p.id) (href) } title=(label) aria-current=[(title == label || (href.is_empty() && title == p.name)).then_some("page")] {
                                        (icon(glyph)) span.label { (label) }
                                    }
                                }
                            }
                        }
                        div.side-bottom {
                            nav.theme aria-label="Theme" {
                                a.auto href="/theme/auto" title="Follow the system" aria-label="Theme: follow the system" { (icon("auto")) }
                                a.light href="/theme/light" title="Light" aria-label="Theme: light" { (icon("sun")) }
                                a.dark href="/theme/dark" title="Dark" aria-label="Theme: dark" { (icon("moon")) }
                            }
                            @if project.is_some() {
                                a.side-link href="/me" title="My profile" { (icon("users")) span.label { "My profile" } }
                                form method="post" action="/logout" {
                                    button.side-link type="submit" title="Log out" { (icon("logout")) span.label { "Log out" } }
                                }
                            }
                            a.side-link.collapse href="/nav/collapsed" title="Collapse the menu" { (icon("left")) span.label { "Collapse" } }
                            a.side-link.expand href="/nav/expanded" title="Expand the menu" aria-label="Expand the menu" { (icon("right")) }
                        }
                    }
                    div.content {
                        main { (body) }
                        footer {
                            "OpenLens " (env!("CARGO_PKG_VERSION")) " · map → measure → focus → act → compare"
                            @if let Some(v) = update_available() {
                                " · " a href={ "https://github.com/Philippe-arnd/OpenLens/releases/tag/" (v) } { "Update available: " (v) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Login page.
pub fn login(failed: bool, sso: bool, email: bool) -> Markup {
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
                @if sso {
                    p.muted.small { "or" }
                    p { a.button href="/oidc/start" { (icon("shield")) "Sign in with your company account" } }
                }
                @if email {
                    p.muted.small { "or, if you were invited" }
                    form method="post" action="/login/email" {
                        label for="login-email" { "Your email" }
                        input #login-email name="email" type="email" autocomplete="email" required;
                        button type="submit" { "Email me a login link" }
                    }
                }
            }
        },
    )
}

/// First login with the admin token or the agent's link: create your own
/// admin account (named in the team list, and the one SSO signs in to).
pub fn welcome(sso: bool, error: Option<&str>) -> Markup {
    layout(
        "Welcome",
        None,
        html! {
            section.login {
                h1 { "Welcome to OpenLens " span.wave aria-hidden="true" { "🔭" } }
                p.muted { "You are in with the admin token. Create your own admin account, so the team list knows who you are and you can invite others." }
                @if let Some(e) = error { p.alert role="alert" { (e) } }
                form method="post" action="/welcome" {
                    label for="email" { "Your email" }
                    input #email name="email" type="email" autocomplete="email" required autofocus;
                    button type="submit" { "Create my admin account" }
                }
                p.muted.small {
                    @if sso { "Next time, sign in with your company account. " }
                    @else { "Next time, use a login link from your agent (" code { "openlens-cli login-link --email you@… --role admin" } ") or the admin token. " }
                    a href="/" { "Skip for now" }
                }
            }
        },
    )
}

fn filter_bar(project: &str, b: &Board, f: &Filters, action: &str) -> Markup {
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

fn filter_fields(envs: &[String], b: &Board, f: &Filters) -> Markup {
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

fn tile(accent: &str, glyph: &str, label: &str, value: &str, hint: &str) -> Markup {
    html! {
        div class={ "tile " (accent) } {
            div.label { span.chip-icon { (icon(glyph)) } (label) }
            div.value { (value) }
            div.hint { (hint) }
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

fn mark_icon(kind: &str) -> &'static str {
    match kind {
        "release" => "release",
        "alert" => "alert",
        "note" => "idea",
        _ => "flag",
    }
}

/// The active users bars with the timeline's days marked under them.
fn sparkline_marked(series: &[(String, i64)], marks: &[Mark]) -> Markup {
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

/// A horizontal meter filled to `frac` (0..1). Drawn as SVG so the length
/// is an attribute, not an inline style: the dashboard CSP forbids those.
fn meter(frac: f64) -> Markup {
    let w = format!("{:.1}%", (frac.clamp(0.0, 1.0) * 100.0).max(1.5));
    html! {
        svg.fill aria-hidden="true" focusable="false" { rect width=(w) height="6" rx="3" {} }
    }
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

fn insight_cards(pid: &str, b: &Board, plain: &[(String, String)]) -> Markup {
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
/// `plain`: optional plain-language `(title, next step)` per insight card
/// (same order), shown instead of the original, which stays as the tooltip.
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
                p.muted { "What " (scope) " did lately in " strong { (f.environment) } ". " @if !shared() { a href={ "/p/" (p.id) "/digest" } { "This week vs last →" } " · " a href={ "/p/" (p.id) "/breakdown?by=role&env=" (url_enc(&f.environment)) } { "Break down →" } } }
            }
            (filter_bar(p.id, b, f, ""))
            (insight_cards(p.id, b, plain))
            section.tiles aria-label="Key numbers" {
                (tile("violet", "users", "Daily active users", &{
                    if b.users.known > 0 {
                        format!("{} · {}", compact(b.users.dau), pct(b.users.dau_share()))
                    } else {
                        compact(b.users.dau)
                    }
                }, &{
                    let mut sub = String::new();
                    if b.users.known > 0 {
                        sub.push_str(&format!("of {} known users (90 days) · ", compact(b.users.known)));
                    }
                    sub.push_str(&format!("{} weekly · {} monthly ({})", compact(b.users.wau), compact(b.users.mau), pct(b.users.mau_share())));
                    if b.users.resurrected > 0 {
                        sub.push_str(&format!(" · {} came back", compact(b.users.resurrected)));
                    }
                    if b.users.anonymous > 0 {
                        sub.push_str(&format!(" · +{} not logged in", compact(b.users.anonymous)));
                    }
                    sub
                }))
                (tile("mint", "spark", "Stickiness", &pct(b.users.stickiness), "DAU ÷ MAU"))
                (tile("sky", "clock", "Avg session", &duration(b.sessions.avg_secs), &format!("{} sessions · {} bounce", compact(b.sessions.count), pct(b.sessions.bounce_rate))))
                (tile("sun", "task", "Tasks per user per day", &format!("{:.1}", b.sessions.tasks_per_user_day), "feature uses, last 7 days"))
                (tile("coral", "alert", "Sessions with errors", &pct(b.sessions.error_rate), "last 7 days"))
            }
            div.grid {
                section.card {
                    h2.violet { (icon("pulse")) "Active users" }
                    (sparkline_marked(&b.dau_series, marks))
                    p.muted.small { "Last 14 days. Hover a bar for the day" @if !marks.is_empty() && !shared() { ", a dot for what changed (" a href={ "/p/" (p.id) "/timeline" } { "timeline" } ")" } "." }
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
                @if let Some(r) = revenue {
                    section.card {
                        h2.mint { (icon("star")) "Revenue, last 30 days" }
                        table.plain { tbody { @for m in &r.totals {
                            tr { th scope="row" { (money(m.total, &m.currency)) } td.small.muted { (compact(m.payers)) " payers · " (money(m.per_active_user, &m.currency)) " per active user" } }
                        } } }
                        p.small { a href={ "/p/" (p.id) "/revenue" } { "By feature, role and release →" } }
                    }
                }
                (feature_pulse(b, feats.is_empty()))
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
                (role_matrix(b))
                (retention_grid(b))
                @if !b.funnels.is_empty() {
                    section.card.wide {
                        h2.sky { (icon("funnel")) "Funnels" }
                        @for fun in &b.funnels {
                            h3.small { (fun.feature) }
                            table.bars {
                                tbody { @for (i, st) in fun.steps.iter().enumerate() {
                                    tr {
                                        th scope="row" { span.muted { (i + 1) ". " } (st.event) }
                                        td.meter { (meter(st.conversion)) }
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
                            thead { tr { th { "Flag" } th.num { "Users with / without" } th.num { "Avg session" } th.num { "Tasks / session" } th.num { "Sessions with errors" } th { "Confidence" } } }
                            tbody { @for c in &b.flag_cohorts {
                                tr {
                                    th scope="row" { code { (c.flag) } }
                                    td.num { (compact(c.with.users)) " / " (compact(c.without.users)) }
                                    td.num { (duration(c.with.avg_secs)) " / " (duration(c.without.avg_secs)) }
                                    td.num { (format!("{:.2}", c.with.tasks_per_session)) " / " (format!("{:.2}", c.without.tasks_per_session)) }
                                    td.num { (pct(c.with.error_rate)) " / " (pct(c.without.error_rate)) }
                                    td.small { (confidence(match (c.p_errors, c.p_tasks) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) })) }
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
pub fn events(
    p: &ProjectInfo<'_>,
    b: &Board,
    rows: &[EventRow],
    f: &Filters,
    eq: &openlens_engine::EventQuery,
    next: Option<&str>,
) -> Markup {
    let mut envs = b.environments.clone();
    if !envs.iter().any(|e| e == "production") {
        envs.insert(0, "production".into());
    }
    let sort = eq.sort.as_deref().unwrap_or("time");
    let days = eq.days.unwrap_or(30);
    layout(
        "Events",
        Some(p),
        html! {
            div.heading { h1 { "Events" } p.muted {
                @match sort { "event" => "By event name", "person" => "By user", _ => "Newest first" }
                ", last " (days) (if days == 1 { " day" } else { " days" }) ", 100 per page. Times in " (f.tz) "."
            } }
            form.filters method="get" action={ "/p/" (p.id) "/events" } aria-label="Filters" {
                (filter_fields(&envs, b, f))
                label { "Family "
                    select name="family" {
                        option value="" { "All families" }
                        @for (k, label, _) in openlens_engine::FAMILIES {
                            option value=(k) selected[eq.family.as_deref() == Some(*k)] { (label) }
                        }
                    }
                }
                label { "Event " input type="text" name="event" value=[eq.event.as_deref()] placeholder="any" maxlength="200"; }
                label { "User " input type="text" name="person" value=[eq.person.as_deref()] placeholder="any" maxlength="200"; }
                label { "Page " input type="text" name="path" value=[eq.path.as_deref()] placeholder="/settings" maxlength="200"; }
                label { "Period "
                    select name="days" {
                        @for (d, l) in [(1, "24 hours"), (7, "7 days"), (30, "30 days"), (90, "90 days")] {
                            option value=(d) selected[days == d] { (l) }
                        }
                    }
                }
                label { "Sort "
                    select name="sort" {
                        option value="time" selected[sort == "time"] { "Newest first" }
                        option value="event" selected[sort == "event"] { "Event name" }
                        option value="person" selected[sort == "person"] { "User" }
                    }
                }
                button type="submit" { "Apply" }
            }
            section.card.wide {
                @if rows.is_empty() { p.muted { "Nothing matches. Widen the period or clear a filter." } }
                @else {
                    table.plain.events {
                        thead { tr { th { "When (" (tz_label(&f.tz)) ")" } th { "Event" } th { "User" } th { "Role" } th { "Page" } th { "Feature" } th { "Release" } th { span.sr-only { "Replay" } } } }
                        tbody { @for r in rows {
                            @let fam = openlens_engine::family(&r.event);
                            tr {
                                td.muted.mono { (local_time(&r.timestamp, &f.tz, true)) }
                                td { span class={ "chip fam " (fam) } title=(family_label(fam)) { (icon(family_icon(fam))) span.sr-only { (family_label(fam)) ": " } (r.event) } }
                                td.mono { a href={ "/p/" (p.id) "/people/" (url_enc(&r.distinct_id)) } title="Profile" { (r.distinct_id) } }
                                td { (r.role.as_deref().unwrap_or("—")) }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.feature.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                                td { @if let Some(s) = &r.session_id {
                                    a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (r.timestamp) } title="Replay this moment" aria-label="Replay this moment" { (icon("play")) }
                                } }
                            }
                        } }
                    }
                    @if let Some(n) = next { p { a.button href=(n) { "Older / next 100" (icon("right")) } } }
                }
            }
        },
    )
}

fn family_icon(fam: &str) -> &'static str {
    match fam {
        "page" => "route",
        "click" => "bolt",
        "form" => "task",
        "feature" => "star",
        "identify" => "users",
        "error" => "alert",
        "vital" => "pulse",
        "survey" => "idea",
        "custom" => "spark",
        _ => "events",
    }
}

fn family_label(fam: &str) -> &'static str {
    openlens_engine::FAMILIES
        .iter()
        .find(|(k, _, _)| *k == fam)
        .map_or("Other", |(_, l, _)| l)
}

/// A person's profile (#222): who they are, their role over time, their
/// recordings, their latest events and errors.
#[allow(clippy::too_many_arguments)]
pub fn person(
    p: &ProjectInfo<'_>,
    who: &str,
    person: Option<&openlens_meta::Person>,
    roles: &[(String, String)],
    replays: &[openlens_replay::Summary],
    events: &[EventRow],
    tz: &str,
    admin: bool,
) -> Markup {
    let errors: Vec<&EventRow> = events
        .iter()
        .filter(|e| openlens_engine::family(&e.event) == "error")
        .collect();
    let tf = |t: &str| local_time(t, tz, false);
    layout(
        "Person",
        Some(p),
        html! {
            div.heading { div {
                h1.mono { (who) }
                p.muted {
                    @if let Some(x) = person {
                        @if let Some(r) = &x.role { span.chip { (r) } " " }
                        @if let Some(a) = &x.account { "account " strong { (a) } " · " }
                        @if let Some(pl) = &x.plan { "plan " strong { (pl) } " · " }
                        "first seen " (tf(&x.first_seen)) " · last login " (tf(&x.last_seen))
                    } @else { "Never logged in (anonymous visitor)." }
                    " · " a href={ "/p/" (p.id) "/events?person=" (url_enc(who)) "&days=90" } { "All their events →" }
                }
            } }
            div.grid {
                section.card {
                    h2.violet { (icon("users")) "Role over time" }
                    @if roles.is_empty() { p.muted.small { "No role sent yet." } }
                    @else { ul.plain-list { @for (r, since) in roles { li { span.chip { (r) } " since " (tf(since)) } } } }
                }
                section.card {
                    h2.sky { (icon("play")) "Recordings" }
                    @if replays.is_empty() { p.muted.small { "No recorded session." } }
                    @else { ul.plain-list { @for r in replays.iter().take(20) {
                        li { a href={ "/p/" (p.id) "/replays/" (r.session_id) } { (icon("play")) " " (local_time(&r.started.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)) } " · " (duration((r.updated - r.started).num_seconds().max(0) as f64)) }
                    } } }
                }
                section.card {
                    h2.coral { (icon("alert")) "Errors" }
                    @if errors.is_empty() { p.muted.small { "None in their latest events." } }
                    @else { ul.plain-list { @for e in errors.iter().take(10) {
                        li { (tf(&e.timestamp)) " · " span.mono { (e.path.as_deref().unwrap_or("—")) } @if let Some(r) = &e.release { " · " (r) }
                            @if let Some(s) = &e.session_id { " " a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (e.timestamp) } aria-label="Replay this moment" { (icon("play")) } } }
                    } } }
                    p.small { a href={ "/p/" (p.id) "/errors" } { "All errors →" } }
                }
            }
            @if admin {
                section.card.wide.danger {
                    h2.coral { (icon("shield")) "Privacy" }
                    p { a.button href={ "/p/" (p.id) "/people/" (url_enc(who)) "/export.json" } { (icon("down")) "Export their data (JSON)" } }
                    p.small { "Forgetting removes their events (and those of the anonymous ids linked to them), their recordings and their profile, now, and drops anything they send later. There is no undo." }
                    form.inline method="post" action={ "/p/" (p.id) "/people/" (url_enc(who)) "/forget" } {
                        label { "Type " code { (who) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                        button.danger type="submit" { "Forget this person" }
                    }
                }
            }
            section.card.wide {
                h2.mint { (icon("events")) "Latest events" }
                @if events.is_empty() { p.muted { "No event in 90 days." } }
                @else {
                    div.table-scroll { table.plain.events {
                        thead { tr { th { "When (" (tz_label(tz)) ")" } th { "Event" } th { "Page" } th { "Feature" } th { "Release" } th { span.sr-only { "Replay" } } } }
                        tbody { @for r in events {
                            @let fam = openlens_engine::family(&r.event);
                            tr {
                                td.muted.mono { (local_time(&r.timestamp, tz, true)) }
                                td { span class={ "chip fam " (fam) } { (icon(family_icon(fam))) span.sr-only { (family_label(fam)) ": " } (r.event) } }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.feature.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                                td { @if let Some(s) = &r.session_id {
                                    a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (r.timestamp) } title="Replay this moment" aria-label="Replay this moment" { (icon("play")) }
                                } }
                            }
                        } }
                    } }
                }
            }
        },
    )
}

/// The dimensions a breakdown can use, with their labels.
pub const BREAKDOWNS: &[(&str, &str)] = &[
    ("role", "Role"),
    ("plan", "Plan"),
    ("account", "Account"),
    ("environment", "Environment"),
    ("release", "Release"),
    ("page", "Page"),
    ("feature", "Feature"),
    ("country", "Country"),
    ("device", "Device"),
    ("browser", "Browser"),
    ("os", "OS"),
    ("channel", "Channel"),
];

/// Users, sessions, tasks and errors broken down by one dimension (#222).
pub fn breakdown(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    by: &str,
    rows: &[openlens_engine::BreakdownRow],
) -> Markup {
    let label = BREAKDOWNS
        .iter()
        .find(|(k, _)| *k == by)
        .map_or(by, |(_, l)| l);
    let max = rows.iter().map(|r| r.users).max().unwrap_or(1).max(1) as f64;
    layout(
        "Breakdown",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Break down by " (label.to_lowercase()) }
                p.muted { "Last 30 days, " @if by == "environment" { "every environment" } @else { strong { (f.environment) } } ". Role: the one each event was sent with." }
            } }
            nav.tabs aria-label="Dimension" {
                @for (k, l) in BREAKDOWNS {
                    a href={ "/p/" (p.id) "/breakdown?by=" (k) "&env=" (url_enc(&f.environment)) } aria-current=[(*k == by).then_some("page")] { (l) }
                }
            }
            (filter_bar(p.id, b, f, &format!("/breakdown?by={by}")))
            section.card.wide {
                @if rows.is_empty() { p.muted { "No data in this window." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { (label) } th.num { "Users" } th { span.sr-only { "Share" } } th.num { "Sessions" } th.num { "Tasks" } th.num { "Errors" } th.num { "Events" } } }
                        tbody { @for r in rows {
                            tr {
                                th scope="row" { @match &r.value { Some(v) => span.mono { (v) }, None => span.muted { "(none)" } } }
                                td.num { strong { (compact(r.users)) } }
                                td.meter { (meter(r.users as f64 / max)) }
                                td.num { (compact(r.sessions)) }
                                td.num { (compact(r.tasks)) }
                                td.num { (compact(r.errors)) }
                                td.num.muted { (compact(r.events)) }
                            }
                        } }
                    } }
                }
            }
        },
    )
}

/// Read-only SQL for admins (#222): the schema, a query box, the result
/// and a CSV download.
pub fn sql(
    p: &ProjectInfo<'_>,
    query: &str,
    res: Option<&openlens_query::QueryResult>,
    error: Option<&str>,
) -> Markup {
    let cols = [
        ("uuid", "text"),
        ("event", "text"),
        ("distinct_id", "text"),
        ("session_id", "text"),
        ("timestamp", "timestamp (UTC)"),
        ("received_at", "timestamp"),
        ("url", "text"),
        ("path", "text"),
        ("feature", "text"),
        ("role", "text"),
        ("account", "text"),
        ("plan", "text"),
        ("environment", "text"),
        ("release", "text"),
        ("flags", "JSON text"),
        ("properties", "JSON text"),
        ("date", "partition YYYY-MM-DD"),
    ];
    layout(
        "SQL",
        Some(p),
        html! {
            div.heading { div {
                h1 { "SQL" }
                p.muted { "Read-only, this project only, 10,000 rows and 15 s at most. Tables: " code { "events" } " (logins merged), " code { "raw_events" } ", " code { "persons" } "." }
            } }
            div.sql-layout {
                details.card.schema open {
                    summary { (icon("layers")) " events" }
                    ul.plain-list.small { @for (c, t) in cols { li { code { (c) } " " span.muted { (t) } } } }
                    p.muted.small { "Filter on " code { "date" } " first: it skips whole days of files." }
                }
                div {
                    form.sqlbox method="post" action={ "/p/" (p.id) "/sql" } {
                        label for="sql" { "Query" }
                        textarea #sql name="sql" rows="6" required spellcheck="false" { (query) }
                        div.actions {
                            button type="submit" name="format" value="table" { "Run" }
                            button.soft type="submit" name="format" value="csv" { "Download CSV" }
                        }
                    }
                    @if let Some(e) = error { p.alert role="alert" { (e) } }
                    @if let Some(r) = res {
                        p.muted.small { (r.rows.len()) " rows" @if r.truncated { " · " strong { "cut at the limit" } } }
                        div.table-scroll { table.plain.sqlres {
                            thead { tr { @for c in &r.columns { th { (c) } } } }
                            tbody { @for row in &r.rows { tr { @for c in &r.columns {
                                td.mono.small { @match row.get(c) {
                                    None | Some(serde_json::Value::Null) => span.muted { "null" },
                                    Some(serde_json::Value::String(s)) => (s),
                                    Some(v) => (v.to_string()),
                                } }
                            } } } }
                        } }
                    }
                }
            }
        },
    )
}

/// What the last 7 days say about a feature map (#221).
#[derive(Debug, Default)]
pub struct MapLive {
    /// (feature, uses, users).
    pub uses: Vec<(String, i64, i64)>,
    /// (success event, hits).
    pub success_hits: Vec<(String, i64)>,
    /// Raw role values no role of the map matches.
    pub unmapped_roles: Vec<String>,
}

/// Everything the feature map page shows.
pub struct MapView<'a> {
    /// Project.
    pub p: &'a ProjectInfo<'a>,
    /// The YAML shown in the editor.
    pub yaml: &'a str,
    /// The version shown (None for a draft).
    pub version: Option<i64>,
    /// The latest stored version.
    pub latest: Option<i64>,
    /// The parsed map, when valid.
    pub map: Option<&'a openlens_featuremap::FeatureMap>,
    /// Lint errors and warnings.
    pub issues: &'a [openlens_featuremap::Issue],
    /// Live status.
    pub live: &'a MapLive,
    /// (version, created_at, bytes), newest first.
    pub history: &'a [(i64, String, i64)],
    /// Admins edit.
    pub admin: bool,
    /// When checking a draft: the stored YAML to diff against.
    pub draft: Option<&'a str>,
    /// A notice to show.
    pub notice: Option<&'a str>,
}

/// A line diff of two texts: (`' '` same, `'-'` removed, `'+'` added).
/// Plain LCS, so bounded to 3000 lines a side (larger: whole replace).
pub fn line_diff<'a>(old: &'a str, new: &'a str) -> Vec<(char, &'a str)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > 3000 || b.len() > 3000 {
        return a
            .iter()
            .map(|l| ('-', *l))
            .chain(b.iter().map(|l| ('+', *l)))
            .collect();
    }
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1].saturating_add(1)
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(('-', a[i]));
            i += 1;
        } else {
            out.push(('+', b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| ('-', *l)));
    out.extend(b[j..].iter().map(|l| ('+', *l)));
    out
}

/// The feature map page (#221).
pub fn feature_map(v: &MapView<'_>) -> Markup {
    let p = v.p;
    let errors = v.issues.iter().filter(|i| i.level == "error").count();
    let uses = |k: &str| v.live.uses.iter().find(|(f, _, _)| f == k);
    let hits = |e: &str| {
        v.live
            .success_hits
            .iter()
            .find(|(x, _)| x == e)
            .map_or(0, |(_, n)| *n)
    };
    let diff = v.draft.map(|old| line_diff(old, v.yaml));
    let changed = diff
        .as_ref()
        .is_some_and(|d| d.iter().any(|(c, _)| *c != ' '));
    layout(
        "Feature map",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Feature map" }
                    p.muted {
                        @match (v.draft.is_some(), v.version, v.latest) {
                            (true, _, _) => "Checking an edit, not saved yet.",
                            (_, Some(n), Some(l)) if n != l => { "Version " (n) " (latest is " a href={ "/p/" (p.id) "/features" } { (l) } ")." },
                            (_, Some(n), _) => { "Version " (n) ", used by every board." },
                            _ => "None yet: agents push it with openlens-cli features push, or write it below.",
                        }
                    }
                }
                @if v.latest.is_some() {
                    p { a.button href={ "/p/" (p.id) "/features/map.yaml" (v.version.map(|n| format!("?v={n}")).unwrap_or_default()) } { "YAML" }
                        " " a.button href={ "/p/" (p.id) "/features/map.json" (v.version.map(|n| format!("?v={n}")).unwrap_or_default()) } { "JSON" } }
                }
            }
            @if let Some(n) = v.notice { p.notice role="status" { (n) } }
            @if !v.issues.is_empty() {
                section.card.wide {
                    h2.coral { (icon("alert")) (errors) " errors · " (v.issues.len() - errors) " warnings" }
                    ul.plain-list { @for i in v.issues {
                        li { span class={ "status " (if i.level == "error" { "bad" } else { "warn" }) } { (i.level) } " " code { (i.path) } " " (i.message) }
                    } }
                }
            }
            @if !v.live.unmapped_roles.is_empty() {
                section.card.wide {
                    h2.sun { (icon("gap")) "Roles sent that no role matches" }
                    p { @for (i, r) in v.live.unmapped_roles.iter().enumerate() { @if i > 0 { ", " } code { (r) } } }
                    p.muted.small { "Add them to a role's " code { "match" } " list, or they count apart on the board." }
                }
            }
            @if let Some(m) = v.map {
                section.card.wide {
                    h2.violet { (icon("layers")) (m.features.len()) " features · " (m.roles.len()) " roles · churn after " (m.churn_days()) " days" }
                    div.table-scroll { table.plain {
                        thead { tr { th { "Feature" } th { "Entry" } th { "Success" } th.num { "Uses (7 d)" } th.num { "Users" } th { "Status" } } }
                        tbody { @for f in &m.features {
                            @let u = uses(&f.key);
                            @let h = f.success.as_deref().map(hits);
                            tr {
                                th scope="row" { strong { (f.name) } br; code.small { (f.key) } @if let Some(o) = &f.owner { br; span.muted.small { (o) } } }
                                td.small {
                                    @for r in &f.entry.routes { code { (r) } " " }
                                    @for s in &f.entry.selectors { code { (s) } " " }
                                    @for c in &f.entry.calls { code { (c) } " " }
                                }
                                td.small { @match &f.success { Some(s) => { code { (s) } " · " (compact(h.unwrap_or(0))) } , None => span.muted { "—" } } }
                                td.num { (compact(u.map_or(0, |x| x.1))) }
                                td.num { (compact(u.map_or(0, |x| x.2))) }
                                td { @if u.is_none() { span.status.bad { "not seen" } }
                                     @else if h == Some(0) { span.status.warn { "no success" } }
                                     @else { span.status.good { "live" } } }
                            }
                        } }
                    } }
                    @if !m.roles.is_empty() {
                        p.small { "Roles: " @for (i, r) in m.roles.iter().enumerate() { @if i > 0 { " · " } strong { (r.key) } " ← " (r.matches.join(", ")) } }
                    }
                }
            }
            @if let Some(d) = &diff {
                section.card.wide {
                    h2.sky { (icon("route")) "Changes against the stored map" }
                    @if !changed { p.muted { "No change." } }
                    @else {
                        pre.diff { @for (c, l) in d {
                            @match c { '+' => { ins { "+ " (l) } }, '-' => { del { "- " (l) } }, _ => { span { "  " (l) } } }
                            "\n"
                        } }
                    }
                }
            }
            @if v.admin {
                section.card.wide {
                    h2.mint { (icon("code")) "Edit" }
                    form.sqlbox method="post" action={ "/p/" (p.id) "/features" } {
                        label for="map-yaml" { "openlens.features.yaml" }
                        textarea #map-yaml name="yaml" rows="18" spellcheck="false" { (v.yaml) }
                        div.actions {
                            button.soft type="submit" name="action" value="check" { "Check and show changes" }
                            button type="submit" name="action" value="save" { "Save as a new version" }
                        }
                    }
                    p.muted.small { "Saving a map with errors is refused; warnings are kept. The app's own file stays the source of truth: copy a change back there, or agents will push theirs over it." }
                }
            }
            @if !v.history.is_empty() {
                section.card.wide {
                    h2 { (icon("clock")) "History" }
                    ul.plain-list.small { @for (n, at, bytes) in v.history {
                        li { a href={ "/p/" (p.id) "/features?v=" (n) } { "Version " (n) } " · " (at.get(..16).unwrap_or(at).replace('T', " ")) " UTC · " (bytes) " bytes"
                            @if Some(*n) == v.latest { " · " span.status.good { "current" } } }
                    } }
                }
            }
        },
    )
}

/// The backup plan as the page shows it.
pub struct BackupPlan {
    /// `off`, `daily` or `weekly`.
    pub schedule: String,
    /// Hour, UTC.
    pub hour: u8,
    /// 1 (Monday) to 7.
    pub weekday: u8,
    /// Copies kept.
    pub keep: usize,
    /// Folder as set (empty: default).
    pub dir: String,
    /// Folder used.
    pub folder: String,
    /// Also to S3.
    pub to_s3: bool,
}

/// One backup run.
pub struct BackupRun {
    /// When (RFC 3339).
    pub at: String,
    /// Succeeded.
    pub ok: bool,
    /// Files copied.
    pub files: usize,
    /// Size on disk.
    pub bytes: u64,
    /// Duration.
    pub secs: f64,
    /// Local copy.
    pub dest: String,
    /// S3 object, if uploaded.
    pub s3: Option<String>,
    /// Why it failed.
    pub error: Option<String>,
    /// `schedule` or `manual`.
    pub how: String,
}

/// Everything Settings → Storage and backups shows (#218).
pub struct StorageView {
    /// The S3 settings come from `OPENLENS_S3_*` (read-only here).
    pub from_env: bool,
    /// Bucket.
    pub bucket: Option<String>,
    /// Region.
    pub region: Option<String>,
    /// Endpoint.
    pub endpoint: Option<String>,
    /// Prefix.
    pub prefix: Option<String>,
    /// Access key id.
    pub access_key: Option<String>,
    /// A secret is stored (never shown).
    pub secret_set: bool,
    /// The destination is usable (the secret opens).
    pub secret_ok: bool,
    /// Last good copy.
    pub last_ok: Option<String>,
    /// Objects in the last copy.
    pub last_objects: Option<String>,
    /// Last error.
    pub last_error: Option<String>,
    /// Backup plan.
    pub plan: BackupPlan,
    /// Newest first.
    pub history: Vec<BackupRun>,
    /// A backup runs now.
    pub running: bool,
    /// A local copy can be downloaded.
    pub has_latest: bool,
    /// The data folder.
    pub data_dir: String,
    /// After an action.
    pub notice: Option<String>,
    /// After a failed action.
    pub error: Option<String>,
}

fn bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.0} kB", n as f64 / 1024.0),
        1_048_576..1_073_741_824 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.0),
    }
}

/// Settings → Storage and backups (#218).
pub fn storage(v: &StorageView) -> Markup {
    let ro = v.from_env;
    let day = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    layout(
        "Storage and backups",
        None,
        html! {
            div.heading { div {
                h1 { "Storage and backups" }
                p.muted { "Local disk (" code { (v.data_dir) } ") is the source of truth. S3 keeps a second copy; backups are point-in-time copies you can restore." }
            } }
            @if let Some(n) = &v.notice { p.notice role="status" { (n) } }
            @if let Some(e) = &v.error { p.alert role="alert" { (e) } }
            section.card.wide {
                h2.sky { (icon("layers")) "S3-compatible copy" }
                @if ro { p.small { span.chip { "set by environment" } " The " code { "OPENLENS_S3_*" } " variables win; change them where the server runs." } }
                form.settings method="post" action="/settings/storage/s3" {
                    label { "Bucket " input type="text" name="bucket" value=[v.bucket.as_deref()] maxlength="63" disabled[ro] required; }
                    label { "Region " input type="text" name="region" value=[v.region.as_deref()] placeholder="us-east-1" maxlength="40" disabled[ro]; }
                    label { "Endpoint " input type="url" name="endpoint" value=[v.endpoint.as_deref()] placeholder="AWS: empty · R2, MinIO, Garage: https://…" maxlength="200" disabled[ro]; }
                    label { "Prefix " input type="text" name="prefix" value=[v.prefix.as_deref()] placeholder="openlens" maxlength="100" disabled[ro]; }
                    label { "Access key " input type="text" name="access_key" value=[v.access_key.as_deref()] maxlength="200" autocomplete="off" disabled[ro] required; }
                    label { "Secret key " input type="password" name="secret" placeholder=(if v.secret_set { "stored · leave empty to keep" } else { "" }) maxlength="200" autocomplete="new-password" disabled[ro]; }
                    @if !ro { button type="submit" name="action" value="save" { "Save" } }
                    @if v.bucket.is_some() { button.soft type="submit" name="action" value="test" formnovalidate { "Test the connection" } }
                    @if !ro && v.bucket.is_some() { button.ghost type="submit" name="action" value="remove" formnovalidate { "Remove" } }
                }
                @if v.secret_set && !v.secret_ok && !ro { p.alert role="alert" { "The stored secret can't be opened (the server's secret key changed, or the store was restored): enter it again." } }
                p.small {
                    @match &v.last_ok { Some(t) => { "Last copy " (t.get(..16).unwrap_or(t).replace('T', " ")) " UTC" @if let Some(n) = &v.last_objects { " · " (n) " objects" } }, None => span.muted { "No copy yet." } }
                    @if let Some(e) = &v.last_error { br; span.status.bad { "Last error" } " " (e) }
                }
                p.muted.small { "Copied hourly: every closed day of events, and a daily snapshot of the metadata. The secret is stored sealed and never shown again." }
            }
            section.card.wide {
                h2.mint { (icon("clock")) "Backups" }
                form.settings method="post" action="/settings/storage/backups" {
                    label { "When "
                        select name="schedule" {
                            @for (k, l) in [("off", "Off"), ("daily", "Every day"), ("weekly", "Every week")] {
                                option value=(k) selected[v.plan.schedule == k] { (l) }
                            }
                        }
                    }
                    label { "At (UTC) " select name="hour" { @for h in 0u8..24 { option value=(h) selected[v.plan.hour == h] { (format!("{h:02}:00")) } } } }
                    label { "On (weekly) " select name="weekday" { @for (i, d) in day.iter().enumerate() { option value=(i + 1) selected[usize::from(v.plan.weekday) == i + 1] { (d) } } } }
                    label { "Keep " input type="number" name="keep" min="1" max="60" value=(v.plan.keep); }
                    label { "Folder " input type="text" name="dir" value=(v.plan.dir) placeholder=(v.plan.folder) maxlength="300"; }
                    label.check { input type="checkbox" name="to_s3" checked[v.plan.to_s3]; " Also to S3 (.tar.gz)" }
                    button type="submit" { "Save" }
                }
                div.actions {
                    form.inline method="post" action="/settings/storage/backups/run" {
                        button type="submit" disabled[v.running] { (if v.running { "Backing up…" } else { "Back up now" }) }
                    }
                    @if v.has_latest { a.button href="/settings/storage/backups/latest.tar.gz" { (icon("down")) "Download the latest" } }
                }
                p.muted.small { "Each copy is consistent while the server runs and takes as much room as the data. Failures are emailed to admins when SMTP is set." }
                @if !v.history.is_empty() {
                    div.table-scroll { table.plain {
                        thead { tr { th { "When (UTC)" } th { "Result" } th.num { "Size" } th.num { "Files" } th.num { "Took" } th { "Where" } } }
                        tbody { @for r in &v.history {
                            tr {
                                td.small { (r.at.get(..16).unwrap_or(&r.at).replace('T', " ")) " " span.muted { (r.how) } }
                                td { @if r.ok { span.status.good { "ok" } } @else { span.status.bad { "failed" } br; span.small { (r.error.as_deref().unwrap_or("")) } } }
                                td.num { (bytes(r.bytes)) }
                                td.num { (r.files) }
                                td.num { (duration(r.secs)) }
                                td.small.mono { (r.dest) @if let Some(k) = &r.s3 { br; "s3: " (k) } }
                            }
                        } }
                    } }
                }
            }
            section.card.wide {
                h2.coral { (icon("shield")) "Restore" }
                p { "Restoring replaces the data, so it is done with the server stopped:" }
                pre.snippet { code { "# stop OpenLens, then
mv " (v.data_dir) " " (v.data_dir) ".old
cp -a " (v.plan.folder) "/<copy> " (v.data_dir) "
# start OpenLens" } }
                p.muted.small { "From S3: download " code { "backups/<copy>.tar.gz" } ", " code { "tar xzf" } " it and use the folder inside. S3 secrets are not in backups: enter them again after a restore." }
            }
        },
    )
}

/// Who the users are (#225): a world map of users per country, then
/// devices, browsers, systems, languages and screens.
pub fn audience(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    a: &openlens_engine::Audience,
    geo: bool,
) -> Markup {
    let max = a.countries.iter().map(|c| c.1).max().unwrap_or(1).max(1) as f64;
    let name = |iso: &str| -> String {
        world::COUNTRIES
            .iter()
            .find(|c| c.0 == iso)
            .map_or_else(|| iso.to_owned(), |c| c.1.to_owned())
    };
    let users = a.users.max(1) as f64;
    // Five steps of one hue, on a square-root scale (a few big countries
    // would leave everyone else pale on a linear one).
    let step = |n: i64| -> u8 {
        if n <= 0 {
            0
        } else {
            1 + ((n as f64 / max).sqrt() * 4.0).min(4.0) as u8
        }
    };
    let (w, h) = world::SIZE;
    let list = |title: &str, glyph: &str, rows: &[(String, i64)], by: Option<&str>| {
        html! {
            section.card {
                h2.violet { (icon(glyph)) (title) }
                @if rows.is_empty() { p.muted.small { "Nothing yet." } }
                @else {
                    table.plain.bars { tbody { @for (v, n) in rows.iter().take(8) {
                        tr {
                            th scope="row" { (v) }
                            td.meter { (meter(*n as f64 / users)) }
                            td.num { (pct(*n as f64 / users)) }
                        }
                    } } }
                    @if let Some(by) = by { p.small { a href={ "/p/" (p.id) "/breakdown?by=" (by) "&env=" (url_enc(&f.environment)) } { "Break down by " (title.to_lowercase()) " →" } } }
                }
            }
        }
    };
    layout(
        "Audience",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Audience" }
                p.muted { (compact(a.users)) " users with a page view in the last 30 days, in " strong { (f.environment) } ". IP addresses are used for the country and never stored." }
            } }
            (filter_bar(p.id, b, f, "/audience"))
            section.card.wide {
                h2.violet { (icon("globe")) "Where they are" }
                @if !geo { p.small.muted { "The country database is not installed on this server (it ships in the Docker image; see the deploy guide), so countries are unknown." } }
                svg.world viewBox={ "0 0 " (w) " " (h) } role="img" aria-label="Users per country (the table below has the numbers)" {
                    @for (iso, cname, d) in world::COUNTRIES {
                        @let n = a.countries.iter().find(|c| c.0 == *iso).map_or(0, |c| c.1);
                        path class={ "c" (step(n)) } d=(d) { title { (cname) ": " (compact(n)) (if n == 1 { " user" } else { " users" }) } }
                    }
                }
                div.legend-scale aria-hidden="true" { span { "fewer" } @for i in 1..=5u8 { i class={ "c" (i) } {} } span { "more users" } }
                @if !a.countries.is_empty() {
                    details.table-view {
                        summary { "As a table" }
                        table.plain { thead { tr { th { "Country" } th.num { "Users" } th.num { "Share" } } }
                            tbody { @for (iso, n) in &a.countries { tr { th scope="row" { (name(iso)) " " span.muted.small { (iso) } } td.num { (compact(*n)) } td.num { (pct(*n as f64 / users)) } } } } }
                    }
                }
                p.muted.small { "IP geolocation by " a href="https://db-ip.com" { "DB-IP" } " (CC BY 4.0). Map outlines: Natural Earth." }
            }
            div.grid {
                (list("Devices", "board", &a.devices, Some("device")))
                (list("Browsers", "globe", &a.browsers, Some("browser")))
                (list("Systems", "layers", &a.os, Some("os")))
                (list("Languages", "idea", &a.languages, None))
                (list("Screens", "board", &a.screens, None))
            }
        },
    )
}

/// Feature flow (#224): a Sankey of how sessions move between features,
/// drawn in SVG on the server, with a table view.
pub fn flows(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    flow: &openlens_engine::Flow,
    features: &[(String, String)],
    from: Option<&str>,
    to_success: bool,
) -> Markup {
    use openlens_engine::flows::{EXIT, SUCCESS};
    let label = |k: &str| -> String {
        features
            .iter()
            .find(|(key, _)| key == k)
            .map_or_else(|| k.to_owned(), |(_, n)| n.clone())
    };
    let cols = flow.steps.len().max(1);
    let (w, h, nw, gap) = (1000.0_f64, 440.0_f64, 12.0_f64, 10.0_f64);
    let total = flow
        .steps
        .iter()
        .map(|c| c.iter().map(|n| n.1).sum::<i64>())
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let scale = |col: &Vec<(String, i64)>| (h - gap * (col.len().saturating_sub(1)) as f64) / total;
    let k = flow.steps.iter().map(scale).fold(f64::INFINITY, f64::min);
    let x = |i: usize| {
        if cols == 1 {
            0.0
        } else {
            i as f64 * (w - nw - 150.0) / (cols - 1) as f64
        }
    };
    // Node boxes: (step, name) → (y, height).
    let mut boxes: std::collections::HashMap<(usize, String), (f64, f64)> =
        std::collections::HashMap::new();
    for (i, col) in flow.steps.iter().enumerate() {
        let mut y = 0.0;
        for (n, c) in col {
            let hh = (*c as f64 * k).max(2.0);
            boxes.insert((i, n.clone()), (y, hh));
            y += hh + gap;
        }
    }
    let mut out_off: std::collections::HashMap<(usize, String), f64> =
        std::collections::HashMap::new();
    let mut in_off: std::collections::HashMap<(usize, String), f64> =
        std::collections::HashMap::new();
    let mut ribbons: Vec<(String, String, String, i64, bool)> = Vec::new();
    for l in &flow.links {
        let (Some(a), Some(bx)) = (
            boxes.get(&(l.step, l.from.clone())),
            boxes.get(&(l.step + 1, l.to.clone())),
        ) else {
            continue;
        };
        let th = (l.sessions as f64 * k).max(1.0);
        let oa = out_off.entry((l.step, l.from.clone())).or_insert(0.0);
        let ob = in_off.entry((l.step + 1, l.to.clone())).or_insert(0.0);
        let (x0, x1) = (x(l.step) + nw, x(l.step + 1));
        let (y0, y1) = (a.0 + *oa, bx.0 + *ob);
        *oa += th;
        *ob += th;
        let xm = (x0 + x1) / 2.0;
        let d = format!(
            "M{x0:.1} {y0:.1}C{xm:.1} {y0:.1} {xm:.1} {y1:.1} {x1:.1} {y1:.1}L{x1:.1} {:.1}C{xm:.1} {:.1} {xm:.1} {:.1} {x0:.1} {:.1}Z",
            y1 + th,
            y1 + th,
            y0 + th,
            y0 + th
        );
        ribbons.push((d, l.from.clone(), l.to.clone(), l.sessions, l.to == EXIT));
    }
    let base = format!("/p/{}/flows", p.id);
    layout(
        "Feature flow",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Feature flow" }
                p.muted { "How sessions move between features, last 30 days, " strong { (f.environment) } ": "
                    (compact(flow.sessions)) " sessions, " (compact(flow.succeeded)) " reached a success event. Repeats are merged; 5 steps at most; the 8 widest per step, the rest is Other." }
            } }
            nav.tabs aria-label="View" {
                a href=(base) aria-current=[(from.is_none() && !to_success).then_some("page")] { "From the start" }
                a href={ (base) "?to=success" } aria-current=[to_success.then_some("page")] { "Paths to success" }
            }
            form.filters method="get" action=(base) aria-label="After a feature" {
                label { "After the feature "
                    select name="from" {
                        option value="" { "—" }
                        @for (k, n) in features { option value=(k) selected[from == Some(k.as_str())] { (n) } }
                    }
                }
                input type="hidden" name="env" value=(f.environment);
                button type="submit" { "Show" }
            }
            (filter_bar(p.id, b, f, "/flows"))
            section.card.wide {
                @if flow.sessions == 0 { p.muted { "No session used a feature in this window. Features come from the feature map and " code { "data-feature" } "." } }
                @else {
                    div.sankey-scroll {
                        svg.sankey viewBox={ "-2 -2 " (w + 4.0) " " (h + 4.0) } role="img" aria-label="Feature flow (the table below has the numbers)" {
                            @for (d, a, bn, n, drop) in &ribbons {
                                path class=(if *drop { "ribbon drop" } else { "ribbon" }) d=(d) { title { (label(a)) " → " (label(bn)) ": " (compact(*n)) " sessions" } }
                            }
                            @for (i, col) in flow.steps.iter().enumerate() {
                                @for (n, c) in col {
                                    @let (y, hh) = boxes.get(&(i, n.clone())).copied().unwrap_or((0.0, 0.0));
                                    @let cls = if n == SUCCESS { "node ok" } else if n == EXIT { "node exit" } else { "node" };
                                    rect class=(cls) x=(format!("{:.1}", x(i))) y=(format!("{y:.1}")) width=(nw) height=(format!("{hh:.1}")) rx="2" { title { (label(n)) ": " (compact(*c)) } }
                                    @if hh >= 11.0 {
                                        text.slabel x=(format!("{:.1}", x(i) + nw + 5.0)) y=(format!("{:.1}", y + hh / 2.0 + 4.0)) { (label(n)) " · " (compact(*c)) }
                                    }
                                }
                            }
                        }
                    }
                    details.table-view {
                        summary { "As a table" }
                        table.plain {
                            thead { tr { th.num { "Step" } th { "From" } th { "To" } th.num { "Sessions" } } }
                            tbody { @for l in &flow.links {
                                tr { td.num { (l.step + 1) } td { (label(&l.from)) } td { @if l.to == EXIT { span.status.bad { "exit" } } @else { (label(&l.to)) } } td.num { (compact(l.sessions)) } }
                            } }
                        }
                    }
                }
            }
        },
    )
}

/// Right now (#228): who is using the app, refreshed every 10 seconds by
/// the browser (a `Refresh` header: no script).
pub fn live(p: &ProjectInfo<'_>, f: &Filters, l: &openlens_engine::Live) -> Markup {
    let list = |title: &str, glyph: &str, rows: &[(String, i64)]| {
        html! {
            section.card {
                h2.violet { (icon(glyph)) (title) }
                @if rows.is_empty() { p.muted.small { "Nothing in the last 30 minutes." } }
                @else { table.plain { tbody { @for (v, n) in rows {
                    tr { th scope="row" { span.mono { (v) } } td.num { (compact(*n)) } }
                } } } }
            }
        }
    };
    layout(
        "Live",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Live" }
                p.muted { "Last 30 minutes in " strong { (f.environment) } ". Refreshes every 10 seconds; events show up within the flush delay (a few seconds)." }
            } }
            section.tiles aria-label="Right now" {
                (tile("violet", "users", "Active now", &compact(l.users_5m), "users, last 5 minutes"))
                (tile("sky", "clock", "Last 30 minutes", &compact(l.users_30m), "users"))
                (tile("coral", "alert", "Errors", &compact(l.errors_30m), "last 30 minutes"))
            }
            div.grid {
                (list("Releases", "release", &l.releases))
                (list("Roles", "users", &l.roles))
                (list("Features", "star", &l.features))
                (list("Pages", "route", &l.pages))
            }
            section.card.wide {
                h2.mint { (icon("events")) "Latest events" }
                @if l.events.is_empty() { p.muted { "Quiet." } }
                @else {
                    div.table-scroll { table.plain.events {
                        thead { tr { th { "When (" (tz_label(&f.tz)) ")" } th { "Event" } th { "User" } th { "Page" } th { "Release" } } }
                        tbody { @for r in &l.events {
                            @let fam = openlens_engine::family(&r.event);
                            tr {
                                td.muted.mono { (local_time(&r.timestamp, &f.tz, true)) }
                                td { span class={ "chip fam " (fam) } { (icon(family_icon(fam))) span.sr-only { (family_label(fam)) ": " } (r.event) } }
                                td.mono { a href={ "/p/" (p.id) "/people/" (url_enc(&r.distinct_id)) } { (r.distinct_id) } }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                            }
                        } }
                    } }
                }
            }
        },
    )
}

/// One check as shown: (time, ok, status, ms, error).
pub type CheckRow = (String, bool, Option<i64>, Option<i64>, Option<String>);

/// Uptime (#256): the app's state, availability, response times and
/// incidents.
pub struct UptimeView<'a> {
    /// `up`, `down`, `unknown` or `off`.
    pub state: &'a str,
    /// Since when (a time, in the reader's zone).
    pub since: Option<String>,
    /// The probed URL, when the probe is on.
    pub url: Option<String>,
    /// Availability over 24 h and 7 days (share of good checks).
    pub avail: (Option<f64>, Option<f64>),
    /// p50 and p95 response time over 24 h, ms.
    pub ms: (Option<i64>, Option<i64>),
    /// (rule, start, end, length in minutes, cause), newest first.
    pub incidents: Vec<(String, String, Option<String>, i64, String)>,
    /// The last checks, newest first: (time, ok, status, ms, error).
    pub last: Vec<CheckRow>,
}

/// The Uptime page.
pub fn uptime(p: &ProjectInfo<'_>, v: &UptimeView<'_>) -> Markup {
    let (cls, word) = match v.state {
        "up" => ("good", "Up"),
        "down" => ("bad", "Down"),
        "off" => ("muted", "Not checked"),
        _ => ("muted", "Waiting for the first check"),
    };
    layout(
        "Uptime",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Uptime" }
                p { span class={ "status big " (cls) } { (word) } @if let Some(s) = &v.since { " since " (s) } }
            } }
            @if v.url.is_none() {
                section.card.wide { p { "Set the app address in " a href={ "/p/" (p.id) "/settings#alerts" } { "Settings" } " and OpenLens checks it every minute. Without it, only the traffic tells (after four weeks of history)." } }
            } @else {
                section.tiles aria-label="Availability" {
                    (tile("mint", "task", "Last 24 hours", &v.avail.0.map_or_else(|| "—".into(), |a| format!("{:.2}%", a * 100.0)), "available"))
                    (tile("sky", "clock", "Last 7 days", &v.avail.1.map_or_else(|| "—".into(), |a| format!("{:.2}%", a * 100.0)), "available"))
                    (tile("violet", "bolt", "Response time", &v.ms.0.map_or_else(|| "—".into(), |m| format!("{m} ms")), &v.ms.1.map_or_else(|| "p50, last 24 hours".into(), |m| format!("p50 · p95 {m} ms"))))
                }
                p.muted.small { "Checking " span.mono { (v.url.as_deref().unwrap_or("")) } " every minute. Down after two failed checks in a row (no answer, or HTTP 5xx)." }
            }
            section.card.wide {
                h2.coral { (icon("alert")) "Incidents and alerts, last 30 days" }
                @if v.incidents.is_empty() { p.muted { "None." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "What" } th { "Started" } th { "Ended" } th.num { "Length" } th { "Cause" } } }
                        tbody { @for (rule, start, end, mins, cause) in &v.incidents {
                            tr {
                                td { span class={ "status " (if end.is_none() { "bad" } else { "muted" }) } { (match rule.as_str() {
                                    "traffic" => "traffic stopped",
                                    "down" => "down",
                                    r if r.starts_with("error:") => "new error",
                                    _ => "regression",
                                }) } }
                                td.small { (start) }
                                td.small { @match end { Some(e) => (e), None => strong { "ongoing" } } }
                                td.num { (mins) " min" }
                                td.small { (cause) }
                            }
                        } }
                    } }
                }
            }
            @if !v.last.is_empty() {
                section.card.wide {
                    h2 { (icon("clock")) "Latest checks" }
                    ul.plain-list.small { @for (t, ok, status, ms, err) in v.last.iter().take(15) {
                        li { span class={ "status " (if *ok { "good" } else { "bad" }) } { (if *ok { "ok" } else { "failed" }) } " " (t)
                            @if let Some(s) = status { " · HTTP " (s) } @if let Some(m) = ms { " · " (m) " ms" } @if let Some(e) = err { " · " span.muted { (e) } } }
                    } }
                }
            }
        },
    )
}

/// Where users come from (#226), ranked by the share reaching the value
/// moment, not by visits.
pub fn acquisition(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    rows: &[openlens_engine::AcquisitionRow],
    by: &str,
    last: bool,
) -> Markup {
    let base = format!("/p/{}/acquisition", p.id);
    let env = url_enc(&f.environment);
    let dims = [
        ("channel", "Channel"),
        ("source", "Source"),
        ("campaign", "Campaign"),
        ("referrer", "Referrer"),
    ];
    let total: i64 = rows.iter().map(|r| r.users).sum();
    layout(
        "Acquisition",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Where users come from" }
                p.muted { "Last 90 days, " strong { (f.environment) } ", by " (if last { "last touch (direct visits don't count)" } else { "first touch" }) ". Ranked by the share that reaches the value moment, not by visits." }
            } }
            nav.tabs aria-label="Group by" {
                @for (k, l) in dims {
                    a href={ (base) "?by=" (k) "&touch=" (if last { "last" } else { "first" }) "&env=" (env) } aria-current=[(k == by).then_some("page")] { (l) }
                }
                " · "
                a href={ (base) "?by=" (by) "&touch=first&env=" (env) } aria-current=[(!last).then_some("page")] { "First touch" }
                a href={ (base) "?by=" (by) "&touch=last&env=" (env) } aria-current=[last.then_some("page")] { "Last touch" }
            }
            (filter_bar(p.id, b, f, &format!("/acquisition?by={by}&touch={}", if last { "last" } else { "first" })))
            section.card.wide {
                @if rows.is_empty() { p.muted { "No landing with a channel yet. The SDK sends the referrer and the campaign parameters on the first page view of a visit (SDK builds from 2026-11-23 on)." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { (dims.iter().find(|d| d.0 == by).map_or("Channel", |d| d.1)) } th.num { "Users" } th.num { "Reached value" } th { span.sr-only { "Share" } } th.num { "Came back" } th.num { "Churned" } } }
                        tbody { @for r in rows {
                            tr {
                                th scope="row" { span.mono { (r.value) } @if r.users < 20 { " " span.chip title="Under 20 users: the rates move a lot" { "low sample" } } }
                                td.num { (compact(r.users)) span.muted.small { " · " (pct(r.users as f64 / total.max(1) as f64)) } }
                                td.num { strong { (pct(r.value_rate())) } }
                                td.meter { (meter(r.value_rate())) }
                                td.num { (pct(r.returned as f64 / r.users.max(1) as f64)) }
                                td.num { @if r.judged > 0 { (pct(r.churned as f64 / r.judged as f64)) } @else { span.muted { "—" } } }
                            }
                        } }
                    } }
                    p.muted.small { "Reached value: a success event of the feature map (any feature use when none is declared). Came back: active on two days or more. Churned: of the users first seen before the churn window, those inactive for longer than it now." }
                }
            }
        },
    )
}

/// Churn (#247): how many leave, after how long, at which moment, with
/// what friction, and who is at risk now.
pub fn churn(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    c: &openlens_engine::ChurnReport,
) -> Markup {
    let rate = c.churned_recent as f64 / c.active_at_start.max(1) as f64;
    let net = c.new_recent + c.resurrected - c.churned_recent;
    let leave_max = c.leave_week.iter().map(|x| x.1).max().unwrap_or(1).max(1) as f64;
    layout(
        "Churn",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Churn" }
                p.muted { "A user has churned after " strong { (c.window) " days" } " without using the app (" code { "churn_after_days" } " in the feature map). Last 90 days, " strong { (f.environment) } ", logged-in users." }
            } }
            @if let Some(s) = c.suggested {
                @if s != c.window {
                    p.notice { "Your users usually come back within " strong { (s) " days" } " (9 gaps out of 10 between active days are shorter): " code { "churn_after_days: " (s) } " would fit this app better. Nothing changes until the feature map says so." }
                }
            }
            (filter_bar(p.id, b, f, "/churn"))
            section.tiles aria-label="How many" {
                (tile("coral", "ban", "Churn rate", &pct(rate), &format!("{} of {} users active {} days ago left", compact(c.churned_recent), compact(c.active_at_start), c.window)))
                (tile("mint", "users", "Net change", &format!("{}{}", if net >= 0 { "+" } else { "" }, compact(net)), &format!("{} new + {} back − {} gone", compact(c.new_recent), compact(c.resurrected), compact(c.churned_recent))))
                (tile("sun", "alert", "At risk now", &compact(c.at_risk.len() as i64), "quiet for over half the window, less active than before"))
                (tile("violet", "clock", "Median lifetime", &c.median_days.map_or_else(|| "over 90 days".into(), |d| format!("{d} days")), "half the users are gone by then"))
            }
            div.grid {
                section.card {
                    h2.coral { (icon("ban")) "Churned per week" }
                    @if c.by_week.is_empty() { p.muted.small { "Nobody has churned in the window." } }
                    @else { (sparkline(&c.by_week)) p.muted.small { "Users whose last active day fell that week." } }
                }
                section.card {
                    h2.violet { (icon("clock")) "How long they stay" }
                    table.plain.bars { tbody { @for (d, s) in &c.survival {
                        tr { th scope="row" { "day " (d) } td.meter { (meter(*s)) } td.num { (pct(*s)) } }
                    } } }
                    p.muted.small { "Share still active N days after their first visit (Kaplan–Meier: users who joined recently count only for the days they could have stayed)." }
                }
                section.card {
                    h2.sun { (icon("route")) "When they leave" }
                    @if c.leave_week.is_empty() { p.muted.small { "Nobody has churned yet." } }
                    @else {
                        table.plain.bars { tbody { @for (wk, n) in &c.leave_week {
                            tr { th scope="row" { @if *wk >= 12 { "week 12+" } @else { "week " (wk + 1) } } td.meter { (meter(*n as f64 / leave_max)) } td.num { (compact(*n)) } }
                        } } }
                        p.muted.small { "Week of life in which churned users were last active. A spike in week 1 is onboarding; at the end of a trial, the trial." }
                    }
                }
                section.card {
                    h2.coral { (icon("alert")) "Friction before leaving" }
                    p { "On their last active day, " strong { (pct(c.errors_at_end.0)) } " of churned users hit an error, against " (pct(c.errors_at_end.1)) " of users who stayed." }
                    p.small { a href={ "/p/" (p.id) "/errors" } { "Errors →" } " · " a href={ "/p/" (p.id) "/flows" } { "Feature flow (exits) →" } }
                }
            }
            section.card.wide {
                h2.violet { (icon("layers")) "The last thing they did" }
                @if c.last_place.is_empty() { p.muted { "Nobody has churned in the window." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "Feature or page" } th.num { "Churned users" } th.num { "Share" } th.num { "Users who stayed" } th {} } }
                        tbody { @for (place, n, share, base) in &c.last_place {
                            tr {
                                th scope="row" { span.mono { (place) } }
                                td.num { (compact(*n)) }
                                td.num { strong { (pct(*share)) } }
                                td.num { (pct(*base)) }
                                td { @if *share > base * 1.5 && *n >= 3 { span.status.bad { "more than usual" } } }
                            }
                        } }
                    } }
                    p.muted.small { "Where churned users were last active, against where users who stayed were last seen. A place far above its usual share is where to look (and replay)." }
                }
            }
            section.card.wide {
                h2.sun { (icon("users")) "At risk now" }
                @if c.at_risk.is_empty() { p.muted { "Nobody: active users keep their pace." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "User" } th { "Role" } th { "Last active" } th.num { "Quiet for" } th.num { "Active days, last window" } th.num { "Window before" } } }
                        tbody { @for r in &c.at_risk {
                            tr {
                                td.mono { a href={ "/p/" (p.id) "/people/" (url_enc(&r.distinct_id)) } { (r.distinct_id) } }
                                td { (r.role.as_deref().unwrap_or("—")) }
                                td.small { (r.last_day) }
                                td.num { (r.idle_days) " days" }
                                td.num { (r.recent_days) }
                                td.num { (r.before_days) }
                            }
                        } }
                    } }
                    p.muted.small { "Open a profile to see their last sessions and replays. Win them back before the window closes." }
                }
            }
        },
    )
}

/// An amount with its currency, rounded to the unit above 100.
pub fn money(v: f64, cur: &str) -> String {
    let n = if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    };
    if cur == "?" { n } else { format!("{n} {cur}") }
}

/// Revenue on success events (#231).
pub fn revenue(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    r: Option<&openlens_engine::Revenue>,
) -> Markup {
    let table = |title: &str, glyph: &str, rows: &[(String, String, f64)]| {
        html! {
            section.card {
                h2.mint { (icon(glyph)) (title) }
                table.plain { tbody { @for (k, c, v) in rows {
                    tr { th scope="row" { span.mono { (k) } } td.num { (money(*v, c)) } }
                } } }
            }
        }
    };
    layout(
        "Revenue",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Revenue" }
                p.muted { "Amounts carried by success events (" code { "revenue" } " or " code { "value" } ", with an optional " code { "currency" } "), last 30 days, " strong { (f.environment) } "." }
            } }
            (filter_bar(p.id, b, f, "/revenue"))
            @match r {
                None => section.card.wide { p { "No success event carries an amount yet. Send one with the event, for example " code { "openlens.track(\"order_paid\", { revenue: 49, currency: \"EUR\" })" } ", and declare it as a feature's success event." } },
                Some(r) => {
                    section.tiles aria-label="Totals" {
                        @for m in &r.totals {
                            (tile("mint", "star", &format!("Revenue ({})", m.currency), &money(m.total, &m.currency), &format!("{} orders · {} payers · {} per active user", compact(m.orders), compact(m.payers), money(m.per_active_user, &m.currency))))
                        }
                    }
                    @if r.totals.len() > 1 { p.muted.small { "Currencies are not summed. Set " code { "revenue: { currency: EUR, rates: { USD: 0.92 } }" } " in the feature map to show one total." } }
                    @if r.converted { p.muted.small { "Converted into the display currency with the fixed rates of the feature map." } }
                    div.grid {
                        (table("By feature", "layers", &r.by_feature))
                        (table("By role", "users", &r.by_role))
                        (table("By release", "release", &r.by_release))
                    }
                }
            }
        },
    )
}

/// A goal with where it stands (#235).
pub struct GoalRow {
    /// The stored goal.
    pub goal: openlens_engine::goals::Goal,
    /// KPI label.
    pub label: String,
    /// A share (shown as %).
    pub share: bool,
    /// Value now.
    pub current: Option<f64>,
    /// Projected value at the deadline.
    pub projected: Option<f64>,
    /// Per release (oldest first).
    pub series: Vec<(String, f64)>,
    /// Status label.
    pub status: String,
    /// Reached or on track.
    pub good: bool,
    /// Off track.
    pub bad: bool,
    /// Days to the deadline.
    pub days_left: i64,
}

/// A KPI a goal can be set on, with its suggested target.
pub struct GoalChoice {
    /// Key.
    pub kpi: String,
    /// Label.
    pub label: String,
    /// Higher is better.
    pub up: bool,
    /// A share.
    pub share: bool,
    /// Value now.
    pub current: f64,
    /// Suggested target.
    pub suggested: f64,
}

fn kpi_fmt(v: f64, share: bool) -> String {
    if share {
        pct(v)
    } else if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// The KPI per release with the target as a line (one axis, SVG).
fn goal_chart(r: &GoalRow) -> Markup {
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
    admin: bool,
    deadline: &str,
) -> Markup {
    layout(
        "Goals",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Goals" }
                p.muted { "Where we want each number to be, by when, and whether the trend gets there. Targets are suggested from the releases so far; one click sets them." }
            } }
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
                        @if !g.owner.is_empty() { p.muted.small { "Owner: " (g.owner) } }
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
                        thead { tr { th { "KPI" } th.num { "Now" } th { "Target" } th { "By" } th { "Owner" } th {} } }
                        tbody { @for c in choices {
                            tr {
                                th scope="row" { (c.label) span.muted.small { " · " (if c.up { "higher is better" } else { "lower is better" }) } }
                                td.num { (kpi_fmt(c.current, c.share)) }
                                td colspan="4" {
                                    form.inline method="post" action={ "/p/" (p.id) "/goals" } {
                                        input type="hidden" name="kpi" value=(c.kpi);
                                        label { span.sr-only { "Target" } input type="number" name="target" step="any" min="0" value=(if c.share { format!("{:.1}", c.suggested * 100.0) } else { format!("{:.2}", c.suggested) }) aria-label={ "Target for " (c.label) }; @if c.share { "%" } }
                                        label { span.sr-only { "Deadline" } input type="date" name="deadline" value=(deadline) required aria-label={ "Deadline for " (c.label) }; }
                                        label { span.sr-only { "Owner" } input type="text" name="owner" placeholder="owner" maxlength="80" aria-label={ "Owner of the goal on " (c.label) }; }
                                        button.soft type="submit" { "Set" }
                                    }
                                }
                            }
                        } }
                    } }
                    p.muted.small { "The suggested target is filled in. Status: on track when the straight line through the releases reaches the target by the deadline, at risk when it misses by less than 10% of the target, not enough data under three releases." }
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
    layout(
        "Forms",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Forms" }
                p.muted { "Last 30 days, " strong { (f.environment) } ". A form starts when someone leaves one of its fields, and is abandoned when the session ends without sending it. Field values are never sent; passwords and masked fields send nothing." }
            } }
            (filter_bar(p.id, b, f, "/forms"))
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
                        thead { tr { th { "Field" } th.num { "Sessions" } th.num { "Last before leaving" } th { span.sr-only { "Share" } } th.num { "Came back to it" } th.num { "Time spent" } } }
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

/// Feature flags: switch on/off and change the rollout. Creating flags is
/// done by agents (API/CLI/MCP) with the code that uses them.
pub fn flags(p: &ProjectInfo<'_>, flags: &[openlens_flags::Flag]) -> Markup {
    layout(
        "Flags",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Flags" }
                    p.muted { "Ship a change to some users first, then compare. Each flag's users show up as their own cohort." }
                }
                a.button href={ "/p/" (p.id) "/flags-edit" } { (icon("plus")) "New flag" }
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
                                th scope="row" { a href={ "/p/" (p.id) "/flags-edit?key=" (f.key) } title="Edit" { code { (f.key) } } @if !f.description.is_empty() { br; span.muted.small { (f.description) } } }
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

/// Create or edit a flag (#220). `flag` is None for a new one.
pub fn flag_edit(
    p: &ProjectInfo<'_>,
    flag: Option<&openlens_flags::Flag>,
    error: Option<&str>,
) -> Markup {
    let new = flag.is_none_or(|f| f.key.is_empty());
    let f = flag.cloned().unwrap_or(openlens_flags::Flag {
        key: String::new(),
        description: String::new(),
        enabled: false,
        rollout: 0,
        roles: Vec::new(),
        accounts: Vec::new(),
    });
    let action = format!("/p/{}/flags-edit", p.id);
    layout(
        "Flags",
        Some(p),
        html! {
            div.heading { div {
                h1 { @if new { "New flag" } @else { "Flag " code { (f.key) } } }
                p.muted { a href={ "/p/" (p.id) "/flags" } { "← Flags" } " · the code checks it with " code { "openlens.isEnabled(\"" (if f.key.is_empty() { "key" } else { &f.key }) "\")" } }
            } }
            @if let Some(e) = error { p.alert role="alert" { (e) } }
            section.card.wide {
                form.settings method="post" action=(action) {
                    input type="hidden" name="original" value=(if new { "" } else { flag.map_or("", |x| x.key.as_str()) });
                    label { "Key " input type="text" name="key" value=(f.key) required maxlength="64" pattern="[a-z0-9_-]+" placeholder="new-editor"; }
                    label { "What it is for " input type="text" name="description" value=(f.description) maxlength="200"; }
                    label { "Rollout % " input type="number" name="rollout" min="0" max="100" value=(f.rollout); }
                    label { "Only roles " input type="text" name="roles" value=(f.roles.join(", ")) placeholder="every role"; }
                    label { "Always on for accounts " input type="text" name="accounts" value=(f.accounts.join(", ")) placeholder="none"; }
                    label.check { input type="checkbox" name="enabled" checked[f.enabled]; " On" }
                    button type="submit" { (if new { "Create" } else { "Save" }) }
                }
                p.muted.small { "Roles are the feature map's canonical roles; lists are comma-separated. Users in the rollout keep their bucket when you raise it." }
            }
            @if !new {
                section.card.wide.danger {
                    h2.coral { (icon("alert")) "Delete the flag" }
                    p { "The code then gets " code { "false" } " for it. Its past cohort stays in the data." }
                    form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) "/delete" } {
                        label { "Type " code { (f.key) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                        button.danger type="submit" { "Delete" }
                    }
                }
            }
        },
    )
}

/// Create or edit a micro-survey (#220).
pub fn survey_edit(
    p: &ProjectInfo<'_>,
    survey: Option<&openlens_flags::survey::Survey>,
    error: Option<&str>,
) -> Markup {
    let new = survey.is_none_or(|s| s.key.is_empty());
    let s = survey.cloned().unwrap_or(openlens_flags::survey::Survey {
        key: String::new(),
        question: String::new(),
        kind: "rating".into(),
        choices: Vec::new(),
        trigger: String::new(),
        enabled: false,
        sample: 10,
        roles: Vec::new(),
    });
    layout(
        "Surveys",
        Some(p),
        html! {
            div.heading { div {
                h1 { @if new { "New survey" } @else { "Survey " code { (s.key) } } }
                p.muted { a href={ "/p/" (p.id) "/surveys" } { "← Surveys" } " · one question, right after a moment of value, once per user" }
            } }
            @if let Some(e) = error { p.alert role="alert" { (e) } }
            section.card.wide {
                form.settings method="post" action={ "/p/" (p.id) "/surveys-edit" } {
                    input type="hidden" name="original" value=(if new { "" } else { survey.map_or("", |x| x.key.as_str()) });
                    label { "Key " input type="text" name="key" value=(s.key) required maxlength="64" pattern="[a-z0-9_-]+" placeholder="writing-usefulness"; }
                    label.wide { "Question " input type="text" name="question" value=(s.question) required maxlength="200" placeholder="How useful was writing today?"; }
                    label { "Answer "
                        select name="kind" {
                            @for (k, l) in [("rating", "Rating 1–5"), ("yes_no", "Yes / no"), ("choice", "One choice"), ("text", "Free text")] {
                                option value=(k) selected[s.kind == k] { (l) }
                            }
                        }
                    }
                    label { "Choices " input type="text" name="choices" value=(s.choices.join(", ")) placeholder="for One choice: 2 to 6"; }
                    label { "After the event " input type="text" name="trigger" value=(s.trigger) required maxlength="100" placeholder="entry_saved"; }
                    label { "Ask % of users " input type="number" name="sample" min="0" max="100" value=(s.sample); }
                    label { "Only roles " input type="text" name="roles" value=(s.roles.join(", ")) placeholder="every role"; }
                    label.check { input type="checkbox" name="enabled" checked[s.enabled]; " On" }
                    button type="submit" { (if new { "Create" } else { "Save" }) }
                }
                p.muted.small { "Answers are " code { "$survey" } " events: " a href={ "/p/" (p.id) "/events?event=%24survey" } { "see them in Events" } ", export them from Settings." }
            }
            @if !new {
                section.card.wide.danger {
                    h2.coral { (icon("alert")) "Delete the survey" }
                    p { "Nobody is asked any more. The answers already given stay (they are events)." }
                    form.inline method="post" action={ "/p/" (p.id) "/surveys/" (s.key) "/delete" } {
                        label { "Type " code { (s.key) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                        button.danger type="submit" { "Delete" }
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
                        p.small { a href={ "/p/" (p.id) "/heatmap?path=" (url_enc(&pg.path)) } { (icon("spark")) " Click heatmap" } }
                        p.small.muted { (compact(pg.views)) " views · " (compact(pg.users)) " users" }
                        @if pg.median_scroll >= 0.0 {
                            table.bars { tbody {
                                tr { th scope="row" { "Reach half the page" } td.meter { (meter(pg.reach_half)) } td.num { (pct(pg.reach_half)) } }
                                tr { th scope="row" { "Reach the end" } td.meter { (meter(pg.reach_end)) } td.num { (pct(pg.reach_end)) } }
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
    original: &[Option<openlens_errors::Original>],
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
                        thead { tr { th { "Error" } th.num { "Users" } th.num { "Sessions" } th.num { "Times" } th { "Roles" } th { "Releases" } th { "Where" } th { "Last seen" } th { span.sr-only { "Replay" } } } }
                        tbody { @for (i, g) in groups.iter().enumerate() {
                            tr {
                                th scope="row" {
                                    span.mono { (g.sample.get(..160).unwrap_or(&g.sample)) }
                                    br;
                                    @if let Some(Some(o)) = original.get(i) {
                                        span.small.mono title="Resolved with the release's source map" {
                                            (icon("route")) " " (o.source) ":" (o.line) @if let Some(n) = &o.name { " · " (n) }
                                        }
                                    } @else {
                                        @let place = g.fingerprint.rsplit(" @ ").next().unwrap_or("");
                                        @if place != "?" { span.muted.small.mono { (place) } }
                                    }
                                }
                                td.num { strong { (compact(g.users)) } }
                                td.num { (compact(g.sessions)) }
                                td.num.muted { (compact(g.count)) }
                                td.small { (list(&g.roles)) }
                                td.small { (list(&g.releases)) }
                                td.small { (list(&g.places)) }
                                td.small.muted { (local_time(&g.last_seen, &f.tz, false)) }
                                td.small { @if let Some(u) = &g.last_user { a.mono href={ "/p/" (p.id) "/people/" (url_enc(u)) } title="Who hit it last" { (u) } } }
                                td { @if let Some(s) = &g.last_session {
                                    a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (g.last_seen) } title="Replay the latest occurrence" aria-label="Replay the latest occurrence" { (icon("play")) }
                                } }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// Recorded sessions, most recent first.
pub fn replays(
    p: &ProjectInfo<'_>,
    list: &[openlens_replay::Summary],
    who: Option<&str>,
    tz: &str,
    comments: &[(String, i64)],
) -> Markup {
    layout(
        "Replays",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Replays" }
                    p.muted {
                        "Recorded sessions, most recent first. Inputs are always masked."
                        @if let Some(w) = who { " Only " span.mono { (w) } ". " a href={ "/p/" (p.id) "/replays" } { "Show everyone" } }
                    }
                }
            }
            @if list.is_empty() {
                section.card { p { (icon("play")) " No recordings yet. The SDK records sessions once a page has loaded; they show up here within a few seconds." } }
            } @else {
                section.card.wide {
                    table.plain {
                        thead { tr { th { "Started (" (tz_label(tz)) ")" } th { "User" } th.num { "Length" } th.num { "Size" } th.num { "Comments" } th { span.sr-only { "Play" } } } }
                        tbody { @for r in list {
                            @let started = tz.parse::<chrono_tz::Tz>().map_or_else(|_| r.started.format("%Y-%m-%d %H:%M").to_string(), |z| r.started.with_timezone(&z).format("%Y-%m-%d %H:%M").to_string());
                            @let secs = (r.updated - r.started).num_seconds().max(0) as f64;
                            tr {
                                td.mono { (started) }
                                td.mono { a href={ "/p/" (p.id) "/replays?u=" (r.distinct_id) } { (r.distinct_id) } }
                                td.num { (duration(secs)) }
                                td.num.muted { (compact((r.bytes / 1024) as i64)) " kB" }
                                td.num { @match comments.iter().find(|(s, _)| *s == r.session_id) { Some((_, n)) => { "💬 " (n) }, None => span.muted { "—" } } }
                                td { a.button href={ "/p/" (p.id) "/replays/" (r.session_id) } { (icon("play")) "Play" } }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// The replay player page. The player script (rrweb Replayer, MIT) is the
/// only script the dashboard serves (D-011).
pub fn player(
    p: &ProjectInfo<'_>,
    session: &str,
    who: Option<&str>,
    at: Option<&str>,
    t: Option<u64>,
    comments: &[openlens_meta::ReplayComment],
    me: Option<(&str, bool)>,
) -> Markup {
    let base = format!("/p/{}/replays/{}", p.id, session);
    let data: Vec<serde_json::Value> = comments
        .iter()
        .map(|c| serde_json::json!({ "id": c.id, "ms": c.at_ms, "body": c.body, "author": c.author }))
        .collect();
    let data = serde_json::Value::Array(data).to_string();
    layout(
        "Replay",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Replay" }
                    p.muted {
                        "Session " span.mono { (session) }
                        @if let Some(w) = who { " · " a href={ "/p/" (p.id) "/replays?u=" (w) } { (w) } }
                    }
                }
            }
            link rel="stylesheet" href="/assets/player.css";
            section.card.wide {
                div #ol-player data-src={ "/p/" (p.id) "/replays/" (session) "/events.json" } data-timeline={ "/p/" (p.id) "/replays/" (session) "/timeline.json" } data-at=[at] data-t=[t] data-comments=(data) {
                    p.muted { "Loading the recording…" }
                }
            }
            section.card.wide.comments {
                h2.violet { (icon("idea")) "Comments" span.muted.small { " · pinned to a moment" } }
                @if comments.is_empty() { p.muted.small { "None yet. Pause where something happens and say what you see." } }
                @else {
                    ol.comment-list {
                        @for c in comments {
                            li id={ "c" (c.id) } {
                                a.ol-c-at href={ (base) "?t=" (c.at_ms) "#c" (c.id) } { (duration(c.at_ms as f64 / 1000.0)) }
                                " "
                                span.comment-body { (c.body) }
                                span.muted.small { " — " (c.author) " · " (c.created_at.get(..10).unwrap_or(&c.created_at)) }
                                @if me.is_some_and(|(id, admin)| admin || id == c.user_id) {
                                    form.inline method="post" action={ (base) "/comments/" (c.id) "/delete" } {
                                        button.ghost type="submit" aria-label="Delete this comment" { "Delete" }
                                    }
                                }
                            }
                        }
                    }
                }
                form #ol-comment.comment-form method="post" action={ (base) "/comments" } {
                    input type="hidden" name="at" value=(t.unwrap_or(0));
                    label for="ol-comment-body" { "Comment at the current moment" }
                    textarea #ol-comment-body name="body" required maxlength="2000" rows="2" placeholder="The form error shows here" {}
                    button type="submit" { "Add" }
                }
            }
            script src="/assets/player.js" defer {}
        },
    )
}

/// Feature pulse: for each used feature, how many of the active users
/// adopt it, how often and on how many days they come back to it, and how
/// many reach its success event.
fn feature_pulse(b: &Board, empty: bool) -> Markup {
    let mau = b.users.mau.max(1) as f64;
    let ratio = |a: i64, n: i64| a as f64 / n.max(1) as f64;
    html! {
        section.card.wide {
            h2.sun { (icon("layers")) "Feature pulse" }
            @if empty {
                p.muted { "No feature has been used yet. Features come from " code { "data-feature" } " attributes and the feature map." }
            } @else {
                table.plain.pulse {
                    thead { tr {
                        th { "Feature" }
                        th { "Adoption" }
                        th.num title="Share of active users who used it" { span.sr-only { "Adoption " } "% users" }
                        th.num title="Average uses per user" { "Uses / user" }
                        th.num title="Average distinct days each user came back to it" { "Days / user" }
                        th.num title="Users who reached its success event" { "Completion" }
                        th.num title="Median time from the first use to the success event (p90 below)" { "Task time" }
                    } }
                    tbody { @for x in b.features.iter().filter(|x| x.users > 0) {
                        @let adoption = (x.users as f64 / mau).min(1.0);
                        tr {
                            th scope="row" { (x.name.as_deref().unwrap_or(&x.key)) }
                            td.meter { (meter(adoption)) }
                            td.num { strong { (pct(adoption)) } br; span.muted.small { (compact(x.users)) " users" } }
                            td.num { (format!("{:.1}", ratio(x.uses, x.users))) }
                            td.num { (format!("{:.1}", ratio(x.user_days, x.users))) }
                            td.num {
                                @if x.declared && x.completed_users > 0 { (pct(ratio(x.completed_users, x.users).min(1.0))) }
                                @else { span.muted title="No success event declared or reached yet" { "—" } }
                            }
                            td.num {
                                @if let Some(t) = b.task_times.iter().find(|t| t.feature == x.key) {
                                    (duration(t.median_secs)) br; span.muted.small { "p90 " (duration(t.p90_secs)) }
                                } @else { span.muted { "—" } }
                            }
                        }
                    } }
                }
                p.muted.small { "Last 30 days. Adoption is the share of active users (MAU) who used the feature; completion is the share of them who reached its success event; task time runs from the first use to that event (within 30 minutes)." }
            }
        }
    }
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

/// Weekly retention cohorts: users first seen each week (rows) and the
/// share of them active again N weeks later (columns).
fn retention_grid(b: &Board) -> Markup {
    let rows: Vec<_> = b.retention.iter().filter(|r| r.users > 0).collect();
    let cols = rows.iter().map(|r| r.weeks.len()).max().unwrap_or(0);
    if rows.len() < 2 || cols == 0 {
        return html! {};
    }
    html! {
        section.card.wide {
            h2.mint { (icon("clock")) "Do they come back?" }
            table.plain.matrix {
                thead { tr {
                    th { "First seen (week of)" }
                    th.num { "Users" }
                    @for k in 1..=cols { th.num scope="col" { "Week " (k) } }
                } }
                tbody { @for r in &rows {
                    tr {
                        th scope="row" { (r.week) }
                        td.num.muted { (compact(r.users)) }
                        @for k in 0..cols {
                            @if let Some(share) = r.weeks.get(k) {
                                @let level = if *share <= 0.0 { 0 } else { ((share * 5.0).ceil() as u8).clamp(1, 5) };
                                td class={ "num heat-" (level) } { (pct(*share)) }
                            } @else {
                                td {}
                            }
                        }
                    }
                } }
            }
            p.muted.small { "Share of each week's new users who were active again N weeks later (weeks start on Monday, " (b.filters.as_ref().map_or("UTC", |f| f.tz.as_str())) ")." }
        }
    }
}

/// Roles × features: the share of each role's users who used each
/// feature in the last 30 days. One hue, darker = more adoption; the number
/// is always printed, so colour is never the only cue.
fn role_matrix(b: &Board) -> Markup {
    let roles: Vec<(&str, i64)> = b
        .roles
        .iter()
        .filter_map(|r| Some((r.role.as_deref()?, r.users)))
        .filter(|(_, n)| *n > 0)
        .take(6)
        .collect();
    let feats: Vec<&openlens_engine::FeatureUse> =
        b.features.iter().filter(|f| f.users > 0).take(10).collect();
    if roles.len() < 2 || feats.is_empty() {
        return html! {};
    }
    let cell = |role: &str, feat: &str| {
        b.role_features
            .iter()
            .find(|(r, f, _)| r == role && f == feat)
            .map_or(0, |x| x.2)
    };
    html! {
        section.card.wide {
            h2.violet { (icon("users")) "Who uses what" }
            table.plain.matrix {
                thead { tr { th { "Feature" } @for (r, n) in &roles { th.num scope="col" { (r) br; span.muted.small { (compact(*n)) " users" } } } } }
                tbody { @for f in &feats {
                    tr {
                        th scope="row" { (f.name.as_deref().unwrap_or(&f.key)) }
                        @for (r, n) in &roles {
                            @let share = cell(r, &f.key) as f64 / (*n).max(1) as f64;
                            @let level = if share <= 0.0 { 0 } else { ((share * 5.0).ceil() as u8).clamp(1, 5) };
                            td class={ "num heat-" (level) } title={ (pct(share)) " of " (r) " users used " (f.name.as_deref().unwrap_or(&f.key)) } {
                                @if share > 0.0 { (pct(share)) } @else { span.muted { "—" } }
                            }
                        }
                    }
                } }
            }
            p.muted.small { "Share of each role's users who used the feature, last 30 days. Empty cells are features a role never touches." }
        }
    }
}

/// One project in the switcher.
pub struct ProjectSummary {
    /// Project id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Events in the last 7 days, all environments.
    pub recent_events: i64,
    /// Latest event (UTC), if any.
    pub last_event: Option<String>,
    /// Time zone of the project's days (D-017).
    pub timezone: String,
}

/// All projects, busiest first.
pub fn projects(list: &[ProjectSummary], admin: bool) -> Markup {
    layout(
        "Projects",
        None,
        html! {
            div.heading {
                div { h1 { "Projects" } p.muted { "One per app. Pick one to open its Focus board." } }
                @if admin { a.button href="/users" { (icon("users")) "Team" } " " a.button href="/settings/storage" { (icon("layers")) "Storage and backups" } " " a.button href="/audit" { (icon("shield")) "Audit log" } }
            }
            @if admin {
                details.card.wide.new-project {
                    summary { (icon("plus")) "New project" }
                    form.invite method="post" action="/projects" {
                        label { "Name " input type="text" name="name" required maxlength="80" placeholder="My app"; }
                        label { "Time zone " input type="text" name="timezone" placeholder="Europe/Paris" maxlength="64"; }
                        button type="submit" { "Create" }
                    }
                    p.muted.small { "Agents do the same with " code { "openlens-cli init --name <app>" } ". Empty time zone: UTC." }
                }
            }
            ul.project-list {
                @for p in list {
                    li {
                        a.card href={ "/p/" (p.id) } {
                            h2.violet { img.picon src={ "/p/" (p.id) "/icon" } alt="" width="24" height="24"; (p.name) }
                            @if p.recent_events > 0 {
                                p { strong { (compact(p.recent_events)) } " events in the last 7 days" }
                            } @else {
                                p.muted { "No events in the last 7 days" }
                            }
                            @if let Some(t) = &p.last_event { p.muted.small { "Last event " (t.get(..16).unwrap_or(t)) " UTC" } }
                            p.muted.small { "Days in " (p.timezone) }
                        }
                    }
                }
            }
        },
    )
}

/// A page's click heatmap: density drawn in SVG (no script), plus a link to
/// see it over the real page when a recording of that page exists.
pub fn heatmap(
    p: &ProjectInfo<'_>,
    map: &openlens_engine::ClickMap,
    overlay: Option<&str>,
) -> Markup {
    let cols = openlens_engine::MAP_COLUMNS;
    let row = openlens_engine::MAP_ROW;
    let vw = if map.viewport > 0 { map.viewport } else { 1280 };
    let rows = map.cells.iter().map(|c| c.1).max().unwrap_or(0) + 2;
    let height = (rows * row).max(400);
    let max = map.cells.iter().map(|c| c.2).max().unwrap_or(1).max(1) as f64;
    let cw = vw as f64 / cols as f64;
    layout(
        "Heatmap",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Click heatmap" }
                p.muted { span.mono { (map.path) } " · " (compact(map.clicks)) " clicks with a position, last 30 days · median screen " (vw) " px wide" }
            } }
            @if let Some(href) = overlay {
                p { a.button href=(href) { (icon("play")) "See it over the real page" } }
            }
            @if map.cells.is_empty() {
                section.card { p.muted { "No clicks with a position on this page yet (positions are recorded by SDK 0.1 builds from 2026-11-22 on)." } }
            } @else {
                section.card.wide {
                    svg.heatmap viewBox={ "0 0 " (vw) " " (height) } role="img" aria-label={ "Click density on " (map.path) } {
                        rect.page x="0" y="0" width=(vw) height=(height) {}
                        @for (cx, cy, n, rage) in &map.cells {
                            @let o = format!("{:.2}", (0.15 + 0.85 * (*n as f64 / max)).min(1.0));
                            circle class=(if *rage > 0 { "dot rage" } else { "dot" })
                                cx=(format!("{:.0}", (*cx as f64 + 0.5) * cw)) cy=((cy * row) + row / 2)
                                r=(format!("{:.0}", (cw * 0.9).max(8.0))) fill-opacity=(o) {
                                title { (n) " clicks" @if *rage > 0 { ", " (rage) " rage" } }
                            }
                        }
                    }
                    p.muted.small { "Each dot is a 2% × 20 px area; darker means more clicks. Red marks rage clicks. The page is drawn at its median screen width." }
                }
            }
        },
    )
}

/// The heatmap drawn over a recorded visit of the page (player page).
pub fn heatmap_overlay(p: &ProjectInfo<'_>, path: &str, session: &str) -> Markup {
    layout(
        "Heatmap",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Click heatmap" }
                p.muted { span.mono { (path) } " · drawn over a recorded visit of this page (text is masked as recorded)" }
            } }
            link rel="stylesheet" href="/assets/player.css";
            section.card.wide {
                div #ol-player
                    data-src={ "/p/" (p.id) "/replays/" (session) "/events.json" }
                    data-heatmap={ "/p/" (p.id) "/heatmap.json?path=" (url_enc(path)) }
                    data-path=(path) {
                    p.muted { "Loading the page…" }
                }
            }
            script src="/assets/player.js" defer {}
        },
    )
}

/// The weekly digest page.
pub fn digest(p: &ProjectInfo<'_>, d: &openlens_engine::Digest, mailed: bool) -> Markup {
    let row = |label: &str, x: &openlens_engine::Delta| {
        let change = x.change();
        html! {
            tr {
                th scope="row" { (label) }
                td.num { strong { (compact(x.now)) } }
                td.num.muted { (compact(x.before)) }
                td.num {
                    @match change {
                        Some(c) if c.abs() >= 0.005 => span class=(if c > 0.0 { "up" } else { "down" }) { (format!("{:+.0}%", c * 100.0)) },
                        Some(_) => span.muted { "same" },
                        None => span.muted { "—" },
                    }
                }
            }
        }
    };
    layout(
        "Weekly digest",
        Some(p),
        html! {
            div.heading { div {
                h1 { "This week vs last" }
                p.muted { "Last 7 days against the 7 before, production. "
                    @if mailed { "Emailed every Monday." } @else { "Set OPENLENS_SMTP_URL and OPENLENS_DIGEST_TO to receive it by email every Monday." } }
            } }
            section.card.wide {
                table.plain {
                    thead { tr { th {} th.num { "This week" } th.num { "Last week" } th.num { "Change" } } }
                    tbody {
                        (row("Active users", &d.users))
                        (row("Sessions", &d.sessions))
                        (row("Errors", &d.errors))
                        @for (k, t) in &d.tasks { (row(&format!("Completed: {k}"), t)) }
                    }
                }
            }
            @if !d.insights.is_empty() {
                section.card.wide {
                    h2.violet { (icon("idea")) "What to focus on" }
                    ul { @for i in &d.insights { li { strong { (i.title) } " · " span.muted { (i.evidence) } br; span.small { "Next step: " (i.next_step) } } } }
                }
            }
        },
    )
}

/// One row of the team page (#216).
pub struct UserRow<'a> {
    /// The user.
    pub user: &'a openlens_meta::User,
    /// `active`, `invited`, `expired` or `disabled`.
    pub status: &'a str,
    /// When the pending invitation expires (UTC), if one is open.
    pub expires: Option<String>,
}

/// The team: invite, find, edit, disable and remove users (#216).
pub fn users(
    rows: &[UserRow<'_>],
    projects: &[String],
    new_link: Option<(&str, &str)>,
    q: &str,
    notice: Option<&str>,
) -> Markup {
    layout(
        "Team",
        None,
        html! {
            div.heading { div {
                h1 { "Team" }
                p.muted { "Admins change things (flags, settings, team); viewers look. The owner (admin token) always has access, so the last admin can't be removed or demoted." }
            } }
            @if let Some(n) = notice { p.notice role="status" { (n) } }
            @if let Some((email, link)) = new_link {
                section.card.wide {
                    h2.mint { (icon("task")) "Login link for " (email) }
                    p { "Send this one-time link (valid 7 days):" }
                    pre.snippet { code { (link) } }
                }
            }
            section.card.wide {
                h2.violet { (icon("users")) "Invite someone" }
                form.invite method="post" action="/users" {
                    label { "Email " input type="email" name="email" required; }
                    label { "Name " input type="text" name="name" maxlength="80"; }
                    label { "Role " select name="role" { option value="viewer" { "viewer" } option value="admin" { "admin" } } }
                    label { "Projects " input type="text" name="projects" placeholder="all" list="project-ids"; }
                    datalist #project-ids { @for p in projects { option value=(p) {} } }
                    button type="submit" { "Create the login link" }
                }
                p.muted.small { "Projects: comma-separated ids (" (projects.join(", ")) "), or empty for all." }
            }
            section.card.wide {
                form.inline method="get" action="/users" role="search" {
                    label { "Find " input type="text" name="q" value=(q) placeholder="name or email"; }
                    button.ghost type="submit" { "Search" }
                }
                @if rows.is_empty() { p.muted { @if q.is_empty() { "Nobody invited yet." } @else { "Nobody matches." } } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "User" } th { "Role" } th { "Projects" } th { "Status" } th { "Last sign-in" } th { span.sr-only { "Actions" } } } }
                        tbody { @for r in rows {
                            @let u = r.user;
                            tr {
                                td { @if let Some(n) = &u.name { strong { (n) } br; } span.small { (u.email) } }
                                td { span.chip { (u.role) } }
                                td.small { @match &u.projects { Some(p) => (p.join(", ")), None => span.muted { "all" } } }
                                td.small {
                                    span class={ "status " (match r.status { "active" => "good", "disabled" => "bad", _ => "muted" }) } { (r.status) }
                                    @if let Some(e) = &r.expires { br; span.muted { "until " (e) } }
                                }
                                td.small.muted { (u.last_sign_in.as_deref().and_then(|t| t.get(..16)).map(|t| t.replace('T', " ")).unwrap_or_else(|| "never".into())) }
                                td { a.button href={ "/users/" (u.id) } { "Edit" } }
                            }
                        } }
                    } }
                }
            }
        },
    )
}

/// Edit one user: profile, access, sign-in, invitation and removal (#216).
pub fn user_edit(u: &openlens_meta::User, status: &str, projects: &[String]) -> Markup {
    let base = format!("/users/{}", u.id);
    layout(
        "Team",
        None,
        html! {
            div.heading { div {
                h1 { (u.name.as_deref().unwrap_or(&u.email)) }
                p.muted { a href="/users" { "← Team" } " · " (status) }
            } }
            section.card.wide {
                h2.violet { (icon("users")) "Profile and access" }
                form.settings method="post" action=(base) {
                    label { "Name " input type="text" name="name" value=[u.name.as_deref()] maxlength="80"; }
                    label { "Email " input type="email" name="email" value=(u.email) required maxlength="200"; }
                    label { "Role " select name="role" {
                        option value="viewer" selected[u.role == "viewer"] { "viewer" }
                        option value="admin" selected[u.role == "admin"] { "admin" }
                    } }
                    label { "Projects " input type="text" name="projects" value=(u.projects.as_ref().map(|p| p.join(",")).unwrap_or_default()) placeholder="all" list="project-ids"; }
                    datalist #project-ids { @for p in projects { option value=(p) {} } }
                    button type="submit" { "Save" }
                }
                p.muted.small { "Changing the email keeps the user's sessions and history." }
            }
            section.card.wide {
                h2.sky { (icon("shield")) "Sign-in" }
                div.actions {
                    form.inline method="post" action={ (base) "/link" } { button type="submit" { (if status == "invited" || status == "expired" { "Resend the invitation" } else { "New login link" }) } }
                    @if status == "invited" {
                        form.inline method="post" action={ (base) "/revoke" } { button.ghost type="submit" { "Revoke the invitation" } }
                    }
                    @if u.disabled {
                        form.inline method="post" action={ (base) "/enable" } { button type="submit" { "Enable sign-in" } }
                    } @else {
                        form.inline method="post" action={ (base) "/disable" } { button.ghost type="submit" { "Disable sign-in" } }
                    }
                }
                p.muted.small { "Disabling ends their sessions at once and keeps the account; enabling lets them sign in again." }
            }
            section.card.wide.danger {
                h2.coral { (icon("alert")) "Remove" }
                form.inline method="post" action={ (base) "/delete" } {
                    label { "Type " code { (u.email) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                    button.danger type="submit" { "Remove from the team" }
                }
            }
        },
    )
}

/// My profile: the time zone I read times in, and my name (#215, #216).
pub fn me(
    name: Option<&str>,
    email: Option<&str>,
    tz: Option<&str>,
    notice: Option<&str>,
) -> Markup {
    layout(
        "My profile",
        None,
        html! {
            div.heading { div {
                h1 { "My profile" }
                p.muted { @if let Some(e) = email { (e) } @else { "Owner (admin token)" } }
            } }
            @if let Some(n) = notice { p.notice role="status" { (n) } }
            section.card.wide {
                form.settings method="post" action="/me" {
                    @if email.is_some() {
                        label { "Name " input type="text" name="name" value=[name] maxlength="80"; }
                    }
                    label { "Time zone " input type="text" name="timezone" value=[tz] placeholder="the project's" maxlength="64" list="zones"; }
                    datalist #zones { @for z in ["Europe/Paris", "Europe/London", "Europe/Berlin", "Europe/Rome", "America/New_York", "America/Los_Angeles", "Asia/Tokyo", "UTC"] { option value=(z) {} } }
                    button type="submit" { "Save" }
                }
                p.muted.small { "Every time and every day on the boards follows this zone (an IANA name like Europe/Paris). Empty: each project's own zone." }
            }
        },
    )
}

/// What the settings page shows of a project (#219).
pub struct SettingsView<'a> {
    /// Time zone of the project's days (D-017).
    pub timezone: &'a str,
    /// The app address set by an admin, if any.
    pub app_url: Option<&'a str>,
    /// The app address seen in recent pageviews.
    pub detected_url: Option<&'a str>,
    /// This server's public address (for the snippets).
    pub host: &'a str,
    /// Wiring check over the last 24 hours, as (label, count, hint).
    pub checks: &'a [(&'a str, i64, &'a str)],
    /// Version of the feature map, if one was pushed.
    pub map_version: Option<i64>,
    /// Admins see the forms.
    pub admin: bool,
    /// A notice to show at the top (after an action).
    pub notice: Option<&'a str>,
    /// Yesterday, in the project's zone (default export day).
    pub yesterday: &'a str,
    /// Alert settings (#256).
    pub alerts: AlertsView,
    /// Share links (#229).
    pub shares: Vec<openlens_meta::ShareLink>,
}

/// A project's alert settings as the page shows them (#256).
#[derive(Debug, Clone, Default)]
pub struct AlertsView {
    /// Probe and alert when down.
    pub down: bool,
    /// Minutes before announcing.
    pub down_wait: u32,
    /// Alert when traffic stops.
    pub traffic: bool,
    /// Alert on regressions and new errors.
    pub regressions: bool,
    /// Probe path.
    pub health_path: String,
    /// Email alerts.
    pub email: bool,
    /// Webhook URL.
    pub webhook: String,
    /// ntfy topic.
    pub ntfy: String,
    /// SMTP is configured.
    pub smtp: bool,
}

/// A project's settings: name, app address, time zone, capture key and
/// snippets, wiring check, export, and deletion (#219).
pub fn project_settings(p: &ProjectInfo<'_>, v: &SettingsView<'_>) -> Markup {
    let base = format!("/p/{}/settings", p.id);
    let script = format!(
        r#"<script src="{}/c.js" data-key="{}" defer></script>"#,
        v.host, p.key
    );
    let npm = format!(
        "import {{ init }} from '@openlens/web';\ninit({{ host: '{}', key: '{}' }});",
        v.host, p.key
    );
    let server = format!("OPENLENS_HOST={}\nOPENLENS_KEY={}", v.host, p.key);
    layout(
        "Settings",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Settings" }
                p.muted { "Everything agents do with " code { "openlens-cli" } " for this project, here too." }
            } }
            @if let Some(n) = v.notice { p.notice role="status" { (n) } }
            section.card.wide {
                h2.violet { (icon("gear")) "Project" }
                @if v.admin {
                    form.settings method="post" action=(base) {
                        label { "Name " input type="text" name="name" value=(p.name) required maxlength="80"; }
                        label { "App address "
                            input type="url" name="app_url" value=[v.app_url] placeholder=(v.detected_url.unwrap_or("https://app.example.com")) maxlength="200";
                        }
                        label { "Time zone " input type="text" name="timezone" value=(v.timezone) required maxlength="64"; }
                        button type="submit" { "Save" }
                    }
                } @else {
                    dl.facts {
                        dt { "Name" } dd { (p.name) }
                        dt { "App address" } dd { (v.app_url.or(v.detected_url).unwrap_or("—")) }
                        dt { "Time zone" } dd { (v.timezone) }
                    }
                }
                p.muted.small {
                    "The app address gives the project its icon (the app's favicon). Empty: "
                    @match v.detected_url { Some(u) => { "detected from pageviews, " span.mono { (u) } "." }, None => "detected from pageviews once some arrive." }
                    " Days and weeks of every KPI start in the time zone (an IANA name like Europe/Paris)."
                }
            }
            section.card.wide {
                h2.sky { (icon("code")) "Install" }
                p { "Capture key " code.mono { (p.key) } " (public: it only sends events)." }
                h3 { "Script tag" }
                pre.snippet { code { (script) } }
                h3 { "npm" }
                pre.snippet { code { (npm) } }
                h3 { "Server SDKs (Node, Python, Go, Ruby, PHP)" }
                pre.snippet { code { (server) } }
                p.muted.small { "Agents: " a href={ (v.host) "/agents.md" } { "the wiring guide" } "." }
                @if v.admin {
                    form.inline method="post" action={ (base) "/rotate-key" } {
                        label.check { input type="checkbox" name="confirm" value="yes" required; " The old key stops working at once: every app must get the new one" }
                        button.ghost type="submit" { "Rotate the key" }
                    }
                }
            }
            section.card.wide {
                h2.mint { (icon("task")) "Wiring check" }
                p.muted.small { "Last 24 hours, all environments (what " code { "openlens-cli verify" } " checks)." }
                ul.checks {
                    @for (label, n, hint) in v.checks {
                        li.ok[*n > 0] {
                            (icon(if *n > 0 { "task" } else { "alert" }))
                            span { strong { (label) } " " (compact(*n)) }
                            @if *n == 0 { span.muted.small { " · " (hint) } }
                        }
                    }
                    li.ok[v.map_version.is_some()] {
                        (icon(if v.map_version.is_some() { "task" } else { "alert" }))
                        span { strong { "Feature map" } " " @match v.map_version { Some(n) => { "v" (n) }, None => "none" } }
                        @if v.map_version.is_none() { span.muted.small { " · push one with " code { "openlens-cli map push" } } }
                    }
                }
            }
            section.card.wide #alerts {
                h2.coral { (icon("bolt")) "Alerts" }
                p.small { "Know when the app is down: OpenLens checks the app address every minute (" a href={ "/p/" (p.id) "/uptime" } { "Uptime" } "), notices when the traffic stops for much longer than usual (after four weeks of history), and, every hour, when the board finds a regression (a release or flag with more errors, pages turning slow) or a new error hits several users. One message when an incident starts, one when it ends." }
                @if v.admin {
                    form.settings method="post" action={ (base) "/alerts" } {
                        label.check { input type="checkbox" name="down" checked[v.alerts.down]; " App down (needs the app address)" }
                        label { "Announce after (min) " input type="number" name="down_wait" min="1" max="60" value=(v.alerts.down_wait); }
                        label { "Health path " input type="text" name="health_path" value=(v.alerts.health_path) maxlength="200"; }
                        label.check { input type="checkbox" name="traffic" checked[v.alerts.traffic]; " Traffic stopped" }
                        label.check { input type="checkbox" name="regressions" checked[v.alerts.regressions]; " Regressions and new errors" }
                        label.check { input type="checkbox" name="email" checked[v.alerts.email]; " Email" @if !v.alerts.smtp { span.muted { " (SMTP not set)" } } }
                        label { "Webhook (Slack, Discord…) " input type="url" name="webhook" value=(v.alerts.webhook) placeholder="https://hooks.slack.com/…" maxlength="500"; }
                        label { "ntfy topic " input type="text" name="ntfy" value=(v.alerts.ntfy) placeholder="my-app-alerts" maxlength="300"; }
                        button type="submit" { "Save" }
                    }
                    form.inline method="post" action={ (base) "/alerts/test" } { button.soft type="submit" { "Send a test alert" } }
                    p.muted.small { "Emails go to " code { "OPENLENS_ALERT_TO" } " (or the digest list, or the admins). ntfy: a topic on ntfy.sh, or a full https:// topic URL of your own server; install the ntfy app to get pushes on a phone." }
                }
            }
            @if v.admin {
                section.card.wide #share {
                    h2.sky { (icon("route")) "Share links" }
                    p.small { "A read-only link to the Focus board or the weekly digest, for people without an account: no SQL, people, replays or settings behind it. Filters are fixed when the link is made." }
                    form.settings method="post" action={ (base) "/share" } {
                        label { "What " select name="scope" { option value="board" { "Focus board" } option value="digest" { "Weekly digest" } } }
                        label { "Release " input type="text" name="release" placeholder="all" maxlength="100"; }
                        label { "Expires after (days, 0 = never) " input type="number" name="days" min="0" max="366" value="30"; }
                        label { "May be embedded on " input type="text" name="embed" placeholder="https://intranet.example.com" maxlength="500"; }
                        button type="submit" { "Make a link" }
                    }
                    @if !v.shares.is_empty() {
                        div.table-scroll { table.plain {
                            thead { tr { th { "What" } th { "Release" } th { "Embed" } th { "Expires" } th { "By" } th {} } }
                            tbody { @for l in &v.shares {
                                tr {
                                    td { (l.scope) } td.small { (l.release.as_deref().unwrap_or("all")) } td.small { (l.embed.as_deref().unwrap_or("no")) }
                                    td.small { @match l.expires_at { Some(t) => (chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| d.format("%Y-%m-%d").to_string())), None => "never" } }
                                    td.small { (l.created_by) }
                                    td { form.inline method="post" action={ (base) "/share/" (l.id) "/revoke" } { button.ghost type="submit" { "Revoke" } } }
                                }
                            } }
                        } }
                    }
                }
                section.card.wide {
                    h2.violet { (icon("code")) "Explore the data" }
                    p { a.button href={ "/p/" (p.id) "/sql" } { "Read-only SQL" } " " a.button href={ "/p/" (p.id) "/breakdown?by=role" } { "Break down" } }
                }
            }
            section.card.wide {
                h2.sun { (icon("down")) "Export" }
                form.inline method="get" action={ "/p/" (p.id) "/export" } {
                    label { "Day " input type="date" name="date" value=(v.yesterday) required; }
                    button type="submit" { "Download NDJSON" }
                }
                p.muted.small { "One line per event of that day (UTC partitions), up to 500,000." }
            }
            @if v.admin {
                section.card.wide.danger {
                    h2.coral { (icon("alert")) "Delete the project" }
                    p { "Removes its events, replays, source maps, people, flags, surveys and feature map. There is no undo (backups excepted)." }
                    form.inline method="post" action={ (base) "/delete" } {
                        label { "Type " code { (p.name) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                        button.danger type="submit" { "Delete forever" }
                    }
                }
            }
        },
    )
}

/// Micro-surveys and their answers.
pub fn surveys(
    p: &ProjectInfo<'_>,
    list: &[openlens_flags::survey::Survey],
    results: &[openlens_engine::SurveyResults],
) -> Markup {
    layout(
        "Surveys",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Micro-surveys" }
                    p.muted { "One question, right after a moment of value, to a sample of users, once each. Create them here or let agents do it (" code { "survey_set" } "); answers are " code { "$survey" } " events." }
                }
                a.button href={ "/p/" (p.id) "/surveys-edit" } { (icon("plus")) "New survey" }
            }
            @if list.is_empty() {
                section.card { p.muted { "No survey yet. Ask your agent, for example: “after entry_saved, ask 10% of users how useful writing felt, 1 to 5”." } }
            }
            @for s in list {
                @let r = results.iter().find(|r| r.key == s.key);
                @let answered: i64 = r.map_or(0, |r| r.answers.iter().map(|a| a.1).sum());
                @let max = r.map_or(1, |r| r.answers.iter().map(|a| a.1).max().unwrap_or(1)).max(1) as f64;
                section.card.wide {
                    h2.violet { (icon("idea")) (s.question) }
                    p.small.muted {
                        span.chip { (s.kind) } " after " code { (s.trigger) } " · " (s.sample) "% of "
                        @if s.roles.is_empty() { "everyone" } @else { (s.roles.join(", ")) }
                        " · " (compact(answered)) " answers"
                        @if let Some(r) = r { " · " (compact(r.dismissed)) " dismissed" }
                        " · " a href={ "/p/" (p.id) "/surveys-edit?key=" (s.key) } { "Edit" }
                    }
                    form.inline method="post" action={ "/p/" (p.id) "/surveys/" (s.key) } {
                        label { input type="checkbox" name="enabled" checked[s.enabled]; " on" }
                        label { "sample " input type="number" name="sample" min="0" max="100" value=(s.sample); "%" }
                        button.soft type="submit" { "Save" }
                    }
                    @if let Some(r) = r {
                        @if let Some(m) = r.mean { p { "Mean " strong { (format!("{m:.1}")) } " / 5" } }
                        @if s.kind != "text" && !r.answers.is_empty() {
                            table.bars { tbody { @for (a, n) in &r.answers {
                                tr { th scope="row" { (a) } td.meter { (meter(*n as f64 / max)) } td.num { (compact(*n)) } }
                            } } }
                        }
                        @if !r.by_role.is_empty() {
                            p.small { "By role: " @for (i, (role, m, n)) in r.by_role.iter().enumerate() { @if i > 0 { " · " } strong { (role) } " " (format!("{m:.1}")) span.muted { " (" (n) ")" } } }
                        }
                        @if !r.latest_text.is_empty() {
                            h3.small { "Latest answers" }
                            ul { @for t in &r.latest_text { li { q { (t) } } } }
                        }
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn plain_wording_replaces_the_card_text_and_keeps_the_original() {
        let mut b = Board {
            total_events: 10,
            ..Board::default()
        };
        b.insights.push(openlens_engine::Insight {
            kind: "churn_point",
            title: "72% of churned users were last seen in billing".into(),
            evidence: "113 of 156 churned users".into(),
            next_step: "Look at what happens in billing.".into(),
            score: 1.0,
            subject: "billing".into(),
        });
        let p = ProjectInfo {
            id: "p",
            name: "P",
            key: "pk",
        };
        let plain = [(
            "Most people who left (72%) were in billing last".to_owned(),
            "Check the billing page first.".to_owned(),
        )];
        let html = render(board(
            &p,
            &b,
            &Filters::default(),
            "http://x",
            &plain,
            None,
            &[],
        ));
        assert!(html.contains("Most people who left (72%)"));
        assert!(html.contains("Original: 72% of churned users were last seen in billing"));
        assert!(html.contains("Check the billing page first."));
        let html = render(board(
            &p,
            &b,
            &Filters::default(),
            "http://x",
            &[],
            None,
            &[],
        ));
        assert!(
            html.contains("72% of churned users were last seen in billing")
                && !html.contains("Original:")
        );
    }
}

#[cfg(test)]
mod diff_tests {
    #[test]
    fn line_diff_keeps_common_lines() {
        let d = super::line_diff("a\nb\nc\n", "a\nx\nc\nd\n");
        let s: String = d.iter().map(|(c, l)| format!("{c}{l};")).collect();
        assert_eq!(s, " a;-b;+x; c;+d;");
        assert!(super::line_diff("", "").is_empty());
    }
}

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
                                    ("/events", "events", "Events"),
                                    ("/pages", "layers", "Pages"),
                                    ("/errors", "alert", "Errors"),
                                    ("/replays", "play", "Replays"),
                                    ("/flags", "flag", "Flags"),
                                    ("/surveys", "idea", "Surveys"),
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
                                    p { a.button href={ "/p/" (pid) "/watch?churned_at=" (url_enc(&c.subject)) } { (icon("play")) "Watch a session that churned here" } }
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
                p.muted { "What " (scope) " did lately in " strong { (f.environment) } ". " a href={ "/p/" (p.id) "/digest" } { "This week vs last →" } }
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
                                td.mono { a href={ "/p/" (p.id) "/events?person=" (url_enc(&r.distinct_id)) } title="This user's events" { (r.distinct_id) } }
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
                @if admin { a.button href="/users" { (icon("users")) "Team" } }
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
        let html = render(board(&p, &b, &Filters::default(), "http://x", &plain));
        assert!(html.contains("Most people who left (72%)"));
        assert!(html.contains("Original: 72% of churned users were last seen in billing"));
        assert!(html.contains("Check the billing page first."));
        let html = render(board(&p, &b, &Filters::default(), "http://x", &[]));
        assert!(
            html.contains("72% of churned users were last seen in billing")
                && !html.contains("Original:")
        );
    }
}

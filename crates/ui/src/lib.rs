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
                            a href={ "/p/" (p.id) "/replays" } { (icon("play")) "Replays" }
                            a href={ "/p/" (p.id) "/flags" } { (icon("flag")) "Flags" }
                            a href={ "/p/" (p.id) "/surveys" } { (icon("idea")) "Surveys" }
                        }
                        a.project href="/projects" title="Switch project" { (p.name) " " (icon("down")) }
                    }
                    nav.theme.solo[project.is_none()] aria-label="Theme" {
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
pub fn login(failed: bool, sso: bool) -> Markup {
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
            label.check { input type="checkbox" name="internal" value="1" checked[f.internal]; " Include the team" }
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
                (tile("violet", "users", "Daily active users", &compact(b.users.dau), &{
                    let mut sub = format!("{} weekly · {} monthly", compact(b.users.wau), compact(b.users.mau));
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
                        thead { tr { th { "When (UTC)" } th { "Event" } th { "User" } th { "Role" } th { "Page" } th { "Feature" } th { "Release" } th { span.sr-only { "Replay" } } } }
                        tbody { @for r in rows {
                            tr {
                                td.muted.mono { (r.timestamp.get(..19).unwrap_or(&r.timestamp)) }
                                td { span.chip { (r.event) } }
                                td.mono { (r.distinct_id) }
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
                                td.small.muted { (g.last_seen.get(..16).unwrap_or(&g.last_seen)) }
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
                        thead { tr { th { "Started (UTC)" } th { "User" } th.num { "Length" } th.num { "Size" } th { span.sr-only { "Play" } } } }
                        tbody { @for r in list {
                            @let started = r.started.format("%Y-%m-%d %H:%M").to_string();
                            @let secs = (r.updated - r.started).num_seconds().max(0) as f64;
                            tr {
                                td.mono { (started) }
                                td.mono { a href={ "/p/" (p.id) "/replays?u=" (r.distinct_id) } { (r.distinct_id) } }
                                td.num { (duration(secs)) }
                                td.num.muted { (compact((r.bytes / 1024) as i64)) " kB" }
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
pub fn player(p: &ProjectInfo<'_>, session: &str, who: Option<&str>, at: Option<&str>) -> Markup {
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
                div #ol-player data-src={ "/p/" (p.id) "/replays/" (session) "/events.json" } data-at=[at] {
                    p.muted { "Loading the recording…" }
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
            p.muted.small { "Share of each week's new users who were active again N weeks later (UTC weeks)." }
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
            ul.project-list {
                @for p in list {
                    li {
                        a.card href={ "/p/" (p.id) } {
                            h2.violet { (icon("board")) (p.name) }
                            @if p.recent_events > 0 {
                                p { strong { (compact(p.recent_events)) } " events in the last 7 days" }
                            } @else {
                                p.muted { "No events in the last 7 days" }
                            }
                            @if let Some(t) = &p.last_event { p.muted.small { "Last event " (t.get(..16).unwrap_or(t)) " UTC" } }
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

/// Team: who can see the dashboard (admins only).
pub fn users(
    users: &[openlens_meta::User],
    projects: &[String],
    new_link: Option<(&str, &str)>,
) -> Markup {
    layout(
        "Team",
        None,
        html! {
            div.heading { div {
                h1 { "Team" }
                p.muted { "Admins change things (flags, team); viewers look. The owner (admin token) always has access." }
            } }
            @if let Some((email, link)) = new_link {
                section.card.wide {
                    h2.mint { (icon("task")) "Invitation for " (email) }
                    p { "Send this one-time link (valid 7 days):" }
                    p { code { (link) } }
                }
            }
            section.card.wide {
                h2.violet { (icon("users")) "Invite someone" }
                form.invite method="post" action="/users" {
                    label { "Email " input type="email" name="email" required; }
                    label { "Role " select name="role" { option value="viewer" { "viewer" } option value="admin" { "admin" } } }
                    label { "Projects " input type="text" name="projects" placeholder="all" list="project-ids"; }
                    datalist #project-ids { @for p in projects { option value=(p) {} } }
                    button type="submit" { "Create the login link" }
                }
                p.muted.small { "Projects: comma-separated ids (" (projects.join(", ")) "), or empty for all." }
            }
            section.card.wide {
                @if users.is_empty() { p.muted { "Nobody invited yet." } }
                @else {
                    table.plain {
                        thead { tr { th { "Email" } th { "Role" } th { "Projects" } th { "Invited" } th {} } }
                        tbody { @for u in users {
                            tr {
                                td { (u.email) }
                                td { span.chip { (u.role) } }
                                td.small { @match &u.projects { Some(p) => (p.join(", ")), None => span.muted { "all" } } }
                                td.small.muted { (u.created_at.get(..10).unwrap_or(&u.created_at)) }
                                td { form method="post" action={ "/users/" (u.id) "/delete" } { button.ghost type="submit" { "Remove" } } }
                            }
                        } }
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
            div.heading { div {
                h1 { "Micro-surveys" }
                p.muted { "One question, right after a moment of value, to a sample of users, once each. Agents create them (" code { "survey_set" } "); answers are " code { "$survey" } " events." }
            } }
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

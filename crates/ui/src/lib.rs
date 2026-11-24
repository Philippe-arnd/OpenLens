//! Web dashboard: server-rendered HTML (maud) and SVG charts, no client-side
//! JavaScript (D-011).
//!
//! Minimal, colourful and efficient, with a playful touch: stat tiles that
//! tilt a little on hover, a colour-blob logo that slowly changes shape, friendly copy. Motion
//! respects `prefers-reduced-motion`; dark mode follows the system.
//!
//! Layer: L4 (see `docs/architecture.md`).

use maud::{DOCTYPE, Markup, PreEscaped, html};

mod audience;
mod board;
mod components;
mod explore;
mod features;
mod goals;
mod icons;
mod manage;
mod settings;
mod team;
mod world;
pub use audience::*;
pub use board::*;
pub use components::*;
pub use explore::*;
pub use features::*;
pub use goals::*;
pub use icons::icon;
pub use manage::*;
use openlens_engine::{Board, EventRow, Filters};
pub use settings::*;
pub use team::*;

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

/// The reader's display choices (#300): the theme and the sidebar. The
/// server says where to read them ([`set_prefs_source`]); `layout()`
/// writes them on `<html>`, so nothing rewrites the page afterwards.
#[derive(Debug, Clone, Copy, Default)]
pub struct Prefs {
    /// `light` or `dark`; `None` follows the system.
    pub theme: Option<&'static str>,
    /// The sidebar is folded.
    pub nav_collapsed: bool,
}

static PREFS: std::sync::OnceLock<fn() -> Prefs> = std::sync::OnceLock::new();

/// Where pages read the current request's [`Prefs`] (the server keeps them
/// per request). Set once; later calls are ignored.
pub fn set_prefs_source(source: fn() -> Prefs) {
    let _ = PREFS.set(source);
}

fn prefs() -> Prefs {
    PREFS.get().map_or_else(Prefs::default, |f| f())
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
    let p = prefs();
    html! {
        (DOCTYPE)
        html lang="en" data-theme=[p.theme] data-nav=[p.nav_collapsed.then_some("collapsed")] {
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
    let p = prefs();
    html! {
        (DOCTYPE)
        html lang="en" data-theme=[p.theme] data-nav=[p.nav_collapsed.then_some("collapsed")] {
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
                                    ("/ask", "spark", "Ask"),
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
                                    ("/tickets", "task", "Tickets"),
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

/// The page a login link opens: a button that uses the link. A GET never
/// uses it, so a link preview (Slack, Teams, Outlook Safe Links) leaves it
/// valid for the person it was sent to (#315).
pub fn login_continue(action: &str) -> Markup {
    layout(
        "Log in",
        None,
        html! {
            section.login {
                h1 { "Hello " span.wave aria-hidden="true" { "👋" } }
                p.muted { "This link opens your board once. Continue to log in." }
                form method="post" action=(action) {
                    button type="submit" autofocus { "Continue to my board" }
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

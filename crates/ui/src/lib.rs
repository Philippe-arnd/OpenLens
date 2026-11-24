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
mod feature_page;
mod features;
mod glossary;
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
pub use feature_page::*;
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

/// A newer release (#198), and whether it fixes security advisories (#416).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// Its tag, e.g. `v0.5.0`.
    pub tag: String,
    /// Its notes carry a *Security* section.
    pub security: bool,
}

static UPDATE: std::sync::RwLock<Option<Update>> = std::sync::RwLock::new(None);

/// Record the newest release when it is newer than this one (the server
/// checks once a day), so every page's footer can say so.
pub fn set_update_available(update: Option<Update>) {
    if let Ok(mut u) = UPDATE.write() {
        *u = update;
    }
}

fn update_available() -> Option<Update> {
    UPDATE.read().ok().and_then(|u| u.clone())
}

/// The footer's update notice: louder for a security release.
fn update_notice(u: &Update) -> Markup {
    let href = format!(
        "https://github.com/Philippe-arnd/OpenLens/releases/tag/{}",
        u.tag
    );
    html! {
        @if u.security {
            " · " a.update-security href=(href) { "Security update available: " (u.tag) " — fixes published advisories, upgrade soon" }
        } @else {
            " · " a href=(href) { "Update available: " (u.tag) }
        }
    }
}

/// One sidebar entry (#402): a section whose pages are tabs. `tabs` are
/// (path under `/p/<id>`, tab label, the page titles that belong to it).
struct NavGroup {
    label: &'static str,
    icon: &'static str,
    tabs: &'static [(&'static str, &'static str, &'static [&'static str])],
}

/// The project sidebar: eight entries instead of twenty-one (#402). Every
/// page keeps its URL; a section opens its first tab.
const NAV: &[NavGroup] = &[
    NavGroup {
        label: "Focus board",
        icon: "board",
        tabs: &[("", "Focus board", &["Focus board"])],
    },
    NavGroup {
        label: "Ask",
        icon: "spark",
        tabs: &[("/ask", "Ask", &["Ask"])],
    },
    NavGroup {
        label: "Live",
        icon: "pulse",
        tabs: &[
            ("/live", "Live", &["Live"]),
            ("/uptime", "Uptime", &["Uptime"]),
            ("/timeline", "Timeline", &["Timeline"]),
        ],
    },
    NavGroup {
        label: "Behaviour",
        icon: "events",
        tabs: &[
            ("/events", "Events", &["Events"]),
            ("/pages", "Pages", &["Pages", "Heatmap"]),
            ("/forms", "Forms", &["Forms"]),
            ("/flows", "Feature flow", &["Feature flow"]),
        ],
    },
    NavGroup {
        label: "Quality",
        icon: "alert",
        tabs: &[
            ("/errors", "Errors", &["Errors"]),
            ("/replays", "Replays", &["Replays", "Replay"]),
        ],
    },
    NavGroup {
        label: "People",
        icon: "globe",
        tabs: &[
            ("/audience", "Audience", &["Audience", "Person"]),
            ("/acquisition", "Acquisition", &["Acquisition"]),
            ("/churn", "Churn", &["Churn"]),
            ("/goals", "Goals", &["Goals"]),
        ],
    },
    NavGroup {
        label: "Product",
        icon: "layers",
        tabs: &[
            ("/features", "Feature map", &["Feature map"]),
            ("/flags", "Flags", &["Flags"]),
            ("/surveys", "Surveys", &["Surveys"]),
            ("/tickets", "Tickets", &["Tickets", "New ticket"]),
            ("/digest", "Digest", &["Weekly digest"]),
        ],
    },
    NavGroup {
        label: "Settings",
        icon: "gear",
        tabs: &[("/settings", "Settings", &["Settings"])],
    },
];

/// The section and tab index a page title belongs to. The project's own
/// name is the board's title.
fn group_of(title: &str, project: &str) -> Option<&'static NavGroup> {
    let title = if title == project {
        "Focus board"
    } else {
        title
    };
    NAV.iter()
        .find(|g| g.tabs.iter().any(|(_, _, titles)| titles.contains(&title)))
}

/// The tabs of the current page's section, when it has more than one.
/// Plain links with `aria-current`, so the keyboard and no-JS just work.
fn section_tabs(title: &str, p: &ProjectInfo<'_>) -> Markup {
    let Some(g) = group_of(title, p.name).filter(|g| g.tabs.len() > 1) else {
        return html! {};
    };
    let items: Vec<(String, &str, bool)> = g
        .tabs
        .iter()
        .map(|(href, label, titles)| {
            (
                format!("/p/{}{href}", p.id),
                *label,
                titles.contains(&title),
            )
        })
        .collect();
    components::tabs(g.label, &items)
}

/// The instance pages, as one tab group (#403): path, label, titles.
const INSTANCE_TABS: &[(&str, &str, &[&str])] = &[
    ("/settings/tokens", "API tokens", &["API tokens"]),
    ("/settings/ai", "AI", &["AI"]),
    ("/settings/storage", "Storage", &["Storage and backups"]),
    ("/users", "Team", &["Team"]),
    ("/me", "My profile", &["My profile"]),
];

fn instance_tabs(title: &str) -> Markup {
    if !INSTANCE_TABS.iter().any(|t| t.2.contains(&title)) {
        return html! {};
    }
    let items: Vec<(String, &str, bool)> = INSTANCE_TABS
        .iter()
        .map(|(href, label, titles)| ((*href).to_owned(), *label, titles.contains(&title)))
        .collect();
    components::tabs("Instance", &items)
}

/// The project whose menu the instance pages keep (#403), decided by the
/// server per request: the last one visited, if the viewer still sees it,
/// else their only project.
#[derive(Debug, Clone, Default)]
pub struct NavProject {
    /// Id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Capture key.
    pub key: String,
}

static NAV_PROJECT: std::sync::OnceLock<fn() -> Option<NavProject>> = std::sync::OnceLock::new();

/// Where instance pages read the request's [`NavProject`]. Set once.
pub fn set_nav_project_source(source: fn() -> Option<NavProject>) {
    let _ = NAV_PROJECT.set(source);
}

fn nav_project() -> Option<NavProject> {
    NAV_PROJECT.get().and_then(|f| f())
}

/// A breadcrumb (#403): one shared component for every page; the last
/// crumb is the current page.
#[derive(Debug, Clone, Default)]
pub(crate) struct Crumbs(pub Vec<(String, Option<String>)>);

impl Crumbs {
    /// The default trail of a page: `Projects › <project> › <section> ›
    /// <tab>`, or `Instance › <page>`.
    pub(crate) fn of(title: &str, project: Option<&ProjectInfo<'_>>) -> Self {
        let mut c = Vec::new();
        if let Some(t) = INSTANCE_TABS.iter().find(|t| t.2.contains(&title)) {
            c.push(("Instance".to_owned(), None));
            c.push((t.1.to_owned(), None));
            return Self(c);
        }
        let Some(p) = project else {
            return Self(c);
        };
        c.push(("Projects".to_owned(), Some("/projects".to_owned())));
        c.push((p.name.to_owned(), Some(format!("/p/{}", p.id))));
        if let Some(g) = group_of(title, p.name) {
            if g.label != "Focus board" {
                let first = format!("/p/{}{}", p.id, g.tabs[0].0);
                c.push((g.label.to_owned(), (g.tabs.len() > 1).then_some(first)));
            }
            if g.tabs.len() > 1
                && let Some(t) = g.tabs.iter().find(|t| t.2.contains(&title))
            {
                c.push((t.1.to_owned(), None));
            }
        } else if title != p.name {
            c.push((title.to_owned(), None));
        }
        Self(c)
    }

    fn render(&self) -> Markup {
        if self.0.len() < 2 {
            return html! {};
        }
        let last = self.0.len() - 1;
        html! {
            nav.crumbs aria-label="Breadcrumb" { ol {
                @for (i, (label, href)) in self.0.iter().enumerate() {
                    li {
                        @match (href, i == last) {
                            (_, true) => span aria-current="page" { (label) },
                            (Some(h), false) => a href=(h) { (label) },
                            (None, false) => span { (label) },
                        }
                    }
                }
            } }
        }
    }
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
    // Instance pages keep the last project's menu (#403).
    let remembered = if project.is_none() && INSTANCE_TABS.iter().any(|t| t.2.contains(&title)) {
        nav_project()
    } else {
        None
    };
    let held = remembered.as_ref().map(|n| ProjectInfo {
        id: &n.id,
        name: &n.name,
        key: &n.key,
    });
    let project = project.or(held.as_ref());
    let crumbs = Crumbs::of(title, project);
    layout_with(title, project, &crumbs, body)
}

/// [`layout`] with an explicit breadcrumb (#403; the feature pages, #411,
/// pass their own).
fn layout_with(
    title: &str,
    project: Option<&ProjectInfo<'_>>,
    crumbs: &Crumbs,
    body: Markup,
) -> Markup {
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
                                @for g in NAV {
                                    a href={ "/p/" (p.id) (g.tabs[0].0) } title=(g.label) aria-current=[group_of(title, p.name).is_some_and(|c| c.label == g.label).then_some("page")] {
                                        (icon(g.icon)) span.label { (g.label) }
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
                        main {
                            (crumbs.render())
                            @if let Some(p) = project { (section_tabs(title, p)) }
                            (instance_tabs(title))
                            (body)
                        }
                        footer {
                            "OpenLens " (env!("CARGO_PKG_VERSION")) " · map → measure → focus → act → compare"
                            @if let Some(u) = update_available() {
                                (update_notice(&u))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Login page.
pub fn login(failed: bool, sso: bool, email: bool, password: bool) -> Markup {
    let token_form = html! {
        form method="post" action="/login" {
            label for="token" { "Admin token" }
            input #token name="token" type="password" autocomplete="off" required autofocus[!password];
            button type="submit" { "Open my board" }
        }
    };
    layout(
        "Log in",
        None,
        html! {
            section.login {
                h1 { "Hello again " span.wave aria-hidden="true" { "👋" } }
                @if password {
                    p.muted { "Sign in with your email and password." }
                    @if failed { p.alert role="alert" { "That didn't work. Check and try again; after a few failed tries, wait 15 minutes." } }
                    form method="post" action="/login/password" {
                        label for="pw-email" { "Email" }
                        input #pw-email name="email" type="email" autocomplete="username" required autofocus;
                        label for="pw" { "Password" }
                        input #pw name="password" type="password" autocomplete="current-password" required;
                        button type="submit" { "Log in" }
                    }
                    // The way back in, always (#428): a lost password, a locked account.
                    details.token-login {
                        summary { "Use the admin token" }
                        (token_form)
                    }
                } @else {
                    p.muted { "Paste the admin token, or open the login link your agent sent you." }
                    @if failed { p.alert role="alert" { "That token didn't work. Try again?" } }
                    (token_form)
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

/// [`message`] with markup (a link back, for instance).
pub fn message_html(title: &str, body: Markup) -> Markup {
    layout(
        title,
        None,
        html! { section.login { h1 { (title) } p { (body) } } },
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
    fn a_security_release_gets_a_louder_notice() {
        let plain = update_notice(&Update {
            tag: "v0.5.0".into(),
            security: false,
        })
        .into_string();
        let sec = update_notice(&Update {
            tag: "v0.5.0".into(),
            security: true,
        })
        .into_string();
        assert!(plain.contains("Update available: v0.5.0"));
        assert!(!plain.contains("update-security"));
        assert!(sec.contains("class=\"update-security\""));
        assert!(sec.contains("Security update available: v0.5.0"));
        assert!(sec.contains("releases/tag/v0.5.0"));
    }

    /// Every card and tile of the board opens its detail (#409): a new one
    /// must use `tile_to` / `to(...)`, or say `no_detail` on purpose.
    #[test]
    fn every_board_card_and_tile_leads_somewhere() {
        let src = include_str!("board.rs");
        let body = &src[src.find("pub fn board(").unwrap()..];
        assert!(
            !body.contains("(tile(\""),
            "a board tile without a detail: use tile_to"
        );
        for line in body.lines().filter(|l| l.trim_start().starts_with("h2.")) {
            assert!(
                line.contains("(to(") || line.contains("no_detail"),
                "card without a detail: {line}"
            );
        }
        let f = Filters {
            environment: "production".into(),
            role: Some("admin".into()),
            internal: true,
            ..Filters::default()
        };
        assert_eq!(
            detail_url("d", "/errors", &f),
            "/p/d/errors?env=production&role=admin&internal=1"
        );
        assert_eq!(
            detail_url("d", "/breakdown?by=role", &f),
            "/p/d/breakdown?by=role&env=production&role=admin&internal=1"
        );
        let shared = render_shared(|| to("/x", "Releases")).into_string();
        assert_eq!(shared, "Releases", "no link on a share link");
    }

    #[test]
    fn breadcrumbs_follow_the_section_and_the_instance_pages() {
        let p = ProjectInfo {
            id: "d",
            name: "Demo",
            key: "k",
        };
        let c = Crumbs::of("Heatmap", Some(&p)).render().into_string();
        assert!(c.contains("aria-label=\"Breadcrumb\""));
        assert!(c.contains("href=\"/projects\""));
        assert!(c.contains("href=\"/p/d\""));
        assert!(c.contains("href=\"/p/d/events\">Behaviour"));
        assert!(c.contains("aria-current=\"page\">Pages"));
        let i = Crumbs::of("Storage and backups", None)
            .render()
            .into_string();
        assert!(i.contains("Instance") && i.contains("aria-current=\"page\">Storage"));
        assert!(
            Crumbs::of("Demo", Some(&p))
                .render()
                .into_string()
                .contains("aria-current=\"page\">Demo")
        );
        assert!(
            instance_tabs("AI")
                .into_string()
                .contains("href=\"/settings/ai\" aria-current=\"page\"")
        );
        assert!(instance_tabs("Events").into_string().is_empty());
    }

    #[test]
    fn the_sidebar_has_distinct_icons_and_every_page_keeps_its_url() {
        let icons: std::collections::HashSet<_> = NAV.iter().map(|g| g.icon).collect();
        assert_eq!(icons.len(), NAV.len(), "one distinct icon per entry");
        assert!(NAV.len() <= 8);
        let paths: Vec<_> = NAV
            .iter()
            .flat_map(|g| g.tabs.iter().map(|t| t.0))
            .collect();
        for old in [
            "",
            "/ask",
            "/live",
            "/uptime",
            "/events",
            "/pages",
            "/errors",
            "/replays",
            "/flags",
            "/surveys",
            "/audience",
            "/acquisition",
            "/forms",
            "/churn",
            "/goals",
            "/tickets",
            "/timeline",
            "/flows",
            "/features",
            "/digest",
            "/settings",
        ] {
            assert!(paths.contains(&old), "{old} is reachable");
        }
    }

    #[test]
    fn a_page_shows_its_section_tabs_with_the_current_one_marked() {
        let p = ProjectInfo {
            id: "default",
            name: "Demo",
            key: "k",
        };
        let html = section_tabs("Heatmap", &p).into_string();
        assert!(html.contains("aria-label=\"Behaviour\""));
        assert!(html.contains("href=\"/p/default/pages\" aria-current=\"page\""));
        assert!(html.contains("href=\"/p/default/forms\""));
        assert!(
            section_tabs("Ask", &p).into_string().is_empty(),
            "one page, no tabs"
        );
        assert!(
            section_tabs("Demo", &p).into_string().is_empty(),
            "the board"
        );
        assert_eq!(
            group_of("Demo", "Demo").map(|g| g.label),
            Some("Focus board")
        );
    }

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

//! The shell (#540): the sidebar (brand, project switcher, Go to…, the
//! nine sections, the user menu), the sticky top bar with the breadcrumb
//! and a place for the page's context, the section tabs under it, and the
//! signals line. No script: the user menu is a native `popover`, the
//! sections have `accesskey` 1–9 (v2.1 order, #621).
//!
//! Moved out of `lib.rs` (the file-length check) when the shell changed.

use maud::DOCTYPE;

use super::*;

/// One sidebar entry (#402): a section whose pages are tabs. `tabs` are
/// (path under `/p/<id>`, tab label, the page titles that belong to it).
pub(crate) struct NavGroup {
    pub(crate) label: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) tabs: &'static [(&'static str, &'static str, &'static [&'static str])],
}

/// The project sidebar (#402, v2.1 #621): nine entries in the method's
/// order, Releases first among the data sections (the "compare" step).
/// Every page keeps its URL; a section opens its first tab. Pages that do
/// not exist yet get no tab (Retention in People), and
/// Delivery stays a card on the Releases page until it has its own page.
pub(crate) const NAV: &[NavGroup] = &[
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
        label: "Releases",
        icon: "release",
        tabs: &[
            ("/releases", "Releases", &["Releases"]),
            ("/timeline", "Timeline", &["Timeline"]),
        ],
    },
    NavGroup {
        label: "Live",
        icon: "pulse",
        tabs: &[
            ("/live", "Live", &["Live"]),
            ("/uptime", "Uptime", &["Uptime"]),
        ],
    },
    NavGroup {
        label: "Quality",
        icon: "alert",
        tabs: &[
            ("/errors", "Errors", &["Errors"]),
            ("/routes", "Routes", &["Routes"]),
            ("/replays", "Replays", &["Replays", "Replay"]),
        ],
    },
    NavGroup {
        label: "Behaviour",
        icon: "events",
        tabs: &[
            ("/events", "Events", &["Events"]),
            ("/pages", "Pages", &["Pages"]),
            // The most viewed page's heatmap; Pages links every other one (#592).
            ("/heatmap", "Heatmap", &["Heatmap"]),
            ("/forms", "Forms", &["Forms"]),
            ("/flows", "Feature flow", &["Feature flow"]),
        ],
    },
    NavGroup {
        label: "People",
        icon: "globe",
        tabs: &[
            ("/audience", "Audience", &["Audience", "Person"]),
            ("/churn", "Churn", &["Churn"]),
            ("/acquisition", "Acquisition", &["Acquisition"]),
            ("/revenue", "Revenue", &["Revenue"]),
        ],
    },
    NavGroup {
        label: "Product",
        icon: "layers",
        tabs: &[
            ("/features", "Feature map", &["Feature map"]),
            ("/goals", "Goals", &["Goals"]),
            ("/flags", "Flags", &["Flags"]),
            ("/surveys", "Surveys", &["Surveys"]),
            ("/tickets", "Tickets", &["Tickets", "New ticket"]),
            ("/digest", "Digest", &["Weekly digest"]),
        ],
    },
    NavGroup {
        label: "Settings",
        icon: "gear",
        // One scope per project (#438): its tabs, by what they hold.
        tabs: &[
            ("/settings", "General", &["Settings"]),
            ("/settings/install", "Install", &["Install"]),
            ("/settings/alerts", "Alerts", &["Alert settings"]),
            ("/settings/sharing", "Sharing", &["Share links"]),
            ("/settings/environments", "Environments", &["Environments"]),
            ("/settings/data", "Data", &["Project data"]),
        ],
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

/// A tab group outside projects: path, label, titles.
type Tabs = &'static [(&'static str, &'static str, &'static [&'static str])];

/// The whole install's settings, for admins (#403, #438).
const INSTANCE_TABS: Tabs = &[
    ("/users", "Team", &["Team"]),
    ("/settings/tokens", "API tokens", &["API tokens"]),
    ("/settings/ai", "AI", &["AI"]),
    ("/settings/storage", "Storage", &["Storage and backups"]),
    ("/settings/about", "About", &["About"]),
];

/// The signed-in person's own settings (#438).
const PROFILE_TABS: Tabs = &[
    ("/me", "Profile", &["My profile"]),
    ("/me/sign-in", "Sign-in", &["Sign-in"]),
    ("/me/appearance", "Appearance", &["Appearance"]),
];

/// The scope a page outside projects belongs to: its name, where it
/// starts, and its tabs.
fn scope_of(title: &str) -> Option<(&'static str, &'static str, Tabs)> {
    [
        ("Instance settings", "/settings", INSTANCE_TABS),
        ("My profile", "/me", PROFILE_TABS),
    ]
    .into_iter()
    .find(|(_, _, tabs)| tabs.iter().any(|t| t.2.contains(&title)))
}

fn instance_tabs(title: &str) -> Markup {
    let Some((name, _, tabs)) = scope_of(title) else {
        return html! {};
    };
    let items: Vec<(String, &str, bool)> = tabs
        .iter()
        .map(|(href, label, titles)| ((*href).to_owned(), *label, titles.contains(&title)))
        .collect();
    components::tabs(name, &items)
}

static ADMIN: std::sync::OnceLock<fn() -> bool> = std::sync::OnceLock::new();

/// Where pages learn whether the reader is an admin (the sidebar shows
/// the instance settings to admins only). Set once.
pub fn set_admin_source(source: fn() -> bool) {
    let _ = ADMIN.set(source);
}

pub(crate) fn viewer_is_admin() -> bool {
    ADMIN.get().is_some_and(|f| f())
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
    /// <tab>`, or `<scope> › <tab>` (Instance settings, My profile).
    pub(crate) fn of(title: &str, project: Option<&ProjectInfo<'_>>) -> Self {
        let mut c = Vec::new();
        if let Some((name, home, tabs)) = scope_of(title)
            && let Some(t) = tabs.iter().find(|t| t.2.contains(&title))
        {
            c.push((name.to_owned(), Some(home.to_owned())));
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

static ICON: std::sync::OnceLock<fn(&str) -> bool> = std::sync::OnceLock::new();

/// Where pages learn whether a project has an icon of its own (the server
/// keeps the fetched favicons on disk). Set once. Without it, or when it
/// says no, the switcher shows the project's initial (#540): no image that
/// may not load.
pub fn set_icon_source(source: fn(&str) -> bool) {
    let _ = ICON.set(source);
}

fn has_icon(id: &str) -> bool {
    ICON.get().is_some_and(|f| f(id))
}

/// The first letter or digit of a name, upper-cased (`?` when it has none).
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// The project's mark in the switcher: its icon when the server has one,
/// else its monogram.
fn project_mark(p: &ProjectInfo<'_>) -> Markup {
    html! {
        @if has_icon(p.id) {
            img.picon src={ "/p/" (p.id) "/icon" } alt="" width="24" height="24";
        } @else {
            span.monogram aria-hidden="true" { (initial(p.name)) }
        }
    }
}

/// One pill of the signals line (#540): something that needs attention,
/// with where to act on it.
pub(crate) struct Signal {
    /// `bad`, `warn` or `todo`.
    pub(crate) tone: &'static str,
    /// What it says.
    pub(crate) text: String,
    /// Steps done of the total: a small meter instead of the dot.
    pub(crate) progress: Option<(usize, usize)>,
    /// Where to act.
    pub(crate) href: String,
    /// The link's text.
    pub(crate) link: &'static str,
}

/// Width of the signal meter, in SVG units.
const METER: usize = 44;

/// The signals line: the page's alerts and pending setup as compact pills
/// in one status region. Nothing when there is nothing to say. A meter's
/// width is an SVG attribute (the CSP forbids `style`).
pub(crate) fn signals(items: &[Signal]) -> Markup {
    if items.is_empty() {
        return html! {};
    }
    html! {
        div.signals role="status" {
            @for s in items {
                span class={ "signal " (s.tone) } {
                    @if let Some((done, total)) = s.progress {
                        svg.meter viewBox={ "0 0 " (METER) " 4" } width=(METER) height="4" aria-hidden="true" focusable="false" {
                            rect.track width=(METER) height="4" rx="2" {}
                            rect.done width=(METER * done.min(total) / total.max(1)) height="4" rx="2" {}
                        }
                    } @else {
                        span.dot aria-hidden="true" {}
                    }
                    span { (s.text) }
                    a href=(s.href) { (s.link) }
                }
            }
        }
    }
}

/// The user menu (#540): one button opening a popover with the instance
/// settings (admins), the profile, the appearance, folding the menu and
/// logging out. Pages outside a project keep only the folding links.
fn user_menu(title: &str, signed: bool) -> Markup {
    let scope = scope_of(title).map(|s| s.1);
    let fold = html! {
        a.collapse href="/nav/collapsed" title="Collapse the menu" { (icon("left")) span.label { "Collapse the menu" } }
        a.expand href="/nav/expanded" title="Expand the menu" aria-label="Expand the menu" { (icon("right")) span.label { "Expand the menu" } }
    };
    html! {
        @if signed {
            button.me type="button" popovertarget="me-menu" title="Account" {
                span.avatar aria-hidden="true" { (icon("users")) }
                span.label { "Account" small { "Settings, profile, log out" } }
            }
            div.me-menu #me-menu popover aria-label="Account" {
                @if viewer_is_admin() {
                    a href="/settings" aria-current=[(scope == Some("/settings")).then_some("page")] { (icon("gear")) "Instance settings" }
                }
                a href="/me" aria-current=[(scope == Some("/me") && title != "Appearance").then_some("page")] { (icon("users")) "My profile" }
                a href="/me/appearance" aria-current=[(title == "Appearance").then_some("page")] { (icon("auto")) "Appearance" }
                (fold)
                form method="post" action="/logout" {
                    button type="submit" { (icon("logout")) "Log out" }
                }
            }
        } @else {
            nav.me-menu.inline aria-label="Menu" { (fold) }
        }
    }
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
                link rel="stylesheet" href=(crate::css_href());
                link rel="icon" type="image/svg+xml" href="/assets/favicon.svg";
            }
            body.shared {
                (icons::sprite())
                div.content {
                    header.shared-banner {
                        span.blob aria-hidden="true" {}
                        strong { "OpenLens" }
                        @if let Some(p) = project { span.muted { (p.name) } }
                        span.pill { "Read-only view" }
                    }
                    main { (body) }
                }
            }
        }
    }
}

/// A page in the shell.
pub(crate) fn layout(title: &str, project: Option<&ProjectInfo<'_>>, body: Markup) -> Markup {
    layout_in(title, project, html! {}, body)
}

/// [`layout`] with the page's context controls (the period, environment,
/// role and release chips, #542) in the top bar, after the breadcrumb.
pub(crate) fn layout_in(
    title: &str,
    project: Option<&ProjectInfo<'_>>,
    context: Markup,
    body: Markup,
) -> Markup {
    if shared() {
        return shared_layout(title, project, body);
    }
    // Instance pages keep the last project's menu (#403).
    let remembered = if project.is_none() && scope_of(title).is_some() {
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
    page(title, project, &crumbs, context, body)
}

/// [`layout`] with an explicit breadcrumb (#403; the feature pages, #411,
/// pass their own).
pub(crate) fn layout_with(
    title: &str,
    project: Option<&ProjectInfo<'_>>,
    crumbs: &Crumbs,
    body: Markup,
) -> Markup {
    page(title, project, crumbs, html! {}, body)
}

fn page(
    title: &str,
    project: Option<&ProjectInfo<'_>>,
    crumbs: &Crumbs,
    context: Markup,
    body: Markup,
) -> Markup {
    let prefs = prefs();
    let trail = crumbs.render();
    let tabs = html! {
        @if let Some(p) = project { (section_tabs(title, p)) }
        (instance_tabs(title))
    };
    let current = project
        .and_then(|p| group_of(title, p.name))
        .map(|g| g.label);
    html! {
        (DOCTYPE)
        html lang="en" data-theme=[prefs.theme] data-nav=[prefs.nav_collapsed.then_some("collapsed")] {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) " · OpenLens" }
                link rel="stylesheet" href=(crate::css_href());
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
                                (project_mark(p))
                                span.label { (p.name) small { "Switch project" } }
                            }
                            (goto::goto_form(p))
                            nav.side-nav aria-label="Project" {
                                @for (i, g) in NAV.iter().enumerate() {
                                    a href={ "/p/" (p.id) (g.tabs[0].0) } title=(g.label) accesskey=(i + 1) aria-current=[(current == Some(g.label)).then_some("page")] {
                                        (icon(g.icon)) span.label { (g.label) } kbd aria-hidden="true" { (i + 1) }
                                    }
                                }
                            }
                        }
                        div.side-foot { (user_menu(title, project.is_some())) }
                    }
                    div.content {
                        @if !trail.0.is_empty() || !context.0.is_empty() {
                            header.topbar {
                                div.bar {
                                    (trail)
                                    (context)
                                }
                            }
                        }
                        @if !tabs.0.is_empty() { div.subnav { (tabs) } }
                        main { (body) }
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

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
        assert!(c.contains("aria-current=\"page\">Heatmap"));
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
        // #438: the profile is its own scope, not an instance tab.
        let me = Crumbs::of("Appearance", None).render().into_string();
        assert!(
            me.contains("href=\"/me\">My profile")
                && me.contains("aria-current=\"page\">Appearance")
        );
        let tabs = instance_tabs("Appearance").into_string();
        assert!(tabs.contains("/me/appearance") && !tabs.contains("/settings/ai"));
        // Every settings page is in exactly one scope.
        let project: Vec<&str> = NAV
            .iter()
            .filter(|g| g.label == "Settings")
            .flat_map(|g| g.tabs.iter().flat_map(|t| t.2.iter().copied()))
            .collect();
        let all: Vec<&str> = project
            .iter()
            .copied()
            .chain(
                INSTANCE_TABS
                    .iter()
                    .chain(PROFILE_TABS)
                    .flat_map(|t| t.2.iter().copied()),
            )
            .collect();
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
        assert!(project.len() > 1);
    }

    #[test]
    fn the_sidebar_has_distinct_icons_and_every_page_keeps_its_url() {
        let icons: std::collections::HashSet<_> = NAV.iter().map(|g| g.icon).collect();
        assert_eq!(icons.len(), NAV.len(), "one distinct icon per entry");
        // One digit per access key (#621: nine entries, 1–9).
        assert!(NAV.len() <= 9);
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
            "/releases",
            "/revenue",
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
        assert!(html.contains("href=\"/p/default/heatmap\" aria-current=\"page\""));
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

    fn demo() -> ProjectInfo<'static> {
        ProjectInfo {
            id: "d",
            name: "demo",
            key: "k",
        }
    }

    #[test]
    fn a_project_without_an_icon_gets_its_monogram_not_an_image() {
        for (name, letter) in [("demo", "D"), ("  zebra", "Z"), ("9 lives", "9")] {
            let p = ProjectInfo {
                id: "d",
                name,
                key: "k",
            };
            let html = layout("Events", Some(&p), html! {}).into_string();
            assert!(
                html.contains(&format!(
                    "class=\"monogram\" aria-hidden=\"true\">{letter}<"
                )),
                "{name}"
            );
            assert!(
                !html.contains("picon") && !html.contains("/d/icon"),
                "{name}"
            );
        }
    }

    #[test]
    fn every_section_has_its_access_key_and_the_current_one_is_marked() {
        let html = layout("Events", Some(&demo()), html! {}).into_string();
        assert_eq!(html.matches("accesskey=").count(), NAV.len());
        for k in 1..=NAV.len() {
            assert!(html.contains(&format!("accesskey=\"{k}\"")), "{k}");
        }
        let behaviour = NAV.iter().position(|g| g.label == "Behaviour").unwrap() + 1;
        assert!(html.contains(&format!("accesskey=\"{behaviour}\" aria-current=\"page\"")));
    }

    #[test]
    fn moved_pages_have_their_new_section_in_the_trail_the_tabs_and_the_sidebar() {
        let p = ProjectInfo {
            id: "d",
            name: "Demo",
            key: "k",
        };
        // #621: Timeline under Releases, Goals under Product, Revenue under People.
        for (title, section, first, path) in [
            ("Timeline", "Releases", "/releases", "/timeline"),
            ("Releases", "Releases", "/releases", "/releases"),
            ("Goals", "Product", "/features", "/goals"),
            ("Revenue", "People", "/audience", "/revenue"),
        ] {
            let c = Crumbs::of(title, Some(&p)).render().into_string();
            assert!(
                c.contains(&format!("href=\"/p/d{first}\">{section}<")),
                "{title}: {c}"
            );
            assert!(
                c.contains(&format!("aria-current=\"page\">{title}<")),
                "{title}"
            );
            let tabs = section_tabs(title, &p).into_string();
            assert!(
                tabs.contains(&format!("aria-label=\"{section}\"")),
                "{title}"
            );
            assert!(
                tabs.contains(&format!("href=\"/p/d{path}\" aria-current=\"page\"")),
                "{title}: {tabs}"
            );
            let html = layout(title, Some(&p), html! {}).into_string();
            let key = NAV.iter().position(|g| g.label == section).unwrap() + 1;
            assert!(
                html.contains(&format!("accesskey=\"{key}\" aria-current=\"page\"")),
                "{title}"
            );
            assert_eq!(html.matches("aria-current=\"page\"").count(), 3, "{title}");
        }
        let live = section_tabs("Live", &p).into_string();
        assert!(!live.contains("/timeline") && !live.contains("/releases"));
        assert!(!section_tabs("Churn", &p).into_string().contains("/goals"));
    }

    #[test]
    fn the_user_menu_is_a_popover_that_holds_the_logout_form() {
        let html = layout("Events", Some(&demo()), html! {}).into_string();
        let button = html.find("popovertarget=\"me-menu\"").unwrap();
        let menu = html.find("id=\"me-menu\"").unwrap();
        assert!(button < menu);
        let open = html[..menu].rfind('<').unwrap();
        let tag = &html[open..menu + html[menu..].find('>').unwrap()];
        assert!(tag.contains(" popover"), "{tag}");
        let inside = &html[menu..menu + html[menu..].find("</div>").unwrap()];
        for part in [
            "action=\"/logout\"",
            "href=\"/me\"",
            "/me/appearance",
            "/nav/",
        ] {
            assert!(inside.contains(part), "{part}");
        }
        assert_eq!(html.matches("action=\"/logout\"").count(), 1, "one way out");
        // Outside a project, no menu to open, but the fold links stay.
        let out = crate::message("Not found", "Nothing here.").into_string();
        assert!(!out.contains("popovertarget") && out.contains("/nav/collapsed"));
    }

    #[test]
    fn the_setup_meter_grows_with_the_steps_done() {
        let width = |done: usize| {
            let s = signals(&[Signal {
                tone: "todo",
                text: "Setup".into(),
                progress: Some((done, 3)),
                href: "/x".into(),
                link: "Finish",
            }])
            .into_string();
            let at = s.find("class=\"done\" width=\"").unwrap() + "class=\"done\" width=\"".len();
            s[at..].split('"').next().unwrap().parse::<usize>().unwrap()
        };
        assert!(width(0) < width(1) && width(1) < width(3));
        assert_eq!(width(9), width(3), "never wider than the track");
        assert!(signals(&[]).into_string().is_empty());
    }

    #[test]
    fn pages_carry_no_inline_style_and_no_script() {
        let b = Board {
            total_events: 10,
            app_down: Some("No traffic since 15:54".into()),
            objectives_at_risk: vec!["Activation".into()],
            setup: Some((1, 3)),
            ..Board::default()
        };
        let board = crate::board(&demo(), &b, &Filters::default(), "http://x", &[], None, &[])
            .into_string();
        // The alerts and the setup are one status line of pills.
        assert_eq!(
            board.matches("class=\"signals\" role=\"status\"").count(),
            1
        );
        assert_eq!(board.matches("class=\"signal ").count(), 3);
        assert!(board.contains("class=\"topbar\"") && board.contains("aria-label=\"Breadcrumb\""));
        let pages = [
            board,
            layout("Heatmap", Some(&demo()), html! { p { "x" } }).into_string(),
            crate::message("Not found", "Nothing here.").into_string(),
            crate::login(true, true, true, true).into_string(),
        ];
        for html in pages {
            assert!(!html.contains("style="), "inline style");
            assert!(!html.contains("<script"), "script");
        }
    }
}

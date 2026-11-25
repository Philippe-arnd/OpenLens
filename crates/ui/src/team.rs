//! Projects, heatmaps, the digest, the team, my profile, project settings and surveys.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

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
                @if admin { a.button href="/users" { (icon("users")) "Team" } " " a.button href="/settings/storage" { (icon("layers")) "Storage and backups" } " " a.button href="/settings/tokens" { (icon("code")) "API tokens" } " " a.button href="/settings/ai" { (icon("spark")) "AI" } " " a.button href="/audit" { (icon("shield")) "Audit log" } }
            }
            @if admin {
                details.card.wide.new-project {
                    summary { (icon("plus")) "New project" }
                    form.invite method="post" action="/projects" {
                        label { "Name " input type="text" name="name" required maxlength="80" placeholder="My app"; }
                        label { "Time zone " input type="text" name="timezone" placeholder="Europe/Paris" maxlength="64" list="tz-all"; }
                        (zone_list())
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
                    thead { tr { th { span.sr-only { "Metric" } } (th_num("This week")) (th_num("Last week")) (th_num("Change")) } }
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
            @if !d.goals.is_empty() {
                section.card.wide {
                    h2.sun { (icon("star")) "Goals" }
                    ul.plain-list {
                        @for o in &d.objectives_at_risk { li { span.status.bad { "objective at risk" } " " (o) } }
                        @for (line, st) in &d.goals { li { (line) " — " strong { (st) } } }
                    }
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
                    p { "Send this one-time link (valid 7 days, 24 hours after a password reset):" }
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
                    @if !u.disabled && status != "invited" {
                        form.inline method="post" action={ (base) "/reset-password" } { button.ghost type="submit" { "Reset the password" } }
                    }
                    @if status == "invited" {
                        form.inline method="post" action={ (base) "/revoke" } { button.ghost type="submit" { "Revoke the invitation" } }
                    }
                    @if u.disabled {
                        form.inline method="post" action={ (base) "/enable" } { button type="submit" { "Enable sign-in" } }
                    } @else {
                        form.inline method="post" action={ (base) "/disable" } { button.ghost type="submit" { "Disable sign-in" } }
                    }
                }
                p.muted.small { "Disabling ends their sessions at once and keeps the account; enabling lets them sign in again. Resetting the password ends their sessions and gives you a one-time link (24 h) for them to set a new one; no password is ever shown." }
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

/// The password card of *My profile* (#428).
pub struct MePassword {
    /// Password sign-in is on.
    pub enabled: bool,
    /// Signed in with the admin token (no account yet).
    pub owner: bool,
    /// This account has a password.
    pub has_password: bool,
    /// The notice is an error.
    pub error: bool,
    /// The second factor (#440).
    pub totp: crate::TotpCard,
}

/// The tabs of *My profile* (#438).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeTab {
    /// Name and time zone.
    #[default]
    Profile,
    /// Password.
    SignIn,
    /// Theme and sidebar.
    Appearance,
}

impl MeTab {
    fn title(self) -> &'static str {
        match self {
            Self::Profile => "My profile",
            Self::SignIn => "Sign-in",
            Self::Appearance => "Appearance",
        }
    }
}

/// How the reader wants pages to look (#438).
#[derive(Debug, Clone, Copy, Default)]
pub struct MeLook<'a> {
    /// `light`, `dark`, or `None` to follow the system.
    pub theme: Option<&'a str>,
    /// The sidebar starts folded.
    pub nav_folded: bool,
    /// Saved for the account (across browsers), not only this browser.
    pub per_account: bool,
}

/// My profile: the time zone I read times in, my name (#215, #216), my
/// password (#428) and how pages look (#438), one tab each.
pub fn me(
    tab: MeTab,
    name: Option<&str>,
    email: Option<&str>,
    tz: Option<&str>,
    notice: Option<&str>,
    pw: &MePassword,
    look: &MeLook<'_>,
) -> Markup {
    layout(
        tab.title(),
        None,
        html! {
            div.heading { div {
                h1 { "My profile" }
                p.muted { @if let Some(e) = email { (e) } @else { "Owner (admin token)" } }
            } }
            @if let Some(n) = notice {
                @if pw.error { p.alert role="alert" { (n) } } @else { p.notice role="status" { (n) } }
            }
            @if tab == MeTab::SignIn && !pw.enabled {
                section.card { p.muted { "Password sign-in is turned off on this server: you sign in with a link or your identity provider." } }
            }
            @if tab == MeTab::SignIn && pw.enabled {
                section.card.wide #password {
                    @if pw.owner {
                        h2.violet { (icon("shield")) "Create your admin account" }
                        p.small { "You signed in with the admin token. Create an account with your email and a password to sign in without it. The token keeps working as the way back in (a lost password, a locked account)." }
                        form.settings method="post" action="/me/password" {
                            label { "Email " input type="email" name="email" autocomplete="username" required maxlength="200"; }
                            label { "Password " input type="password" name="password" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            label { "Again " input type="password" name="again" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            button type="submit" { "Create my account" }
                        }
                    } @else {
                        h2.violet { (icon("shield")) (if pw.has_password { "Change password" } else { "Set a password" }) }
                        form.settings method="post" action="/me/password" {
                            input type="email" name="email" value=[email] autocomplete="username" hidden;
                            @if pw.has_password {
                                label { "Current password " input type="password" name="current" autocomplete="current-password" required maxlength="200"; }
                            }
                            label { "New password " input type="password" name="password" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            label { "Again " input type="password" name="again" autocomplete="new-password" required minlength="12" maxlength="200"; }
                            button type="submit" { "Save the password" }
                        }
                        p.muted.small { "At least 12 characters; a few words work well. Saving it signs out your other sessions." }
                    }
                }
                (crate::totp::totp_card(&pw.totp))
            }
            @if tab == MeTab::Appearance {
                section.card.wide {
                    h2.violet { (icon("sun")) "Appearance" }
                    form.settings method="post" action="/me/appearance" {
                        fieldset {
                            legend { "Theme" }
                            label.check { input type="radio" name="theme" value="auto" checked[look.theme.is_none()]; " Follow the system" }
                            label.check { input type="radio" name="theme" value="light" checked[look.theme == Some("light")]; " Light" }
                            label.check { input type="radio" name="theme" value="dark" checked[look.theme == Some("dark")]; " Dark" }
                        }
                        label.check { input type="checkbox" name="nav_folded" value="on" checked[look.nav_folded]; " Start with the menu folded (icons only)" }
                        button type="submit" { "Save" }
                    }
                    p.muted.small {
                        @if look.per_account { "Saved with your account: it follows you to every browser you sign in from." }
                        @else { "Saved in this browser (the admin token has no account)." }
                    }
                }
            }
            @if tab == MeTab::Profile {
            section.card.wide {
                form.settings method="post" action="/me" {
                    @if email.is_some() {
                        label { "Name " input type="text" name="name" value=[name] maxlength="80"; }
                    }
                    label { "Time zone " input type="text" name="timezone" value=[tz] placeholder="the project's" maxlength="64" list="tz-all"; }
                    (zone_list())
                    button type="submit" { "Save" }
                }
                p.muted.small { "Every time and every day on the boards follows this zone (an IANA name like Europe/Paris). Empty: each project's own zone." }
            }
            }
        },
    )
}

/// Instance settings → About (#438): the version, whether a newer one is
/// out, and where the docs are.
pub fn about() -> Markup {
    let update = crate::update_available();
    layout(
        "About",
        None,
        html! {
            div.heading { div {
                h1 { "Instance settings" }
                p.muted { "What runs here, for the whole install." }
            } }
            section.card.wide {
                h2.violet { (icon("idea")) "About" }
                dl.facts {
                    dt { "Version" } dd { "OpenLens " (env!("CARGO_PKG_VERSION")) }
                    dt { "Updates" } dd {
                        @match &update {
                            Some(u) => (crate::update_notice(u)),
                            None => "Up to date, as far as the last daily check knows.",
                        }
                    }
                }
                p.small {
                    a href="/agents.md" { "Wiring guide for agents" } " · "
                    a href="https://github.com/Philippe-arnd/OpenLens/tree/main/docs" { "Documentation" } " · "
                    a href="https://github.com/Philippe-arnd/OpenLens/blob/main/SECURITY.md" { "Security policy" }
                }
            }
        },
    )
}

/// The tabs of a project's settings (#438).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    /// Name, app address, time zone, setup progress.
    #[default]
    General,
    /// Snippets, capture key, wiring check.
    Install,
    /// Alerts.
    Alerts,
    /// Share links.
    Sharing,
    /// Explore, export, delete.
    Data,
}

impl SettingsTab {
    /// The tab of `/p/<id>/settings/<slug>`.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Some(match slug {
            "install" => Self::Install,
            "alerts" => Self::Alerts,
            "sharing" => Self::Sharing,
            "data" => Self::Data,
            _ => return None,
        })
    }

    /// The page title, as the sidebar's tabs name it.
    fn title(self) -> &'static str {
        match self {
            Self::General => "Settings",
            Self::Install => "Install",
            Self::Alerts => "Alert settings",
            Self::Sharing => "Share links",
            Self::Data => "Project data",
        }
    }
}

/// What the settings page shows of a project (#219).
pub struct SettingsView<'a> {
    /// The tab shown (#438).
    pub tab: SettingsTab,
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
    /// Setup steps (#403).
    pub setup: Vec<SetupStep>,
    /// The latest releases, suggested for a share link (#403).
    pub releases: &'a [String],
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
    /// Certificate expiry.
    pub cert: bool,
    /// Failing-requests spike.
    pub failing: bool,
    /// Quiet hours (start, end).
    pub quiet: Option<(u8, u8)>,
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
    let tab = v.tab;
    layout(
        tab.title(),
        Some(p),
        html! {
            div.heading { div {
                h1 { "Project settings" }
                p.muted { "Everything agents do with " code { "openlens-cli" } " for this project, here too." }
            } }
            @if let Some(n) = v.notice { p.notice role="status" { (n) } }
            @if tab == SettingsTab::General {
            @if !v.setup.is_empty() { (setup_progress(&v.setup)) }
            section.card.wide {
                h2.violet { (icon("gear")) "Project" }
                @if v.admin {
                    form.settings method="post" action=(base) {
                        label { "Name " input type="text" name="name" value=(p.name) required maxlength="80"; }
                        label { "App address "
                            input type="url" name="app_url" value=[v.app_url] placeholder=(v.detected_url.unwrap_or("https://app.example.com")) maxlength="200";
                        }
                        label { "Time zone " input type="text" name="timezone" value=(v.timezone) required maxlength="64" list="tz-all"; }
                        (zone_list())
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
            }
            @if tab == SettingsTab::Install {
            section.card.wide #install {
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
            }
            @if tab == SettingsTab::Alerts {
            section.card.wide #alerts {
                h2.coral { (icon("bolt")) "Alerts" }
                p.small { "Know when the app is down: OpenLens checks the app address every minute (" a href={ "/p/" (p.id) "/uptime" } { "Uptime" } "), notices when the traffic stops for much longer than usual (after four weeks of history), and, every hour, when the board finds a regression (a release or flag with more errors, pages turning slow) or a new error hits several users. One message when an incident starts, one when it ends." }
                @if v.admin {
                    form.settings method="post" action={ (base) "/alerts" } {
                        label.check { input type="checkbox" name="down" checked[v.alerts.down]; " App down (needs the app address)" }
                        label { "Announce after (min) " input type="number" name="down_wait" min="1" max="60" value=(v.alerts.down_wait); }
                        label { "Health path " input type="text" name="health_path" value=(v.alerts.health_path) maxlength="200" list="health-paths"; }
                        datalist id="health-paths" { option value="/" {} option value="/healthz" {} option value="/readyz" {} option value="/health" {} }
                        label.check { input type="checkbox" name="traffic" checked[v.alerts.traffic]; " Traffic stopped" }
                        label.check { input type="checkbox" name="regressions" checked[v.alerts.regressions]; " Regressions and new errors" }
                        label.check { input type="checkbox" name="failing" checked[v.alerts.failing]; " Failing requests spike" }
                        label.check { input type="checkbox" name="cert" checked[v.alerts.cert]; " Certificate expiring (14 days)" }
                        label { "Quiet from " (hour_select("quiet_start", v.alerts.quiet.map(|q| q.0))) }
                        label { "to " (hour_select("quiet_end", v.alerts.quiet.map(|q| q.1))) }
                        label.check { input type="checkbox" name="email" checked[v.alerts.email]; " Email" @if !v.alerts.smtp { span.muted { " (SMTP not set)" } } }
                        label { "Webhook (Slack, Discord…) " input type="url" name="webhook" value=(v.alerts.webhook) placeholder="https://hooks.slack.com/…" maxlength="500"; }
                        label { "ntfy topic " input type="text" name="ntfy" value=(v.alerts.ntfy) placeholder="my-app-alerts" maxlength="300"; }
                        button type="submit" { "Save" }
                    }
                    form.inline method="post" action={ (base) "/alerts/test" } { button.soft type="submit" { "Send a test alert" } }
                    p.muted.small { "Emails go to " code { "OPENLENS_ALERT_TO" } " (or the digest list, or the admins). ntfy: a topic on ntfy.sh, or a full https:// topic URL of your own server; install the ntfy app to get pushes on a phone. Quiet hours (the project's time zone) hold non-urgent alerts until they end; " em { "down" } " and an expired certificate always go out. An incident that lasts is repeated at most once an hour." }
                }
            }
            }
            @if tab == SettingsTab::Sharing {
            @if !v.admin { section.card { p.muted { "Admins make and revoke share links." } } }
            @if v.admin {
                section.card.wide #share {
                    h2.sky { (icon("route")) "Share links" }
                    p.small { "A read-only link to the Focus board or the weekly digest, for people without an account: no SQL, people, replays or settings behind it. Filters are fixed when the link is made." }
                    form.settings method="post" action={ (base) "/share" } {
                        label { "What " select name="scope" { option value="board" { "Focus board" } option value="digest" { "Weekly digest" } } }
                        label { "Release " input type="text" name="release" placeholder="all" maxlength="100" list="share-releases"; }
                        datalist id="share-releases" { @for r in v.releases { option value=(r) {} } }
                        label { "Expires after (days, 0 = never) " input type="number" name="days" min="0" max="366" value="30"; }
                        label { "May be embedded on " input type="text" name="embed" placeholder="https://intranet.example.com" maxlength="500" list="embed-on"; }
                        datalist id="embed-on" { @if let Some(u) = v.app_url.or(v.detected_url) { option value=(u) {} } }
                        button type="submit" { "Make a link" }
                    }
                    @if !v.shares.is_empty() {
                        div.table-scroll { table.plain {
                            thead { tr { th { "What" } th { "Release" } th { "Embed" } th { "Expires" } th { "By" } th { span.sr-only { "Actions" } } } }
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
            }
            }
            @if tab == SettingsTab::Data {
            @if v.admin {
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

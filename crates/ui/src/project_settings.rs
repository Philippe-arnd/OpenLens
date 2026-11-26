//! A project's settings: General, Install, Alerts, Sharing, Environments,
//! Data (#219, #438).
//!
//! Moved out of `team.rs` as is (#546).

use super::*;

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
    /// Hosts per environment and the host rules (#410).
    Environments,
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
            "environments" => Self::Environments,
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
            Self::Environments => "Environments",
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
    /// Hosts and rules, read only for the *Environments* tab (#410).
    pub environments: Option<openlens_engine::environments::Environments>,
    /// The dry run of a re-labelling, or why it was refused (#484).
    pub relabel: Option<Result<openlens_engine::environments::Relabelled, String>>,
    /// The host allow-list and what it dropped (#485).
    pub allow: Option<openlens_engine::environments::AllowReport>,
    /// The preview of a new allow-list, or why it was refused (#485).
    pub allow_preview: Option<Result<openlens_engine::environments::AllowPreview, String>>,
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
            @if tab == SettingsTab::Environments {
            @if let Some(e) = &v.environments {
                (crate::environments::tab(&base, v.admin, v.timezone, e, v.relabel.as_ref(), v.allow.as_ref(), v.allow_preview.as_ref()))
            } @else {
                section.card { p.muted { "The environments could not be read just now." } }
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

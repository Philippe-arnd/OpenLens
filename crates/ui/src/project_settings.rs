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
    /// The public status page's card (#561), on the *Sharing* tab.
    pub status_page: Option<Markup>,
    /// The destructive action to confirm in a dialog (`?confirm=`, #545):
    /// the project's deletion, on the *Data* tab.
    pub confirm: Option<Confirm>,
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
    /// Error budget spent (#560).
    pub budget_spent: bool,
    /// Error budget burning fast (#560).
    pub budget_burn: bool,
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
/// snippets, wiring check, export, and deletion (#219). Every form is a
/// form card (#546): fields in a grid, help under each, Save in the foot.
pub fn project_settings(p: &ProjectInfo<'_>, v: &SettingsView<'_>) -> Markup {
    let base = format!("/p/{}/settings", p.id);
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
            @match tab {
                SettingsTab::General => (general(p, v, &base)),
                SettingsTab::Install => (install(p, v, &base)),
                SettingsTab::Alerts => (alerts(p, v, &base)),
                SettingsTab::Sharing => (sharing(v, &base)),
                SettingsTab::Environments => {
                    @if let Some(e) = &v.environments {
                        (crate::environments::tab(&base, v.admin, v.timezone, e, v.relabel.as_ref(), v.allow.as_ref(), v.allow_preview.as_ref()))
                    } @else {
                        section.card { p.muted { "The environments could not be read just now." } }
                    }
                }
                SettingsTab::Data => (data(p, v, &base)),
            }
        },
    )
}

/// The foot of a form card: its submit button and one line of text.
pub(crate) fn form_foot(button: &str, line: Markup) -> Markup {
    html! {
        div.form-foot {
            button.btn.primary type="submit" { (button) }
            p { (line) }
        }
    }
}

/// The setup of a project (#403, #546) as numbered steps: the required
/// ones with a progress bar, the optional ones folded below. A done step
/// is ticked; a step to do keeps its one action.
pub(crate) fn setup_progress(steps: &[SetupStep]) -> Markup {
    let req: Vec<&SetupStep> = steps.iter().filter(|s| s.required).collect();
    let opt: Vec<&SetupStep> = steps.iter().filter(|s| !s.required).collect();
    let done = req.iter().filter(|s| s.done).count();
    let opt_done = opt.iter().filter(|s| s.done).count();
    // The bar's width is an SVG attribute (the CSP forbids style=), in
    // the units of a 100-wide track, so it can never pass the track.
    let width = if req.is_empty() {
        100.0
    } else {
        (done as f64 * 100.0 / req.len() as f64).clamp(0.0, 100.0)
    };
    let first_todo = req.iter().position(|s| !s.done);
    html! {
        section.card.wide.setup aria-label="Setup" {
            div.setup-head {
                div.section-head {
                    h2 { (if done == req.len() { "Set up" } else { "Finish the setup" }) }
                    span.muted.num { (done) " of " (req.len()) " required · " (opt_done) " of " (opt.len()) " optional" }
                }
                svg.progress role="img" aria-label=(format!("{done} of {} required steps done", req.len())) viewBox="0 0 100 6" preserveAspectRatio="none" {
                    rect.track width="100" height="6" {}
                    rect.bar width=(format!("{width:.1}")) height="6" {}
                }
            }
            div.steps {
                @for (i, st) in req.iter().enumerate() { (setup_step(i + 1, st, first_todo == Some(i))) }
                @if !opt.is_empty() {
                    details {
                        summary {
                            (opt.len()) " optional steps"
                            span { @for (i, st) in opt.iter().enumerate() { @if i > 0 { " · " } (st.label) } }
                        }
                        div.steps {
                            @for (i, st) in opt.iter().enumerate() { (setup_step(req.len() + i + 1, st, false)) }
                        }
                    }
                }
            }
        }
    }
}

/// One setup step: its state (a tick, or its number), its label, and
/// either "Done" or its action.
fn setup_step(n: usize, st: &SetupStep, next: bool) -> Markup {
    html! {
        div class={ "step " (if st.done { "done" } else { "todo" }) } {
            span.state {
                @if st.done { (icon("task")) span.sr-only { "Done:" } } @else { (n) }
            }
            div {
                b { (st.label) }
                @if !st.required { p { "Optional" } }
            }
            @if st.done {
                span.status.good { "Done" }
            } @else {
                a class=(if next { "btn primary sm" } else { "btn sm" }) href=(st.href) { "Set it up" }
            }
        }
    }
}

/// *General*: the setup steps and the project's name, address and zone.
fn general(p: &ProjectInfo<'_>, v: &SettingsView<'_>, base: &str) -> Markup {
    let detected = html! {
        "The app address gives the project its icon (the app's favicon). Empty: "
        @match v.detected_url { Some(u) => { "detected from pageviews, " span.mono { (u) } "." }, None => "detected from pageviews once some arrive." }
    };
    html! {
        @if !v.setup.is_empty() { (setup_progress(&v.setup)) }
        @if v.admin {
            form.card.wide.form-card #project method="post" action=(base) aria-labelledby="project-h" {
                div.section-head { h2 #project-h { (icon("gear")) "Project" } }
                div.form-grid {
                    label.field { "Name" input type="text" name="name" value=(p.name) required maxlength="80"; }
                    label.field { "App address"
                        input type="url" name="app_url" value=[v.app_url] placeholder=(v.detected_url.unwrap_or("https://app.example.com")) maxlength="200";
                        small { (detected) }
                    }
                    label.field { "Time zone"
                        input type="text" name="timezone" value=(v.timezone) required maxlength="64" list="tz-all";
                        small { "Days and weeks of every KPI start here. An IANA name, like Europe/Paris." }
                    }
                }
                (zone_list())
                (form_foot("Save", html! { "Changes apply to every board of this project." }))
            }
        } @else {
            section.card.wide {
                h2.violet { (icon("gear")) "Project" }
                dl.facts {
                    dt { "Name" } dd { (p.name) }
                    dt { "App address" } dd { (v.app_url.or(v.detected_url).unwrap_or("—")) }
                    dt { "Time zone" } dd { (v.timezone) }
                }
                p.muted.small { (detected) " Days and weeks of every KPI start in the time zone (an IANA name like Europe/Paris)." }
            }
        }
    }
}

/// *Install*: one card per method, the wiring check, and the key.
fn install(p: &ProjectInfo<'_>, v: &SettingsView<'_>, base: &str) -> Markup {
    let script = format!(
        r#"<script src="{}/c.js" data-key="{}" defer></script>"#,
        v.host, p.key
    );
    let npm = format!(
        "import {{ init }} from '@openlens-obs/web';\ninit({{ host: '{}', key: '{}' }});",
        v.host, p.key
    );
    let server = format!("OPENLENS_HOST={}\nOPENLENS_KEY={}", v.host, p.key);
    html! {
        section.card.wide #install {
            h2.sky { (icon("code")) "Script tag" }
            p.small { "In every page's " code { "<head>" } ". Capture key " code.mono { (p.key) } " (public: it only sends events)." }
            pre.snippet { code { (script) } }
            p.muted.small { "One click selects the whole snippet. Agents: " a href={ (v.host) "/agents.md" } { "the wiring guide" } "." }
        }
        section.card.wide {
            h2.sky { (icon("code")) "npm" }
            pre.snippet { code { (npm) } }
            p.muted.small { "One click selects the whole snippet." }
        }
        section.card.wide {
            h2.sky { (icon("code")) "Server SDKs (Node, Python, Go, Ruby, PHP)" }
            pre.snippet { code { (server) } }
            p.muted.small { "One click selects the whole snippet." }
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
        @if v.admin {
            form.card.wide.form-card.danger method="post" action={ (base) "/rotate-key" } aria-labelledby="key-h" {
                div.section-head { h2 #key-h { (icon("alert")) "Capture key" } }
                p.small { "Now " code.mono { (p.key) } ". A new key replaces it everywhere at once." }
                label.switch {
                    input type="checkbox" name="confirm" value="yes" required;
                    span { b { "The old key stops working at once" } small { "Every app must get the new one." } }
                }
                div.form-foot {
                    button.btn.danger type="submit" { "Rotate the key" }
                    p { "Events sent with the old key are refused from then on." }
                }
            }
        }
    }
}

/// One alert type: its on/off switch, what it watches, and its fields.
fn alert_row(name: &str, on: bool, label: &str, help: &str, fields: Markup) -> Markup {
    html! {
        li {
            label.switch {
                input type="checkbox" role="switch" name=(name) checked[on];
                span { b { (label) } small { (help) } }
            }
            div.form-grid { (fields) }
        }
    }
}

/// *Alerts* (#256): one row per alert type, quiet hours, the channels.
fn alerts(p: &ProjectInfo<'_>, v: &SettingsView<'_>, base: &str) -> Markup {
    let a = &v.alerts;
    let intro = html! {
        p.small { "Know when the app is down: OpenLens checks the app address every minute (" a href={ "/p/" (p.id) "/uptime" } { "Uptime" } "), notices when the traffic stops for much longer than usual (after four weeks of history), and, every hour, when the board finds a regression (a release or flag with more errors, pages turning slow) or a new error hits several users. One message when an incident starts, one when it ends." }
    };
    html! {
        @if !v.admin {
            section.card.wide #alerts {
                h2.coral { (icon("bolt")) "Alerts" }
                (intro)
            }
        }
        @if v.admin {
            form.card.wide.form-card #alerts method="post" action={ (base) "/alerts" } aria-labelledby="alerts-h" {
                div.section-head { h2 #alerts-h { (icon("bolt")) "Alerts" } }
                (intro)
                ul.alert-rows {
                    (alert_row("down", a.down, "App down", "Checked every minute at the app address (needed).", html! {
                        label.field { "Announce after (min)" input type="number" name="down_wait" min="1" max="60" value=(a.down_wait); small { "A shorter outage stays quiet." } }
                        label.field { "Health path" input type="text" name="health_path" value=(a.health_path) maxlength="200" list="health-paths"; small { "The page the check opens." } }
                    }))
                    (alert_row("traffic", a.traffic, "Traffic stopped", "Much longer than usual, after four weeks of history.", html! {}))
                    (alert_row("regressions", a.regressions, "Regressions and new errors", "Checked every hour: more errors after a release or a flag, pages turning slow, a new error hitting several users.", html! {}))
                    (alert_row("failing", a.failing, "Failing requests spike", "Many more failed requests than usual.", html! {}))
                    (alert_row("cert", a.cert, "Certificate expiring", "14 days before it ends.", html! {}))
                    (alert_row("budget_spent", a.budget_spent, "Error budget spent", "With an availability target (Uptime): once, when its 30-day budget is used up.", html! {}))
                    (alert_row("budget_burn", a.budget_burn, "Error budget burning fast", "At the last day's rate, what is left of the budget is gone within 2 days.", html! {}))
                }
                datalist id="health-paths" { option value="/" {} option value="/healthz" {} option value="/readyz" {} option value="/health" {} }
                fieldset {
                    legend { "Quiet hours" }
                    div.form-grid {
                        label.field { "From" (hour_select("quiet_start", a.quiet.map(|q| q.0))) small { "In the project's time zone." } }
                        label.field { "To" (hour_select("quiet_end", a.quiet.map(|q| q.1))) small { "Down and an expired certificate always go out." } }
                    }
                }
                fieldset {
                    legend { "Channels" }
                    label.switch {
                        input type="checkbox" role="switch" name="email" checked[a.email];
                        span { b { "Email" } small { @if a.smtp { "To the alert list, the digest list, or the admins." } @else { "SMTP not set: no email goes out." } } }
                    }
                    div.form-grid {
                        label.field { "Webhook (Slack, Discord…)" input type="url" name="webhook" value=(a.webhook) placeholder="https://hooks.slack.com/…" maxlength="500"; small { "Empty: no webhook." } }
                        label.field { "ntfy topic" input type="text" name="ntfy" value=(a.ntfy) placeholder="my-app-alerts" maxlength="300"; small { "A topic on ntfy.sh, or a full https:// topic URL." } }
                    }
                }
                (form_foot("Save", html! { "One message when an incident starts, one when it ends." }))
            }
            section.card.wide {
                form.inline method="post" action={ (base) "/alerts/test" } { button.soft type="submit" { "Send a test alert" } }
                p.muted.small { "Emails go to " code { "OPENLENS_ALERT_TO" } " (or the digest list, or the admins). ntfy: a topic on ntfy.sh, or a full https:// topic URL of your own server; install the ntfy app to get pushes on a phone. Quiet hours (the project's time zone) hold non-urgent alerts until they end; " em { "down" } " and an expired certificate always go out. An incident that lasts is repeated at most once an hour." }
            }
        }
    }
}

/// *Sharing* (#229): the form card, then the links as a table.
fn sharing(v: &SettingsView<'_>, base: &str) -> Markup {
    html! {
        @if let Some(card) = &v.status_page { (card) }
        @if !v.admin { section.card { p.muted { "Admins make and revoke share links." } } }
        @if v.admin {
            form.card.wide.form-card #share method="post" action={ (base) "/share" } aria-labelledby="share-h" {
                div.section-head { h2 #share-h { (icon("route")) "Share links" } }
                p.small { "A read-only link to the Focus board or the weekly digest, for people without an account: no SQL, people, replays or settings behind it. Filters are fixed when the link is made." }
                div.form-grid {
                    fieldset.field {
                        legend { "What" }
                        div.seg {
                            label { input type="radio" name="scope" value="board" checked; "Focus board" }
                            label { input type="radio" name="scope" value="digest"; "Weekly digest" }
                        }
                    }
                    label.field { "Release" input type="text" name="release" placeholder="all" maxlength="100" list="share-releases"; small { "Empty: every release." } }
                    label.field { "Expires after (days)" input type="number" name="days" min="0" max="366" value="30"; small { "0: never." } }
                    label.field { "May be embedded on" input type="text" name="embed" placeholder="https://intranet.example.com" maxlength="500" list="embed-on"; small { "Empty: it cannot be embedded in another site." } }
                }
                datalist id="share-releases" { @for r in v.releases { option value=(r) {} } }
                datalist id="embed-on" { @if let Some(u) = v.app_url.or(v.detected_url) { option value=(u) {} } }
                (form_foot("Make a link", html! { "Anyone with the link reads what it shows, until it expires or is revoked." }))
            }
            @if !v.shares.is_empty() {
                section.card.wide {
                    h2.sky { (icon("route")) "Links" }
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
}

/// *Data*: explore, export, and the project's deletion.
fn data(p: &ProjectInfo<'_>, v: &SettingsView<'_>, base: &str) -> Markup {
    html! {
        @if v.admin {
            section.card.wide {
                h2.violet { (icon("code")) "Explore the data" }
                p { a.btn href={ "/p/" (p.id) "/sql" } { "Read-only SQL" } " " a.btn href={ "/p/" (p.id) "/breakdown?by=role" } { "Break down" } }
            }
        }
        form.card.wide.form-card method="get" action={ "/p/" (p.id) "/export" } aria-labelledby="export-h" {
            div.section-head { h2 #export-h { (icon("down")) "Export" } }
            div.form-grid {
                label.field { "Day" input type="date" name="date" value=(v.yesterday) required; small { "A UTC day: one partition of events." } }
            }
            (form_foot("Download NDJSON", html! { "One line per event of that day, up to 500,000." }))
        }
        @if v.admin {
            @let page = format!("{base}/data");
            @let what = "Removes its events, replays, source maps, people, flags, surveys and feature map. There is no undo (backups excepted).";
            (crate::detail::danger_zone("data-danger", "Danger zone", crate::detail::danger_item("Delete the project",
                html! { (what) },
                html! { a.btn.danger-btn href=(crate::detail::confirm_href(&page, Confirm::Delete)) { "Delete the project…" } })))
            @if v.confirm == Some(Confirm::Delete) {
                (crate::detail::confirm_dialog(&format!("Delete {}?", p.name), html! { (what) },
                    crate::detail::typed_form(&format!("{base}/delete"), p.name, "Delete forever", &page)))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// The `<input …>` tags of `html`, without their `<input `.
    pub(crate) fn inputs(html: &str) -> Vec<&str> {
        html.split("<input ")
            .skip(1)
            .map(|t| &t[..t.find('>').unwrap_or(t.len())])
            .collect()
    }

    /// Three required steps, `done` of them done, and two optional ones.
    fn steps(done: usize) -> Vec<SetupStep> {
        let s = |label, done, required, href: &str| SetupStep {
            label,
            done,
            required,
            href: href.into(),
        };
        vec![
            s("First event received", done > 0, true, "/s/install"),
            s("App address known", done > 1, true, "/s/general"),
            s("An alert channel", done > 2, true, "/s/alerts"),
            s("Releases detected", false, false, "/s/releases"),
            s("SMTP for emails", true, false, "/s/smtp"),
        ]
    }

    /// The `width` of the `<rect class="{class}">` in `html`.
    fn rect_width(html: &str, class: &str) -> f64 {
        let at = html.find(&format!(r#"<rect class="{class}""#)).unwrap();
        let tag = &html[at..at + html[at..].find('>').unwrap()];
        let w = tag.split(r#"width=""#).nth(1).unwrap();
        w[..w.find('"').unwrap()].parse().unwrap()
    }

    /// The markup of the step labelled `label`.
    fn step_block<'a>(html: &'a str, label: &str) -> &'a str {
        let at = html.find(&format!("<b>{label}</b>")).unwrap();
        let start = html[..at].rfind(r#"<div class="step "#).unwrap();
        let end = [r#"<div class="step "#, "<details", "</details>"]
            .iter()
            .filter_map(|m| html[at..].find(m))
            .min()
            .map_or(html.len(), |e| at + e);
        &html[start..end]
    }

    fn view(tab: SettingsTab) -> SettingsView<'static> {
        SettingsView {
            tab,
            timezone: "Europe/Paris",
            app_url: None,
            detected_url: Some("https://app.example.com"),
            host: "https://lens.example.com",
            checks: &[],
            map_version: None,
            admin: true,
            notice: None,
            yesterday: "2026-11-25",
            alerts: AlertsView::default(),
            shares: vec![],
            setup: steps(1),
            releases: &[],
            environments: Some(openlens_engine::environments::Environments {
                days: 30,
                hosts: vec![],
                rules: vec![],
                unclassified: vec![],
                aliases: vec![],
                hidden: vec![],
                monitors: vec![],
            }),
            relabel: None,
            allow: None,
            allow_preview: None,
            status_page: None,
            confirm: None,
        }
    }

    const P: ProjectInfo<'static> = ProjectInfo {
        id: "p1",
        name: "Shop",
        key: "pk_test",
    };

    #[test]
    fn the_setup_bar_grows_with_the_steps_done_and_stays_in_its_track() {
        let mut last = -1.0;
        for done in 0..=3 {
            let html = setup_progress(&steps(done)).into_string();
            let (bar, track) = (rect_width(&html, "bar"), rect_width(&html, "track"));
            assert!(bar > last, "{done} done: {bar} after {last}");
            assert!((0.0..=track).contains(&bar), "{bar} in {track}");
            last = bar;
        }
        let all = setup_progress(&steps(3)).into_string();
        assert!(rect_width(&all, "track") - rect_width(&all, "bar") < 1e-9);
        // Only optional steps: the bar is full, never past its track.
        let optional: Vec<SetupStep> = steps(0).into_iter().filter(|s| !s.required).collect();
        let html = setup_progress(&optional).into_string();
        assert!(rect_width(&html, "bar") <= rect_width(&html, "track"));
    }

    #[test]
    fn done_steps_are_ticked_and_steps_to_do_keep_their_action() {
        for done in 0..=3 {
            let list = steps(done);
            let html = setup_progress(&list).into_string();
            for st in &list {
                let block = step_block(&html, st.label);
                let link = format!(r#"href="{}""#, st.href);
                if st.done {
                    assert!(block.contains(r#"class="step done""#), "{block}");
                    assert!(!block.contains(&link), "{block}");
                } else {
                    assert!(block.contains(r#"class="step todo""#), "{block}");
                    assert!(block.contains(&link), "{block}");
                }
            }
            // One primary action at most: the next required step.
            let primary = html.matches("btn primary").count();
            assert_eq!(primary, usize::from(done < 3), "{done} done");
            // The optional steps are folded.
            let fold = html.find("<details").unwrap();
            assert!(html.find("<b>SMTP for emails</b>").unwrap() > fold);
        }
    }

    #[test]
    fn every_alert_type_is_a_switch_in_the_form_that_posts_the_alerts() {
        let html = project_settings(&P, &view(SettingsTab::Alerts)).into_string();
        let at = html.find(r#"action="/p/p1/settings/alerts""#).unwrap();
        let form = &html[at..at + html[at..].find("</form>").unwrap()];
        for name in [
            "down",
            "traffic",
            "regressions",
            "failing",
            "cert",
            "budget_spent",
            "budget_burn",
            "email",
        ] {
            let tag = inputs(form)
                .into_iter()
                .find(|t| t.contains(&format!(r#"name="{name}""#)))
                .unwrap();
            assert!(
                tag.contains(r#"type="checkbox""#) && tag.contains(r#"role="switch""#),
                "{name}: {tag}"
            );
        }
        // A threshold sits in the row of its alert type.
        let down = form
            .split("<li>")
            .find(|r| r.contains(r#"name="down""#))
            .unwrap();
        assert!(down.contains(r#"name="down_wait""#) && down.contains(r#"name="health_path""#));
    }

    #[test]
    fn the_settings_tabs_need_no_inline_style_or_script() {
        for tab in [
            SettingsTab::General,
            SettingsTab::Install,
            SettingsTab::Alerts,
            SettingsTab::Sharing,
            SettingsTab::Environments,
            SettingsTab::Data,
        ] {
            let html = project_settings(&P, &view(tab)).into_string();
            assert!(!html.contains(" style="), "{tab:?}");
            assert!(!html.contains("<script"), "{tab:?}");
            assert!(
                html.contains("form-card") || html.contains("form-body"),
                "{tab:?}"
            );
        }
    }

    #[test]
    fn the_two_option_choices_keep_one_checked_radio() {
        // Sharing's "What" and an environment check's alert level.
        for (tab, name) in [
            (SettingsTab::Sharing, "scope"),
            (SettingsTab::Environments, "level"),
        ] {
            let html = project_settings(&P, &view(tab)).into_string();
            let field = format!(r#"name="{name}""#);
            let radios: Vec<&str> = inputs(&html)
                .into_iter()
                .filter(|t| t.contains(r#"type="radio""#) && t.contains(&field))
                .collect();
            assert!(radios.len() >= 2, "{name}");
            let checked = radios.iter().filter(|t| t.contains(" checked")).count();
            assert_eq!(checked, 1, "{name}");
        }
    }
}

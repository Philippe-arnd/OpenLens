//! The public status page (#561) and its Settings card, in the v2 design
//! (`docs/design/ui-v2`). The page is for anyone, without signing in: no
//! shell, no link into the dashboard, no script, no form, no inline style;
//! it refreshes itself every minute and follows the system's light or dark
//! theme. Everything it prints comes from
//! [`openlens_engine::commands::status_page::Public`], which holds only
//! public fields; posted text is escaped and only `http(s)://` words
//! become links.

use openlens_engine::commands::status_page::{
    Public, PublicAnnouncement, PublicComponent, PublicDay,
};

use super::*;

/// A time as the page shows it: `27 Nov 2026, 14:02 UTC`.
fn at(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| {
        d.format("%-d %b %Y, %H:%M UTC").to_string()
    })
}

/// A time of day, `14:02`, for the end of something that started the
/// same day.
fn clock(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map_or_else(String::new, |d| d.format("%H:%M").to_string())
}

fn same_day(a: i64, b: i64) -> bool {
    let d = |t| chrono::DateTime::from_timestamp(t, 0).map(|d| d.date_naive());
    d(a).is_some() && d(a) == d(b)
}

/// From `a` to `b` in words: `27 Nov 2026, 14:02 UTC to 14:20`.
fn span(a: i64, b: Option<i64>) -> String {
    match b {
        Some(b) if same_day(a, b) => format!("{} to {}", at(a), clock(b)),
        Some(b) => format!("{} to {}", at(a), at(b)),
        None => format!("since {}", at(a)),
    }
}

/// The page's headline for a state.
#[must_use]
pub fn state_words(state: &str) -> &'static str {
    match state {
        "operational" => "Everything is working",
        "degraded" => "Partly degraded",
        "down" => "Down",
        "maintenance" => "Under planned maintenance",
        _ => "Not checked yet",
    }
}

/// A component's state, short.
fn chip_words(state: &str) -> &'static str {
    match state {
        "operational" => "Working",
        "degraded" => "Degraded",
        "down" => "Down",
        "maintenance" => "Maintenance",
        _ => "Not checked",
    }
}

/// The v2 tag tone of a state or an announcement level.
fn tone(state: &str) -> &'static str {
    match state {
        "operational" | "resolved" => "tone-good",
        "degraded" | "maintenance" => "tone-warn",
        "down" | "outage" => "tone-bad",
        _ => "tone-info",
    }
}

fn pct(a: Option<f64>) -> String {
    a.map_or_else(|| "—".to_owned(), |a| format!("{:.2}%", a * 100.0))
}

fn day_title(d: &PublicDay) -> String {
    match d.availability {
        Some(a) => format!("{} (UTC): {:.2}% available", d.day, a * 100.0),
        None => format!("{} (UTC): not checked", d.day),
    }
}

fn component(c: &PublicComponent) -> Markup {
    let figure = |w: &str| {
        c.availability
            .iter()
            .find(|f| f.window == w)
            .and_then(|f| f.availability)
    };
    html! {
        section.card.status-component {
            div.status-line {
                h2 { (c.name) }
                span class={ "tag " (tone(c.state)) } { (chip_words(c.state)) }
            }
            div.uptime-days role="img" aria-label=(format!("{}: daily availability over {} days, {} over 90 days", c.name, c.days.len(), pct(figure("90d")))) {
                @for d in &c.days { span class={ "uptime-day " (d.state) } title=(day_title(d)) {} }
            }
            dl.status-figures {
                div { dt { "24 hours" } dd { (pct(figure("24h"))) } }
                div { dt { "30 days" } dd { (pct(figure("30d"))) } }
                div { dt { "90 days" } dd { (pct(figure("90d"))) } }
                @if let Some(ms) = c.response_p95_ms { div { dt { "Response time (p95, 24 h)" } dd { (ms) " ms" } } }
                @if let Some(s) = &c.slo {
                    div { dt { "Target" } dd { (format!("{:.2}%", s.target * 100.0)) } }
                    div { dt { "Error budget left" } dd { (openlens_engine::maintenance::length(s.left_seconds.max(0))) " of " (openlens_engine::maintenance::length(s.budget_seconds.max(0))) } }
                }
            }
        }
    }
}

fn announcement(a: &PublicAnnouncement) -> Markup {
    html! {
        li id={ "a" (a.id) } {
            div.status-line {
                strong { (a.title) }
                span class={ "tag " (tone(&a.level)) } { (a.level) }
            }
            p.muted.small { (a.components.join(", ")) " · " (span(a.starts_at, a.ends_at)) }
            @if !a.body.is_empty() { p.comment-body { (crate::announcements::linked(&a.body)) } }
            @if !a.updates.is_empty() {
                ol.status-updates { @for u in &a.updates {
                    li { strong { (u.state) } span.muted { " · " (at(u.at)) } " — " span.comment-body { (crate::announcements::linked(&u.body)) } }
                } }
            }
        }
    }
}

/// A calm drawing for the empty state: a frame with a check mark.
fn art() -> Markup {
    html! {
        svg.art viewBox="0 0 96 72" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            rect x="10" y="8" width="76" height="56" rx="10" {}
            path d="M34 37l9 9 19-19" {}
        }
    }
}

/// The public page of `v`, served at `/status/<slug>`.
#[must_use]
pub fn status_page(v: &Public, slug: &str) -> Markup {
    let base = format!("/status/{slug}");
    let (current, past): (Vec<&PublicAnnouncement>, Vec<&PublicAnnouncement>) = v
        .announcements
        .iter()
        .filter(|a| v.notice.as_ref().is_none_or(|n| n.id != a.id))
        .partition(|a| a.active || a.starts_at > v.generated_at);
    html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                meta http-equiv="refresh" content="60";
                title { (v.name) " status" }
                link rel="stylesheet" href=(crate::css_href());
                link rel="icon" href={ (base) "/icon" };
                link rel="alternate" type="application/atom+xml" title={ (v.name) " announcements" } href={ (base) "/feed.atom" };
            }
            body.status-page {
                main.status {
                    header.status-head {
                        img.status-icon src={ (base) "/icon" } alt="" width="40" height="40";
                        div {
                            h1 { (v.name) }
                            p.muted { "Service status" }
                        }
                    }
                    section.card.status-now {
                        span class={ "tag " (tone(v.state)) } { (state_words(v.state)) }
                        @if let Some(s) = v.since {
                            p.muted { (if v.state == "operational" { "Last incident ended " } else { "Since " }) (at(s)) "." }
                        }
                    }
                    @if let Some(n) = &v.notice {
                        section.card.status-list {
                            div.status-line { strong { (n.title) } span.tag.tone-info { "Notice" } }
                            @if !n.body.is_empty() { p.comment-body { (crate::announcements::linked(&n.body)) } }
                        }
                    }
                    @if !v.maintenance.is_empty() {
                        section.card.status-list {
                            h2 { "Planned maintenance" }
                            ul { @for m in &v.maintenance {
                                li {
                                    div.status-line {
                                        strong { (m.components.join(", ")) }
                                        @if m.state == "running" { span.tag.tone-warn { "Now" } } @else { span.tag.tone-info { "Planned" } }
                                    }
                                    p.muted.small { (span(m.starts_at, Some(m.ends_at))) }
                                }
                            } }
                        }
                    }
                    @if !current.is_empty() {
                        section.card.status-list {
                            h2 { "Now" }
                            ul { @for a in &current { (announcement(a)) } }
                        }
                    }
                    @for c in &v.components { (component(c)) }
                    p.muted.small.status-legend { "Green: no down time. Amber: down less than an hour. Red: down an hour or more. Grey: not checked. Days are UTC." }
                    @if !past.is_empty() {
                        section.card.status-list {
                            h2 { "Past announcements" }
                            ul { @for a in &past { (announcement(a)) } }
                        }
                    }
                    @if v.incidents.is_empty() {
                        section.card.empty {
                            (art())
                            div {
                                h2 { "No incident in 90 days" }
                                p { "No component was detected down over the last 90 days." }
                            }
                        }
                    } @else {
                        section.card.table-card {
                            h2 { "Detected incidents, last 90 days" }
                            div.table-scroll { table {
                                thead { tr { th { "Component" } th { "Down" } th.num { "Length" } } }
                                tbody { @for i in &v.incidents {
                                    tr {
                                        td { (i.component) @if i.ended_at.is_none() { " " span.tag.tone-bad { "Ongoing" } } }
                                        td { (span(i.started_at, i.ended_at)) }
                                        td.num { (openlens_engine::maintenance::length(i.seconds)) }
                                    }
                                } }
                            } }
                        }
                    }
                    footer.status-foot {
                        "Times are UTC. This page refreshes every minute (last at " (at(v.generated_at)) "). "
                        a href={ (base) ".json" } { "JSON" } " · "
                        a href={ (base) "/feed.atom" } { "Atom feed" } " · "
                        a href={ (base) "/badge.svg" } { "Badge" }
                        " · Served by OpenLens."
                    }
                }
            }
        }
    }
}

/// One component row of the Settings card.
#[derive(Debug, Clone, Default)]
pub struct StatusComponentRow {
    /// `production` or a monitored environment.
    pub environment: String,
    /// Its public name (empty when not shown yet).
    pub name: String,
    /// Shown on the page.
    pub shown: bool,
    /// Its response time is shown.
    pub response_time: bool,
}

/// What the Settings card shows (#561).
#[derive(Debug, Clone, Default)]
pub struct StatusPageCard {
    /// The page is on.
    pub enabled: bool,
    /// The slug, if one was chosen.
    pub slug: String,
    /// The page's full address, once there is a slug.
    pub url: Option<String>,
    /// Production first, then every monitored environment.
    pub components: Vec<StatusComponentRow>,
    /// Admins see the form.
    pub admin: bool,
}

/// Settings → Sharing: the public status page's card, a v2 form card.
#[must_use]
pub fn status_page_card(p: &ProjectInfo<'_>, v: &StatusPageCard) -> Markup {
    html! {
        section.card.form-card #status-page {
            div {
                h2 { (icon("pulse")) "Public status page" }
                p.muted.small {
                    "A read-only page anyone can open without signing in: whether the app works now, 90 days of daily bars, availability, announcements and planned maintenance. "
                    "It never shows an address, a health path, an error text, an event or a person. Off by default."
                }
                @if let Some(u) = &v.url {
                    p.small { @if v.enabled { span.tag.tone-good { "On" } " " a href=(u) { (u) } } @else { span.tag.tone-info { "Off" } " Its address is kept: " (u) } }
                }
            }
            @if v.admin {
                form method="post" action={ "/p/" (p.id) "/settings/status-page" } {
                    div.form-grid {
                        div.field {
                            span { "Page" }
                            label.check { input type="checkbox" name="enabled" checked[v.enabled]; " Serve it to anyone" }
                            small { "Off: the page and its JSON, badge and feed answer 404." }
                        }
                        label.field {
                            span { "Address" }
                            input type="text" name="slug" value=(v.slug) placeholder="my-app" pattern="[a-z0-9][a-z0-9-]{1,38}[a-z0-9]" minlength="3" maxlength="40";
                            small { "/status/<address>: 3 to 40 lowercase letters, digits and dashes. One project per address." }
                        }
                    }
                    div.table-scroll { table {
                        thead { tr { th { "Show" } th { "Environment" } th { "Public name" } th { "Response time" } } }
                        tbody { @for c in &v.components {
                            @let prod = c.environment == "production";
                            tr {
                                td { @if prod { input type="checkbox" checked disabled aria-label="Production is always shown"; } @else { input type="checkbox" name={ "show:" (c.environment) } checked[c.shown] aria-label={ "Show " (c.environment) }; } }
                                td.mono { (c.environment) }
                                td { input type="text" name={ "name:" (c.environment) } value=(c.name) placeholder=(if prod { "App" } else { "API" }) maxlength="60" aria-label={ "Public name of " (c.environment) }; }
                                td { input type="checkbox" name={ "rt:" (c.environment) } checked[c.response_time] aria-label={ "Show the response time of " (c.environment) }; }
                            }
                        } }
                    } }
                    small.muted { "Other environments can be shown once they are monitored (Environments tab). A dedicated host name (status.example.com) is a reverse-proxy rule: see the deploy guide." }
                    div.form-foot {
                        button.btn.primary type="submit" { "Save" }
                        p { "Saving replaces the published page at once." }
                    }
                }
            }
        }
    }
}

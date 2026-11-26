//! Status announcements on the Uptime page, and the page that writes,
//! updates and closes them (#562). Every posted text is shown as text:
//! maud escapes it, and only `http(s)://` words become links.

use super::*;

/// One announcement as shown (times already in the reader's zone).
#[derive(Debug, Clone, Default)]
pub struct NoticeRow {
    /// Id.
    pub id: i64,
    /// Title.
    pub title: String,
    /// Body, plain text.
    pub body: String,
    /// `information`, `degraded`, `outage` or `resolved`.
    pub level: String,
    /// Components, comma-separated.
    pub components: String,
    /// Start.
    pub start: String,
    /// End, if any.
    pub end: Option<String>,
    /// Pinned at the top.
    pub pinned: bool,
    /// Started and not ended.
    pub active: bool,
    /// Follows an incident.
    pub incident: Option<i64>,
    /// Its updates, oldest first: (time, state, text).
    pub updates: Vec<(String, String, String)>,
}

/// Something proposed about an incident.
#[derive(Debug, Clone)]
pub struct OfferRow {
    /// `write_update` or `mark_resolved`.
    pub kind: &'static str,
    /// The incident id.
    pub incident: i64,
    /// What happened (its cause).
    pub what: String,
    /// When it started.
    pub since: String,
    /// The announcement to resolve.
    pub announcement: Option<i64>,
}

/// The announcements part of the Uptime page.
#[derive(Debug, Clone, Default)]
pub struct NoticesView {
    /// Newest first.
    pub items: Vec<NoticeRow>,
    /// Proposals on recent incidents.
    pub offers: Vec<OfferRow>,
    /// Announcements also go to the alert channels.
    pub send: bool,
}

fn level_class(level: &str) -> &'static str {
    match level {
        "outage" => "bad",
        "degraded" => "warn",
        "resolved" => "good",
        _ => "muted",
    }
}

/// Plain text, with `http(s)://` words as links (escaped like the rest).
pub fn linked(text: &str) -> Markup {
    html! {
        @for piece in text.split_inclusive(char::is_whitespace) {
            @let word = piece.trim_end();
            @let space = piece.get(word.len()..).unwrap_or("");
            @let url = word.trim_end_matches(['.', ',', ')', ';', ':', '!', '?']);
            @let rest = word.get(url.len()..).unwrap_or("");
            @if (url.starts_with("https://") || url.starts_with("http://")) && url.len() > 8 && !url.contains(['"', '<', '>', '\\']) {
                a href=(url) rel="nofollow noopener noreferrer" { (url) } (rest) (space)
            } @else {
                (piece)
            }
        }
    }
}

fn notice(p: &ProjectInfo<'_>, n: &NoticeRow, admin: bool) -> Markup {
    let base = format!("/p/{}/uptime/announcements/{}", p.id, n.id);
    html! {
        li id={ "a" (n.id) } {
            span class={ "status " (level_class(&n.level)) } { (n.level) }
            @if n.pinned { " " span.chip { "pinned" } }
            " " strong { (n.title) }
            span.muted.small { " · " (n.components) " · " (n.start) @if let Some(e) = &n.end { " → " (e) } @else { " → ongoing" } }
            @if !n.body.is_empty() { p.comment-body { (linked(&n.body)) } }
            @if !n.updates.is_empty() {
                ol.plain-list.small { @for (t, state, text) in &n.updates {
                    li { strong { (state) } span.muted { " · " (t) } " — " span.comment-body { (linked(text)) } }
                } }
            }
            @if admin {
                div.small {
                    a href=(base) { "Edit or write an update" }
                    @if n.active && n.level != "resolved" {
                        " "
                        form.inline method="post" action={ (base) "/close" } {
                            button.ghost type="submit" { (if n.level == "information" { "End it" } else { "Mark resolved" }) }
                        }
                    }
                }
            }
        }
    }
}

/// The announcements section of the Uptime page: what the team posted
/// (90 days), what to do about recent incidents, and the sending switch.
pub fn notices(p: &ProjectInfo<'_>, v: &NoticesView) -> Markup {
    let admin = viewer_is_admin();
    let base = format!("/p/{}/uptime/announcements", p.id);
    html! {
        section.card.wide {
            h2.violet { (icon("flag")) "Announcements, last 90 days" }
            @if admin && !v.offers.is_empty() {
                ul.plain-list { @for o in &v.offers {
                    li {
                        span.status.bad { "incident" } " " (o.what) span.muted.small { " · since " (o.since) } " "
                        @if o.kind == "write_update" {
                            a href={ (base) "/new?incident=" (o.incident) } { "Write an update" }
                        } @else if let Some(a) = o.announcement {
                            "The probe closed it. "
                            form.inline method="post" action={ (base) "/" (a) "/close" } {
                                button type="submit" { "Mark resolved" }
                            }
                        }
                    }
                } }
            }
            @if v.items.is_empty() { p.muted { "None. An announcement tells users what is going on: a slowdown, an outage and its updates, a maintenance." } }
            @else {
                ul.plain-list { @for n in &v.items { (notice(p, n, admin)) } }
            }
            @if admin {
                p { a href={ (base) "/new" } { (icon("plus")) "Write an announcement" } }
                form.inline method="post" action={ (base) "/send" } {
                    input type="hidden" name="send" value=(if v.send { "off" } else { "on" });
                    span.small.muted { "Sent to the alert channels: " strong { (if v.send { "yes" } else { "no" }) } " " }
                    button.ghost type="submit" { (if v.send { "Stop sending them" } else { "Send them too" }) }
                }
            }
        }
    }
}

/// The form of an announcement: new (with an incident to follow, maybe)
/// or one to edit, with its update and close forms.
#[derive(Debug, Clone, Default)]
pub struct NoticeForm {
    /// The announcement edited; `None` for a new one.
    pub id: Option<i64>,
    /// The incident it follows, if any.
    pub incident: Option<i64>,
    /// What that incident is.
    pub incident_text: Option<String>,
    /// Title.
    pub title: String,
    /// Body.
    pub body: String,
    /// Level.
    pub level: String,
    /// Components, comma-separated.
    pub components: String,
    /// Start (`YYYY-MM-DDTHH:MM`, the reader's zone), empty for now.
    pub start: String,
    /// End, the same way, empty for none.
    pub end: String,
    /// Pinned.
    pub pinned: bool,
    /// The edited one's updates, oldest first: (time, state, text).
    pub updates: Vec<(String, String, String)>,
    /// Still showing (an edited one).
    pub active: bool,
    /// Why the last post was refused.
    pub error: Option<String>,
}

/// The page that writes an announcement, or edits one, posts its
/// updates, closes and deletes it.
pub fn notice_page(p: &ProjectInfo<'_>, f: &NoticeForm) -> Markup {
    let base = format!("/p/{}/uptime/announcements", p.id);
    let action =
        f.id.map_or_else(|| base.clone(), |id| format!("{base}/{id}"));
    let levels = openlens_engine::commands::announcements::LEVELS;
    let states = openlens_engine::commands::announcements::STATES;
    layout(
        "Uptime",
        Some(p),
        html! {
            div.heading { div {
                h1 { @if f.id.is_some() { "Announcement" } @else { "New announcement" } }
                p.muted { a href={ "/p/" (p.id) "/uptime" } { "← Uptime" } " · shown on the Uptime page and the status page; plain text, links are kept." }
            } }
            @if let Some(e) = &f.error { p.alert role="alert" { (e) } }
            @if let Some(t) = &f.incident_text { p { "Follows the incident: " strong { (t) } } }
            @if f.id.is_some() {
                section.card.wide {
                    h2 { (icon("clock")) "Write an update" }
                    @if !f.updates.is_empty() {
                        ol.plain-list.small { @for (t, state, text) in &f.updates {
                            li { strong { (state) } span.muted { " · " (t) } " — " span.comment-body { (linked(text)) } }
                        } }
                    }
                    form.settings method="post" action={ (action) "/updates" } {
                        label { "State "
                            select name="state" { @for s in states { option value=(s) { (s) } } }
                        }
                        label.wide { "What is happening "
                            textarea name="body" rows="4" required maxlength="4000" {}
                        }
                        button type="submit" { "Post the update" }
                    }
                    p.muted.small { "A resolved update also marks the announcement resolved and ends it." }
                }
            }
            section.card.wide {
                @if f.id.is_some() { h2 { (icon("gear")) "Edit" } }
                form.settings method="post" action=(action) {
                    @if let Some(i) = f.incident { input type="hidden" name="incident" value=(i); }
                    label.wide { "Title " input type="text" name="title" value=(f.title) required maxlength="200" placeholder="Slower exports this afternoon"; }
                    label.wide { "Text "
                        textarea name="body" rows="5" maxlength="4000" { (f.body) }
                    }
                    label { "Level "
                        select name="level" { @for l in levels { option value=(l) selected[l == f.level] { (l) } } }
                    }
                    @if f.id.is_none() && f.incident.is_some() {
                        label { "First update "
                            select name="state" { @for s in states { option value=(s) { (s) } } }
                        }
                    }
                    label { "Components " input type="text" name="components" value=(f.components) maxlength="400" placeholder="production"; }
                    label { "Start " input type="datetime-local" name="start" value=(f.start); }
                    label { "End " input type="datetime-local" name="end" value=(f.end); }
                    label.check { input type="checkbox" name="pinned" checked[f.pinned]; " Pin it at the top (information only; unpins the others)" }
                    button type="submit" { (if f.id.is_some() { "Save" } else { "Post" }) }
                }
                p.muted.small { "Components: production or environment names, comma-separated. Start empty: now. Times are in your time zone." }
            }
            @if let Some(id) = f.id {
                section.card.wide.danger {
                    h2.coral { (icon("alert")) "Close or delete" }
                    @if f.active {
                        form.settings method="post" action={ (base) "/" (id) "/close" } {
                            label.wide { "Closing word (optional) " input type="text" name="body" maxlength="4000" placeholder="Fixed: exports are back to normal."; }
                            button type="submit" { "Mark resolved" }
                        }
                    }
                    form.inline method="post" action={ (base) "/" (id) "/delete" } {
                        button.ghost type="submit" { "Delete it" }
                    }
                }
            }
        },
    )
}

//! A person's profile, breakdowns and the SQL page.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// How many of a person's latest events their page lists (#590): the
/// rest are one link away, on the Events page filtered on them.
pub(crate) const LATEST: usize = 30;

/// A person's profile (#222): who they are, their role over time, their
/// recordings, their latest events and errors. Admins get the privacy
/// danger zone; `confirm` opens the dialog that forgets them (#545).
#[allow(clippy::too_many_arguments)]
pub fn person(
    p: &ProjectInfo<'_>,
    who: &str,
    person: Option<&openlens_meta::Person>,
    roles: &[(String, String)],
    replays: &[openlens_replay::Summary],
    events: &[EventRow],
    tz: &str,
    admin: bool,
    confirm: Option<Confirm>,
) -> Markup {
    use crate::detail::{
        confirm_dialog, confirm_href, danger_item, danger_zone, entity, typed_form,
    };
    let errors: Vec<&EventRow> = events
        .iter()
        .filter(|e| openlens_engine::family(&e.event) == "error")
        .collect();
    let tf = |t: &str| local_time(t, tz, false);
    let page = format!("/p/{}/people/{}", p.id, url_enc(who));
    let all_events = format!("/p/{}/events?person={}&days=90", p.id, url_enc(who));
    let facts = html! {
        @if let Some(x) = person {
            dl.facts {
                @if let Some(r) = &x.role { div { dt { "Role" } dd { span.tag.tone-accent { (r) } } } }
                @if let Some(a) = &x.account { div { dt { "Account" } dd { (a) } } }
                @if let Some(pl) = &x.plan { div { dt { "Plan" } dd { (pl) } } }
                @if let Some(s) = &x.signed_up { div { dt { "Signed up" } dd { (tf(s)) } } }
                div { dt { "First seen" } dd { (tf(&x.first_seen)) } }
                div { dt { "Last login" } dd { (tf(&x.last_seen)) } }
            }
        } @else { p.muted { "Anonymous visitor, never logged in." } }
    };
    // The trail ends on the person (#590): `… › Audience › u276`.
    let mut crumbs = Crumbs::of("Person", Some(p));
    if let Some(last) = crumbs.0.last_mut() {
        last.1 = Some(format!("/p/{}/audience", p.id));
    }
    crumbs.0.push((who.to_owned(), None));
    layout_with(
        "Person",
        Some(p),
        &crumbs,
        html! {
            (entity(who, html! { span.mono { (who) } }, facts, html! { a.btn href=(all_events) { "All their events" } }))
            div.kpis {
                section.card.mini aria-labelledby="p-role" {
                    h2 #p-role { (icon("users")) "Role over time" }
                    @if roles.is_empty() { p.empty-line { "No role sent yet." } }
                    @else { ul { @for (r, since) in roles { li { span.tag.tone-accent { (r) } " since " (tf(since)) } } } }
                }
                section.card.mini aria-labelledby="p-rec" {
                    h2 #p-rec { (icon("play")) "Recordings" @if !replays.is_empty() { " " span.tag.tone-info { (replays.len()) } } }
                    @if replays.is_empty() { p.empty-line { "No recorded session." } }
                    @else { ul { @for r in replays.iter().take(20) {
                        li { a href={ "/p/" (p.id) "/replays/" (r.session_id) } { (icon("play")) " " (local_time(&r.started.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)) } " · " (duration((r.updated - r.started).num_seconds().max(0) as f64)) }
                    } } }
                }
                section.card.mini aria-labelledby="p-err" {
                    h2 #p-err { (icon("alert")) "Errors" @if !errors.is_empty() { " " span.tag.tone-bad { (errors.len()) } } }
                    @if errors.is_empty() { p.empty-line { "None in their latest events." } }
                    @else { ul { @for e in errors.iter().take(10) {
                        li { span.num { (tf(&e.timestamp)) } " " span.path { (e.path.as_deref().unwrap_or("—")) } @if let Some(r) = &e.release { " " span.muted { "· " (r) } }
                            @if let Some(s) = &e.session_id { " " a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (e.timestamp) } aria-label="Replay this moment" { (icon("play")) } } }
                    } } }
                    a href={ "/p/" (p.id) "/errors" } { "All errors" }
                }
            }
            @if events.is_empty() {
                section.card.wide { h2.mint { (icon("events")) "Latest events" } p.muted { "No event in 90 days." } }
            } @else {
                (crate::tables::event_table(
                    p.id,
                    "Latest events",
                    html! {
                        h2 { (icon("events")) "Latest events" }
                        a href=(all_events) { "All their events" }
                    },
                    &events[..events.len().min(LATEST)],
                    tz,
                    crate::tables::EventCols { user: false, role: false, groups: true },
                    html! {},
                ))
            }
            @if admin {
                (danger_zone("p-priv", "Privacy", html! {
                    (danger_item("Export their data", html! { "Every event, recording and profile field held for " span.mono { (who) } ", as JSON." },
                        html! { a.btn href={ (page) "/export.json" } { (icon("down")) "Export JSON" } }))
                    (danger_item("Forget this person", html! { "Removes their events (and those of the anonymous ids linked to them), their recordings and their profile, now, and drops anything they send later. There is no undo." },
                        html! { a.btn.danger-btn href=(confirm_href(&page, Confirm::Forget)) { "Forget this person…" } }))
                }))
                @if confirm == Some(Confirm::Forget) {
                    (confirm_dialog(&format!("Forget {who}?"),
                        html! { "Their events, recordings and profile are deleted now. Anything they send later is dropped. There is no undo." },
                        typed_form(&format!("{page}/forget"), who, "Forget forever", &page)))
                }
            }
        },
    )
}

/// The dimensions a breakdown can use, with their labels.
pub const BREAKDOWNS: &[(&str, &str)] = &[
    ("role", "Role"),
    ("plan", "Plan"),
    ("account", "Account"),
    ("environment", "Environment"),
    ("release", "Release"),
    ("page", "Page"),
    ("feature", "Feature"),
    ("country", "Country"),
    ("device", "Device"),
    ("browser", "Browser"),
    ("os", "OS"),
    ("channel", "Channel"),
];

/// Users, sessions, tasks and errors broken down by one dimension (#222).
pub fn breakdown(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    by: &str,
    rows: &[openlens_engine::BreakdownRow],
) -> Markup {
    use crate::tables::Col;
    let label = BREAKDOWNS
        .iter()
        .find(|(k, _)| *k == by)
        .map_or(by, |(_, l)| l);
    let max = rows.iter().map(|r| r.users).max().unwrap_or(1).max(1) as f64;
    layout_in(
        "Breakdown",
        Some(p),
        context_bar(p.id, b, f, &format!("/breakdown?by={by}"), false),
        html! {
            div.heading { div {
                h1 { "Break down by " (label.to_lowercase()) }
                p.muted { "Last 30 days, " @if by == "environment" { "every environment" } @else { strong { (f.environment) } } ". Role: the one each event was sent with." }
            } }
            nav.tabs aria-label="Dimension" {
                @for (k, l) in BREAKDOWNS {
                    a href=(filtered_href(p.id, "/breakdown", f, &[("by", k)])) aria-current=[(*k == by).then_some("page")] { (l) }
                }
            }
            @if rows.is_empty() {
                section.card.wide { p.muted { "No data in this window." } }
            } @else {
                (crate::tables::TableCard {
                    label: "Breakdown",
                    head: html! {},
                    cols: &[
                        Col::Shown(label), Col::Shown("Users"), Col::Num("Sessions"),
                        Col::Num("Tasks"), Col::Num("Errors"), Col::Num("Events"),
                    ],
                    body: html! { @for r in rows {
                        tr {
                            th scope="row" { @match &r.value { Some(v) => span.mono { (v) }, None => span.muted { "(none)" } } }
                            td.users { (crate::board::bar_with(r.users as f64 / max, &compact(r.users))) }
                            td.num { (compact(r.sessions)) }
                            td.num { (compact(r.tasks)) }
                            td.num { (compact(r.errors)) }
                            td.num.muted { (compact(r.events)) }
                        }
                    } },
                    foot: html! {},
                }
                .render())
            }
        },
    )
}

/// A link to `path` (after `/p/{project}`) with the page's own pairs and
/// every current filter of the context bar, so switching a tab keeps them.
pub(crate) fn filtered_href(pid: &str, path: &str, f: &Filters, page: &[(&str, &str)]) -> String {
    let mut kv = kept_filters(f, Ctx::Team, page);
    if f.internal {
        kv.push(("internal".into(), "1".into()));
    }
    let q = kv
        .iter()
        .map(|(k, v)| format!("{}={}", url_enc(k), url_enc(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("/p/{pid}{path}?{q}")
}

/// Read-only SQL for admins (#222): the schema, a query box, the result
/// and a CSV download.
pub fn sql(
    p: &ProjectInfo<'_>,
    query: &str,
    res: Option<&openlens_query::QueryResult>,
    error: Option<&str>,
) -> Markup {
    let cols = [
        ("uuid", "text"),
        ("event", "text"),
        ("distinct_id", "text"),
        ("session_id", "text"),
        ("timestamp", "timestamp (UTC)"),
        ("received_at", "timestamp"),
        ("url", "text"),
        ("path", "text"),
        ("feature", "text"),
        ("role", "text"),
        ("account", "text"),
        ("plan", "text"),
        ("environment", "text"),
        ("release", "text"),
        ("flags", "JSON text"),
        ("properties", "JSON text"),
        ("date", "partition YYYY-MM-DD"),
    ];
    layout(
        "SQL",
        Some(p),
        html! {
            div.heading { div {
                h1 { "SQL" }
                p.muted { "Read-only, this project only, 10,000 rows and 15 s at most. Tables: " code { "events" } " (logins merged), " code { "raw_events" } ", " code { "persons" } "." }
            } }
            div.sql-layout {
                details.card.schema open {
                    summary { (icon("layers")) " events" }
                    ul.plain-list.small { @for (c, t) in cols { li { code { (c) } " " span.muted { (t) } } } }
                    p.muted.small { "Filter on " code { "date" } " first: it skips whole days of files." }
                }
                div {
                    form.sqlbox method="post" action={ "/p/" (p.id) "/sql" } {
                        label for="sql" { "Query" }
                        textarea #sql name="sql" rows="6" required spellcheck="false" { (query) }
                        div.actions {
                            button type="submit" name="format" value="table" { "Run" }
                            button.soft type="submit" name="format" value="csv" { "Download CSV" }
                        }
                    }
                    @if let Some(e) = error { p.alert role="alert" { (e) } }
                    @if let Some(r) = res {
                        p.muted.small { (r.rows.len()) " rows" @if r.truncated { " · " strong { "cut at the limit" } } }
                        div.table-scroll { table.plain.sqlres {
                            thead { tr { @for c in &r.columns { th { (c) } } } }
                            tbody { @for row in &r.rows { tr { @for c in &r.columns {
                                td.mono.small { @match row.get(c) {
                                    None | Some(serde_json::Value::Null) => span.muted { "null" },
                                    Some(serde_json::Value::String(s)) => (s),
                                    Some(v) => (v.to_string()),
                                } }
                            } } } }
                        } }
                    }
                }
            }
        },
    )
}

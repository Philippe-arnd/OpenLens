//! A person's profile, breakdowns and the SQL page.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// A person's profile (#222): who they are, their role over time, their
/// recordings, their latest events and errors.
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
) -> Markup {
    let errors: Vec<&EventRow> = events
        .iter()
        .filter(|e| openlens_engine::family(&e.event) == "error")
        .collect();
    let tf = |t: &str| local_time(t, tz, false);
    layout(
        "Person",
        Some(p),
        html! {
            div.heading { div {
                h1.mono { (who) }
                p.muted {
                    @if let Some(x) = person {
                        @if let Some(r) = &x.role { span.chip { (r) } " " }
                        @if let Some(a) = &x.account { "account " strong { (a) } " · " }
                        @if let Some(pl) = &x.plan { "plan " strong { (pl) } " · " }
                        "first seen " (tf(&x.first_seen)) " · last login " (tf(&x.last_seen))
                    } @else { "Never logged in (anonymous visitor)." }
                    " · " a href={ "/p/" (p.id) "/events?person=" (url_enc(who)) "&days=90" } { "All their events →" }
                }
            } }
            div.grid {
                section.card {
                    h2.violet { (icon("users")) "Role over time" }
                    @if roles.is_empty() { p.muted.small { "No role sent yet." } }
                    @else { ul.plain-list { @for (r, since) in roles { li { span.chip { (r) } " since " (tf(since)) } } } }
                }
                section.card {
                    h2.sky { (icon("play")) "Recordings" }
                    @if replays.is_empty() { p.muted.small { "No recorded session." } }
                    @else { ul.plain-list { @for r in replays.iter().take(20) {
                        li { a href={ "/p/" (p.id) "/replays/" (r.session_id) } { (icon("play")) " " (local_time(&r.started.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)) } " · " (duration((r.updated - r.started).num_seconds().max(0) as f64)) }
                    } } }
                }
                section.card {
                    h2.coral { (icon("alert")) "Errors" }
                    @if errors.is_empty() { p.muted.small { "None in their latest events." } }
                    @else { ul.plain-list { @for e in errors.iter().take(10) {
                        li { (tf(&e.timestamp)) " · " span.mono { (e.path.as_deref().unwrap_or("—")) } @if let Some(r) = &e.release { " · " (r) }
                            @if let Some(s) = &e.session_id { " " a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (e.timestamp) } aria-label="Replay this moment" { (icon("play")) } } }
                    } } }
                    p.small { a href={ "/p/" (p.id) "/errors" } { "All errors →" } }
                }
            }
            @if admin {
                section.card.wide.danger {
                    h2.coral { (icon("shield")) "Privacy" }
                    p { a.button href={ "/p/" (p.id) "/people/" (url_enc(who)) "/export.json" } { (icon("down")) "Export their data (JSON)" } }
                    p.small { "Forgetting removes their events (and those of the anonymous ids linked to them), their recordings and their profile, now, and drops anything they send later. There is no undo." }
                    form.inline method="post" action={ "/p/" (p.id) "/people/" (url_enc(who)) "/forget" } {
                        label { "Type " code { (who) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                        button.danger type="submit" { "Forget this person" }
                    }
                }
            }
            section.card.wide {
                h2.mint { (icon("events")) "Latest events" }
                @if events.is_empty() { p.muted { "No event in 90 days." } }
                @else {
                    div.table-scroll { table.plain.events {
                        thead { tr { th { "When (" (tz_label(tz)) ")" } th { "Event" } th { "Page" } th { "Feature" } th { "Release" } th { span.sr-only { "Replay" } } } }
                        tbody { @for r in events {
                            @let fam = openlens_engine::family(&r.event);
                            tr {
                                td.muted.mono { (local_time(&r.timestamp, tz, true)) }
                                td { span class={ "chip fam " (fam) } { (icon(family_icon(fam))) span.sr-only { (family_label(fam)) ": " } (r.event) } }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.feature.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                                td { @if let Some(s) = &r.session_id {
                                    a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (r.timestamp) } title="Replay this moment" aria-label="Replay this moment" { (icon("play")) }
                                } }
                            }
                        } }
                    } }
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
    let label = BREAKDOWNS
        .iter()
        .find(|(k, _)| *k == by)
        .map_or(by, |(_, l)| l);
    let max = rows.iter().map(|r| r.users).max().unwrap_or(1).max(1) as f64;
    layout(
        "Breakdown",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Break down by " (label.to_lowercase()) }
                p.muted { "Last 30 days, " @if by == "environment" { "every environment" } @else { strong { (f.environment) } } ". Role: the one each event was sent with." }
            } }
            nav.tabs aria-label="Dimension" {
                @for (k, l) in BREAKDOWNS {
                    a href={ "/p/" (p.id) "/breakdown?by=" (k) "&env=" (url_enc(&f.environment)) } aria-current=[(*k == by).then_some("page")] { (l) }
                }
            }
            (context_bar(p.id, b, f, &format!("/breakdown?by={by}"), false))
            section.card.wide {
                @if rows.is_empty() { p.muted { "No data in this window." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { (label) } (th_num("Users")) th { span.sr-only { "Share" } } (th_num("Sessions")) (th_num("Tasks")) (th_num("Errors")) (th_num("Events")) } }
                        tbody { @for r in rows {
                            tr {
                                th scope="row" { @match &r.value { Some(v) => span.mono { (v) }, None => span.muted { "(none)" } } }
                                td.num { strong { (compact(r.users)) } }
                                td.meter { (meter(r.users as f64 / max)) }
                                td.num { (compact(r.sessions)) }
                                td.num { (compact(r.tasks)) }
                                td.num { (compact(r.errors)) }
                                td.num.muted { (compact(r.events)) }
                            }
                        } }
                    } }
                }
            }
        },
    )
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

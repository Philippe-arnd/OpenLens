//! The events page (moved out of the board module, #543).

use crate::*;

/// Recent events.
pub fn events(
    p: &ProjectInfo<'_>,
    b: &Board,
    rows: &[EventRow],
    f: &Filters,
    eq: &openlens_engine::EventQuery,
    next: Option<&str>,
) -> Markup {
    let mut envs = b.environments.clone();
    if !envs.iter().any(|e| e == "production") {
        envs.insert(0, "production".into());
    }
    let sort = eq.sort.as_deref().unwrap_or("time");
    let days = eq.days.unwrap_or(30);
    layout(
        "Events",
        Some(p),
        html! {
            div.heading { h1 { "Events" } p.muted {
                @match sort { "event" => "By event name", "person" => "By user", _ => "Newest first" }
                ", last " (days) (if days == 1 { " day" } else { " days" }) ", 100 per page. Times in " (f.tz) "."
            } }
            form.filters method="get" action={ "/p/" (p.id) "/events" } aria-label="Filters" {
                (filter_fields(&envs, b, f))
                label { "Family "
                    select name="family" {
                        option value="" { "All families" }
                        @for (k, label, _) in openlens_engine::FAMILIES {
                            option value=(k) selected[eq.family.as_deref() == Some(*k)] { (label) }
                        }
                    }
                }
                label { "Event " input type="text" name="event" value=[eq.event.as_deref()] placeholder="any" maxlength="200"; }
                label { "User " input type="text" name="person" value=[eq.person.as_deref()] placeholder="any" maxlength="200"; }
                label { "Page " input type="text" name="path" value=[eq.path.as_deref()] placeholder="/settings" maxlength="200"; }
                label { "Period "
                    select name="days" {
                        @for (d, l) in [(1, "24 hours"), (7, "7 days"), (30, "30 days"), (90, "90 days")] {
                            option value=(d) selected[days == d] { (l) }
                        }
                    }
                }
                label { "Sort "
                    select name="sort" {
                        option value="time" selected[sort == "time"] { "Newest first" }
                        option value="event" selected[sort == "event"] { "Event name" }
                        option value="person" selected[sort == "person"] { "User" }
                    }
                }
                button type="submit" { "Apply" }
            }
            section.card.wide {
                @if rows.is_empty() { p.muted { "Nothing matches. Widen the period or clear a filter." } }
                @else {
                    table.plain.events {
                        thead { tr { th { "When (" (tz_label(&f.tz)) ")" } th { "Event" } th { "User" } th { "Role" } th { "Page" } th { "Feature" } th { "Release" } th { span.sr-only { "Replay" } } } }
                        tbody { @for r in rows {
                            @let fam = openlens_engine::family(&r.event);
                            tr {
                                td.muted.mono { (local_time(&r.timestamp, &f.tz, true)) }
                                td { span class={ "chip fam " (fam) } title=(family_label(fam)) { (icon(family_icon(fam))) span.sr-only { (family_label(fam)) ": " } (r.event) } }
                                td.mono { a href={ "/p/" (p.id) "/people/" (url_enc(&r.distinct_id)) } title="Profile" { (r.distinct_id) } }
                                td { (r.role.as_deref().unwrap_or("—")) }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.feature.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                                td { @if let Some(s) = &r.session_id {
                                    a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (r.timestamp) } title="Replay this moment" aria-label="Replay this moment" { (icon("play")) }
                                } }
                            }
                        } }
                    }
                    @if let Some(n) = next { p { a.button href=(n) { "Older / next 100" (icon("right")) } } }
                }
            }
        },
    )
}

pub(crate) fn family_icon(fam: &str) -> &'static str {
    match fam {
        "page" => "route",
        "click" => "bolt",
        "form" => "task",
        "feature" => "star",
        "identify" => "users",
        "error" => "alert",
        "vital" => "pulse",
        "survey" => "idea",
        "custom" => "spark",
        _ => "events",
    }
}

pub(crate) fn family_label(fam: &str) -> &'static str {
    openlens_engine::FAMILIES
        .iter()
        .find(|(k, _, _)| *k == fam)
        .map_or("Other", |(_, l, _)| l)
}

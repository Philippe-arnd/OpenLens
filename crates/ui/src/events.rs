//! The events page (#544): a refine form, the active filters and the
//! event table with day groups and a pager.

use super::*;
use crate::tables::{EventCols, event_table, pager};

/// The events page's own filters as query pairs, minus `drop` (env, role,
/// release and team come first, as on every page). Never the cursor.
fn events_url(pid: &str, f: &Filters, eq: &openlens_engine::EventQuery, drop: &str) -> String {
    let mut url = detail_url(pid, "/events", f);
    let days = eq.days.map(|d| d.to_string());
    let own = [
        ("family", eq.family.as_deref()),
        ("event", eq.event.as_deref()),
        ("person", eq.person.as_deref()),
        ("path", eq.path.as_deref()),
        ("days", days.as_deref()),
        ("sort", eq.sort.as_deref()),
    ];
    for (k, v) in own {
        if let Some(v) = v.filter(|_| k != drop) {
            url.push('&');
            url.push_str(k);
            url.push('=');
            url.push_str(&url_enc(v));
        }
    }
    url
}

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
    let later_page = eq.before.is_some() || eq.skip > 0;
    let first = events_url(p.id, f, eq, "");
    let active: Vec<(&str, &str, String)> = [
        (
            "family",
            "Family",
            eq.family.as_deref().map(|k| family_label(k).to_owned()),
        ),
        ("event", "Event", eq.event.clone()),
        ("person", "User", eq.person.clone()),
        ("path", "Page", eq.path.clone()),
    ]
    .into_iter()
    .filter_map(|(k, l, v)| Some((k, l, v?)))
    .collect();
    let mut links: Vec<(&str, &str)> = Vec::new();
    if later_page {
        links.push((first.as_str(), "First page"));
    }
    if let Some(n) = next {
        links.push((n, if sort == "time" { "Older" } else { "Next 100" }));
    }
    let shown = format!(
        "{} {} on this page · 100 per page",
        rows.len(),
        if rows.len() == 1 { "event" } else { "events" }
    );
    layout(
        "Events",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Events" }
                    p.muted {
                        @match sort { "event" => "By event name", "person" => "By user", _ => "Newest first" }
                        ", last " (days) (if days == 1 { " day" } else { " days" }) ", 100 per page. Times in " (f.tz) "."
                    }
                }
                a.button href={ "/p/" (p.id) "/settings/data" } { (icon("down")) "Export" }
            }
            form.card.refine method="get" action={ "/p/" (p.id) "/events" } aria-label="Refine the events" {
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
            div.active-filters {
                span { "Showing" }
                strong { (shown) }
                @if active.is_empty() { span { "· no extra filter" } }
                @else {
                    @for (k, l, v) in &active {
                        a href=(events_url(p.id, f, eq, k)) title="Remove this filter" {
                            (l) ": " span.mono { (v) } span aria-hidden="true" { " ×" } span.sr-only { " (remove)" }
                        }
                    }
                }
            }
            @if rows.is_empty() {
                section.card.empty {
                    (crate::tables::empty_art())
                    div {
                        h2 { "Nothing matches" }
                        p { "Widen the period or clear a filter." }
                        @if !active.is_empty() || later_page { p { a.button href=(events_url(p.id, f, &openlens_engine::EventQuery::default(), "")) { "Clear the filters" } } }
                    }
                }
            } @else {
                (event_table(
                    p.id,
                    "Events",
                    html! {},
                    rows,
                    &f.tz,
                    EventCols { user: true, role: true, groups: sort == "time" },
                    pager(&shown, &links),
                ))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn row(ts: &str) -> EventRow {
        EventRow {
            timestamp: ts.into(),
            event: "$error".into(),
            distinct_id: "u1".into(),
            role: None,
            path: Some("/billing".into()),
            feature: None,
            release: None,
            session_id: None,
        }
    }

    /// A later page links back to the first one with every filter kept
    /// and the cursor dropped.
    #[test]
    fn the_pager_keeps_the_current_filters() {
        let p = ProjectInfo {
            id: "d",
            name: "D",
            key: "k",
        };
        let f = Filters {
            environment: "staging".into(),
            role: Some("admin".into()),
            tz: "UTC".into(),
            ..Filters::default()
        };
        let eq = openlens_engine::EventQuery {
            family: Some("error".into()),
            person: Some("u 1".into()),
            path: Some("/billing".into()),
            days: Some(7),
            before: Some("2026-11-26 09:00:00".into()),
            limit: 100,
            ..Default::default()
        };
        let html = render(events(
            &p,
            &Board::default(),
            &[row("2026-11-26 08:00:00"), row("2026-11-25 08:00:00")],
            &f,
            &eq,
            Some("/p/d/events?next"),
        ));
        let pager = &html[html.find("class=\"pager\"").unwrap()..];
        let first = pager
            .split("href=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        for kept in [
            "env=staging",
            "role=admin",
            "family=error",
            "person=u%201",
            "path=/billing",
            "days=7",
        ] {
            assert!(first.contains(kept), "{kept} in {first}");
        }
        assert!(!first.contains("before="));
        assert!(pager.contains("/p/d/events?next"));
        // Each active filter can be removed on its own, the others kept.
        let chips = &html[html.find("class=\"active-filters\"").unwrap()..];
        assert!(chips.contains("Errors") && chips.contains("u 1"));
        assert!(
            html.contains("class=\"card refine\"") && html.contains("class=\"card table-card\"")
        );
        assert!(!html.contains("style=") && !html.contains("<script"));
    }
}

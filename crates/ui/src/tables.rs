//! The v2 data table (#544): one card with a sticky header, day group rows
//! in the reader's time zone, event pills coloured by family, monospace
//! paths, links on users and features, and a pager. Events, Live, Person,
//! Errors and the Audit log all render through it.

use super::*;

/// A column header: shown, numeric (right-aligned), or read by screen
/// readers only (an icon column).
#[derive(Clone, Copy)]
pub(crate) enum Col<'a> {
    Shown(&'a str),
    Num(&'a str),
    Hidden(&'a str),
}

/// A table card: `section.card.table-card` holding an optional head (a
/// title and a link), the table and an optional footer (the pager).
pub(crate) struct TableCard<'a> {
    /// The section's accessible name.
    pub label: &'a str,
    /// What sits above the table (a title, a link); empty for none.
    pub head: Markup,
    pub cols: &'a [Col<'a>],
    /// The `tbody` rows.
    pub body: Markup,
    /// What sits under the table (the pager); empty for none.
    pub foot: Markup,
}

impl TableCard<'_> {
    pub(crate) fn render(&self) -> Markup {
        html! {
            section.card.table-card aria-label=(self.label) {
                @if !self.head.0.is_empty() { div.table-head { (self.head) } }
                div.scroll { table {
                    thead { tr { @for c in self.cols {
                        @match c {
                            Col::Shown(l) => th scope="col" { (l) },
                            Col::Num(l) => th.num scope="col" { (l) },
                            Col::Hidden(l) => th scope="col" { span.sr-only { (l) } },
                        }
                    } } }
                    tbody { (self.body) }
                } }
                (self.foot)
            }
        }
    }
}

/// The pill classes of an event family: the five families the design
/// colours, and a neutral pill for every other one.
pub(crate) fn ev_class(fam: &str) -> &'static str {
    match fam {
        "page" => "ev ev-view",
        "click" => "ev ev-click",
        "error" => "ev ev-error",
        "vital" => "ev ev-vital",
        "identify" => "ev ev-identify",
        _ => "ev ev-other",
    }
}

/// An event name as a pill: its family's icon and colour, the family
/// named for screen readers and on hover.
pub(crate) fn ev_pill(event: &str) -> Markup {
    let fam = openlens_engine::family(event);
    let label = family_label(fam);
    html! {
        span class=(ev_class(fam)) title=(label) {
            (icon(family_icon(fam))) span.sr-only { (label) ": " } (event)
        }
    }
}

/// A UTC timestamp as its day (`YYYY-MM-DD`) and time of day in the
/// reader's zone (through [`local_time`]).
pub(crate) fn day_and_time(ts: &str, tz: &str) -> (String, String) {
    let l = local_time(ts, tz, true);
    let day = l.get(..10).unwrap_or(&l).to_owned();
    let time = l.get(11..).unwrap_or_default().to_owned();
    (day, time)
}

/// A day as the group rows print it ("26 Nov 2026").
pub(crate) fn day_label(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map_or_else(|_| day.to_owned(), |d| d.format("%-d %b %Y").to_string())
}

/// Rows with a `tr.group` row before each new local day, in the order
/// given (newest first gives the newest day first). `row` gets the time
/// of day to print.
pub(crate) fn day_rows<'a, T: 'a>(
    rows: impl IntoIterator<Item = &'a T>,
    ts: impl Fn(&T) -> &str,
    tz: &str,
    cols: usize,
    row: impl Fn(&T, &str) -> Markup,
) -> Markup {
    let mut out = String::new();
    let mut last: Option<String> = None;
    for r in rows {
        let (day, time) = day_and_time(ts(r), tz);
        if last.as_deref() != Some(day.as_str()) {
            out.push_str(
                &html! { tr.group { td colspan=(cols) { time datetime=(day) { (day_label(&day)) } } } }
                    .into_string(),
            );
            last = Some(day);
        }
        out.push_str(&row(r, &time).into_string());
    }
    PreEscaped(out)
}

/// A user id linking to their profile.
pub(crate) fn user_cell(pid: &str, who: &str) -> Markup {
    html! { a.mono href={ "/p/" (pid) "/people/" (url_enc(who)) } title="Profile" { (who) } }
}

/// A feature key linking to its page, or a dash.
pub(crate) fn feature_cell(pid: &str, key: Option<&str>) -> Markup {
    html! {
        @match key {
            Some(k) => a href={ "/p/" (pid) "/features/" (url_enc(k)) } { (k) },
            None => span.muted { "—" },
        }
    }
}

/// A page path in monospace, or a dash.
pub(crate) fn path_cell(path: Option<&str>) -> Markup {
    html! { td.path { (path.unwrap_or("—")) } }
}

/// The footer of a paged table: what is shown, then the page links.
pub(crate) fn pager(summary: &str, links: &[(&str, &str)]) -> Markup {
    html! {
        div.pager {
            span { (summary) }
            @if !links.is_empty() {
                div.actions { @for (href, label) in links { a.button href=(*href) { (label) } } }
            }
        }
    }
}

/// The drawing of an empty state: a flat, dashed trace ending in a dot.
pub(crate) fn empty_art() -> Markup {
    html! {
        svg.art viewBox="0 0 96 72" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true" focusable="false" {
            path d="M4 44h18l6-14 8 28 8-40 7 26h41" stroke-dasharray="3 4" opacity=".5" {}
            circle cx="88" cy="44" r="4" fill="currentColor" stroke="none" opacity=".35" {}
        }
    }
}

/// Which optional columns an event table shows.
#[derive(Clone, Copy)]
pub(crate) struct EventCols {
    pub user: bool,
    pub role: bool,
    /// Day group rows (off when the rows are not in time order).
    pub groups: bool,
}

/// Event rows as a table card: time, event pill, user, role, page,
/// feature, release and a replay link.
pub(crate) fn event_table(
    pid: &str,
    label: &str,
    head: Markup,
    rows: &[EventRow],
    tz: &str,
    c: EventCols,
    foot: Markup,
) -> Markup {
    let when = format!("Time ({})", tz_label(tz));
    let mut cols = vec![Col::Shown(&when), Col::Shown("Event")];
    if c.user {
        cols.push(Col::Shown("User"));
    }
    if c.role {
        cols.push(Col::Shown("Role"));
    }
    cols.extend([
        Col::Shown("Page"),
        Col::Shown("Feature"),
        Col::Shown("Release"),
        Col::Hidden("Replay"),
    ]);
    let row = |r: &EventRow, time: &str| {
        html! {
            tr {
                td.when.muted { (time) }
                td { (ev_pill(&r.event)) }
                @if c.user { td { (user_cell(pid, &r.distinct_id)) } }
                @if c.role { td.muted { (r.role.as_deref().unwrap_or("—")) } }
                (path_cell(r.path.as_deref()))
                td { (feature_cell(pid, r.feature.as_deref())) }
                td.muted { (r.release.as_deref().unwrap_or("—")) }
                td { @if let Some(s) = &r.session_id {
                    a.icon-link href={ "/p/" (pid) "/replays/" (s) "?at=" (r.timestamp) } title="Replay this moment" aria-label="Replay this moment" { (icon("play")) }
                } }
            }
        }
    };
    let body = if c.groups {
        day_rows(rows, |r| r.timestamp.as_str(), tz, cols.len(), row)
    } else {
        html! { @for r in rows { (row(r, &local_time(&r.timestamp, tz, true))) } }
    };
    TableCard {
        label,
        head,
        cols: &cols,
        body,
        foot,
    }
    .render()
}

/// The icon of an event family (the pills and the Events filter).
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

/// The readable name of an event family.
pub(crate) fn family_label(fam: &str) -> &'static str {
    openlens_engine::FAMILIES
        .iter()
        .find(|(k, _, _)| *k == fam)
        .map_or("Other", |(_, l, _)| l)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn ev(ts: &str, event: &str) -> EventRow {
        EventRow {
            timestamp: ts.into(),
            event: event.into(),
            distinct_id: "u1".into(),
            role: None,
            path: Some("/home".into()),
            feature: Some("billing".into()),
            release: None,
            session_id: None,
        }
    }

    #[test]
    fn a_table_over_two_days_has_one_group_row_per_day_newest_first() {
        let rows = [
            ev("2026-11-26 09:00:00", "$pageview"),
            ev("2026-11-26 08:00:00", "$click"),
            ev("2026-11-25 20:00:00", "$error"),
        ];
        let cols = EventCols {
            user: true,
            role: false,
            groups: true,
        };
        let html = event_table("d", "Events", html! {}, &rows, "UTC", cols, html! {}).into_string();
        assert_eq!(html.matches("class=\"group\"").count(), 2, "{html}");
        let (a, b) = (
            html.find("2026-11-26").unwrap(),
            html.find("2026-11-25").unwrap(),
        );
        assert!(a < b, "the newer day comes first, as the rows do");
        // The day is the reader's: 23:30 UTC on the 25th is the 26th in Paris.
        let late = [
            ev("2026-11-25 23:30:00", "$click"),
            ev("2026-11-25 22:30:00", "$click"),
        ];
        let utc = event_table("d", "E", html! {}, &late, "UTC", cols, html! {}).into_string();
        let paris =
            event_table("d", "E", html! {}, &late, "Europe/Paris", cols, html! {}).into_string();
        assert_eq!(utc.matches("class=\"group\"").count(), 1);
        assert_eq!(paris.matches("class=\"group\"").count(), 2);
        // Users and features link to their pages; paths are monospace.
        assert!(
            html.contains("href=\"/p/d/people/u1\"")
                && html.contains("href=\"/p/d/features/billing\"")
        );
        assert!(html.contains("class=\"path\""));
    }

    #[test]
    fn every_event_family_maps_to_a_pill_class() {
        let coloured = ["page", "click", "error", "vital", "identify"];
        let mut seen = std::collections::BTreeSet::new();
        for (fam, _, names) in openlens_engine::FAMILIES {
            let class = ev_class(fam);
            assert!(class.starts_with("ev ev-"), "{fam}");
            seen.insert(class);
            for n in *names {
                assert!(ev_pill(n).into_string().contains(class), "{n}");
            }
        }
        // The coloured families each have their own pill; the rest share the neutral one.
        assert_eq!(seen.len(), coloured.len() + 1);
        assert_eq!(ev_class("not-a-family"), ev_class("custom"));
        assert!(
            ev_pill("order_paid")
                .into_string()
                .contains(ev_class("custom"))
        );
    }
}

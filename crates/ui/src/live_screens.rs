//! Quality and Live screens in the v2 parts (#549): Uptime's status line,
//! its setup step and incidents table, the Timeline grouped by day with
//! typed markers, and Ask's empty state. Releases and Replays render in
//! their own modules with the same parts.

use super::*;
use crate::tables::{Col, TableCard, day_label, empty_art};

/// Uptime's status line: a coloured dot, the state in words (the colour
/// is never the only signal) and since when.
pub(crate) fn uptime_status(state: &str, since: Option<&str>) -> Markup {
    let (tone, word) = match state {
        "up" => ("good", "Up"),
        "down" => ("bad", "Down"),
        "maintenance" => ("warn", "Under maintenance"),
        "off" => ("muted", "Not checked"),
        _ => ("muted", "Waiting for the first check"),
    };
    html! {
        p class={ "state-line " (tone) } {
            span.state-dot aria-hidden="true" {}
            strong { (word) }
            @if let Some(s) = since { span.since { "since " (s) } }
        }
    }
}

/// "Set the app address" as a setup step, when nothing is probed.
pub(crate) fn app_address_step(pid: &str) -> Markup {
    html! {
        section.card.setup aria-labelledby="probe-h" {
            div.setup-head { div.section-head {
                h2 #probe-h { "Check that the app is up" }
                p { "One step" }
            } }
            div.steps {
                div.step.todo {
                    span.state { "1" }
                    div {
                        b { "Set the app address" }
                        p { "OpenLens then checks it every minute. Without it, only the traffic tells (after four weeks of history)." }
                    }
                    a.btn.primary.sm href={ "/p/" (pid) "/settings#project" } { "Set it" }
                }
            }
        }
    }
}

/// What an incident rule is, in words.
fn incident_kind(rule: &str) -> &'static str {
    match rule {
        "traffic" => "traffic stopped",
        "down" => "down",
        r if r.starts_with("budget_") || (r.starts_with("env:") && r.contains(":budget_")) => {
            "error budget"
        }
        r if r.starts_with("error:") => "new error",
        r if r.starts_with("env:") => "environment check",
        _ => "regression",
    }
}

/// One incident: (rule, start, end, length in minutes, cause).
pub(crate) type Incident = (String, String, Option<String>, i64, String);

/// The incidents as a data table: an ongoing one is a red tag with an
/// alert icon and says "ongoing"; the pager pages back in time.
pub(crate) fn incidents_table(
    pid: &str,
    incidents: &[Incident],
    older: Option<i64>,
    paged: bool,
) -> Markup {
    let newest = format!("/p/{pid}/uptime");
    let before = older.map(|b| format!("/p/{pid}/uptime?before={b}"));
    let mut links: Vec<(&str, &str)> = Vec::new();
    if paged {
        links.push((newest.as_str(), "Newest incidents"));
    }
    if let Some(b) = &before {
        links.push((b.as_str(), "Older incidents"));
    }
    let summary = match incidents.len() {
        0 => String::new(),
        1 => "1 incident, newest first".to_owned(),
        n => format!("{n} incidents, newest first"),
    };
    TableCard {
        label: "Incidents and alerts",
        head: html! { h2 { (icon("alert")) "Incidents and alerts" } },
        cols: &[
            Col::Shown("What"),
            Col::Shown("Started"),
            Col::Shown("Ended"),
            Col::Num("Length"),
            Col::Shown("Cause"),
        ],
        body: html! {
            @if incidents.is_empty() {
                tr { td.muted colspan="5" { @if paged { "No older incident." } @else { "None." } } }
            }
            @for (rule, start, end, mins, cause) in incidents {
                tr {
                    td { @if end.is_none() {
                        span.tag.tone-bad { (icon("alert")) (incident_kind(rule)) }
                    } @else {
                        span.tag { (incident_kind(rule)) }
                    } }
                    td.when.muted { (start) }
                    td.when { @match end { Some(e) => span.muted { (e) }, None => strong { "ongoing" } } }
                    td.num { (mins) " min" }
                    td.small { (cause) }
                }
            }
        },
        foot: if summary.is_empty() && links.is_empty() {
            html! {}
        } else {
            crate::tables::pager(&summary, &links)
        },
    }
    .render()
}

/// A timeline entry's type: (words, tag tone, icon).
fn mark_type(m: &Mark) -> (&'static str, &'static str, &'static str) {
    let t = m.text.as_str();
    match m.kind.as_str() {
        "release" => ("release", "tag tone-accent", "release"),
        "alert" if t.starts_with("Regression") => ("regression", "tag tone-bad", "down"),
        "alert" if t.starts_with("New error") => ("new error", "tag tone-bad", "alert"),
        "alert" => ("incident", "tag tone-bad", "alert"),
        "change" if t.starts_with("Feature map") => ("map change", "tag tone-info", "route"),
        "change" if t.starts_with("Flag ") || t.starts_with("A flag") => {
            ("flag change", "tag tone-info", "flag")
        }
        "change" if t.starts_with("Survey ") || t.starts_with("A survey") => {
            ("survey change", "tag tone-info", "idea")
        }
        "change" if t.starts_with("A goal") => ("goal", "tag tone-good", "task"),
        "change" => ("change", "tag tone-info", "flag"),
        _ => ("note", "tag", "idea"),
    }
}

/// The timeline as a data table: a group row per day, newest day first,
/// each entry with its typed marker, who, and Delete on the notes the
/// viewer may delete.
pub(crate) fn timeline_table(pid: &str, marks: &[Mark], me: &str, admin: bool) -> Markup {
    if marks.is_empty() {
        return html! {
            section.card.empty aria-labelledby="tl-empty-h" {
                (empty_art())
                div {
                    h2 #tl-empty-h { "Nothing on the timeline yet" }
                    p { "Releases, flag and survey changes, feature map pushes, incidents and notes from the last 90 days show up here, grouped by day." }
                }
            }
        };
    }
    let mut sorted: Vec<&Mark> = marks.iter().collect();
    // Stable: the order of a day's entries is kept.
    sorted.sort_by(|a, b| b.day.cmp(&a.day));
    let body = html! {
        @for (i, m) in sorted.iter().enumerate() {
            @if i == 0 || sorted.get(i - 1).is_some_and(|p| p.day != m.day) {
                tr.group { td colspan="4" { time datetime=(m.day) { (day_label(&m.day)) } } }
            }
            @let (word, class, glyph) = mark_type(m);
            tr {
                td { span class=(class) { (icon(glyph)) (word) } }
                td { (m.text) }
                td.muted { (m.by.as_deref().unwrap_or("—")) }
                td { @if let (Some(id), Some(a)) = (m.id, &m.author_id) {
                    @if admin || a == me {
                        form.inline method="post" action={ "/p/" (pid) "/timeline/" (id) "/delete" } {
                            button.ghost type="submit" aria-label="Delete this note" { "Delete" }
                        }
                    }
                } }
            }
        }
    };
    TableCard {
        label: "What changed",
        head: html! {},
        cols: &[
            Col::Shown("Type"),
            Col::Shown("What"),
            Col::Shown("By"),
            Col::Hidden("Actions"),
        ],
        body,
        foot: crate::tables::pager(
            &format!(
                "{} entr{}, newest day first",
                marks.len(),
                if marks.len() == 1 { "y" } else { "ies" }
            ),
            &[],
        ),
    }
    .render()
}

/// Ask when no model is set up: one empty state saying what Ask does,
/// with "Set up AI" for admins.
pub(crate) fn ask_off(admin: bool) -> Markup {
    html! {
        section.card.empty aria-labelledby="ask-off-h" {
            (empty_art())
            div {
                h2 #ask-off-h { "Ask is off" }
                p { "Ask answers questions about this project in your words, with the numbers and the pages behind each answer. It runs on a model you choose: an Anthropic key, or any OpenAI-compatible server (a LiteLLM proxy, OpenRouter, a local Ollama: then no data leaves your network)." }
                @if admin {
                    div.actions { a.btn.primary href="/settings/ai" { "Set up AI" } }
                } @else {
                    p { "An admin turns it on in Settings → AI." }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn mark(day: &str, kind: &str, text: &str) -> Mark {
        Mark {
            day: day.into(),
            kind: kind.into(),
            text: text.into(),
            id: None,
            by: None,
            author_id: None,
        }
    }

    #[test]
    fn the_uptime_line_says_down_only_when_the_app_is_down() {
        let down = uptime_status("down", Some("2026-11-26 08:00")).into_string();
        assert!(down.contains("Down") && !down.contains("Up<"));
        assert!(down.contains("2026-11-26 08:00"), "since when");
        for other in ["up", "maintenance", "off", "unknown"] {
            let h = uptime_status(other, None).into_string();
            assert!(!h.contains("Down"), "{other}: {h}");
            assert_ne!(h, down);
        }
        assert!(uptime_status("up", None).into_string().contains("Up"));
    }

    #[test]
    fn the_timeline_has_one_group_row_per_day_newest_first() {
        // Given out of order: the older day first.
        let marks = [
            mark("2026-11-24", "note", "Pricing page shipped"),
            mark("2026-11-26", "release", "Release v2 first seen"),
            mark("2026-11-24", "alert", "Regression: bounce up"),
        ];
        let html = timeline_table("d", &marks, "me", false).into_string();
        assert_eq!(html.matches("class=\"group\"").count(), 2, "{html}");
        let (new, old) = (
            html.find("datetime=\"2026-11-26\"").unwrap(),
            html.find("datetime=\"2026-11-24\"").unwrap(),
        );
        assert!(new < old, "the newer day comes first");
        // Each entry is typed in words, not only by colour.
        assert!(html.contains("release") && html.contains("regression") && html.contains("note"));
        assert!(!html.contains("style=") && !html.contains("<script"));
    }

    #[test]
    fn ask_off_is_one_empty_state_with_set_up_for_admins_only() {
        let p = ProjectInfo {
            id: "d",
            name: "D",
            key: "k",
        };
        let page = |admin: bool| {
            render(crate::ask(&AskView {
                p: &p,
                on: None,
                turns: &[],
                from: None,
                error: None,
                used: 0,
                budget: 1,
                admin,
            }))
        };
        let (admin, viewer) = (page(true), page(false));
        for h in [&admin, &viewer] {
            assert_eq!(h.matches("class=\"card empty\"").count(), 1);
            assert!(!h.contains("textarea"), "no question field while off");
            assert!(!h.contains("style=") && !h.contains("<script"));
        }
        assert!(admin.contains("Set up AI"));
        assert!(!viewer.contains("Set up AI"));
    }
}

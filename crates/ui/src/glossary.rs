//! What each KPI counts and over which window (#405): one table, shown on
//! an ⓘ next to every tile's label (hover, keyboard focus, tap), with no
//! script. The wording follows `docs/kpis.md`, the contract.

use maud::{Markup, html};

/// (tile label, definition). Labels with a variable part (the currency of
/// *Revenue*) are matched on the text before ` (`.
pub(crate) const KPI: &[(&str, &str)] = &[
    (
        "Daily active users",
        "Distinct identified users with at least one event today, in your time zone. Robots and, unless ticked, the team are left out.",
    ),
    (
        "Stickiness",
        "Daily active users ÷ monthly active users (last 30 days). Higher means people come back more often.",
    ),
    (
        "Avg session",
        "Mean session length over the last 7 days. A session ends after 30 minutes without an event. Bounce = sessions with a single page view.",
    ),
    (
        "Tasks per user per day",
        "Feature uses per active user per day, over the last 7 days.",
    ),
    (
        "Sessions with errors",
        "Share of the last 7 days' sessions with at least one captured error.",
    ),
    (
        "Changes that moved their metric",
        "Releases and changes whose target metric moved the expected way, measured 14 days after they shipped.",
    ),
    (
        "Active now",
        "Distinct users with an event in the last 5 minutes.",
    ),
    (
        "Last 30 minutes",
        "Distinct users with an event in the last 30 minutes.",
    ),
    (
        "Errors",
        "Errors captured in the last 30 minutes, all users.",
    ),
    (
        "Last 24 hours",
        "Share of uptime checks of the app address that succeeded in the last 24 hours.",
    ),
    (
        "Last 7 days",
        "Share of uptime checks of the app address that succeeded in the last 7 days.",
    ),
    (
        "Response time",
        "Median (p50) response time of the uptime checks, last 24 hours.",
    ),
    (
        "Churn rate",
        "Users active at the start of the window who have had no event since, ÷ users active at the start.",
    ),
    (
        "Net change",
        "New users + users back after a gap − users gone, over the window.",
    ),
    (
        "At risk now",
        "Users quiet for over half the churn window and less active than their own usual rhythm.",
    ),
    (
        "Median lifetime",
        "Time after which half the users of a cohort have churned (Kaplan–Meier median).",
    ),
    (
        "Revenue",
        "Sum of the purchase events' amounts in the period, in the project's currency; payers are distinct users who paid.",
    ),
    (
        "Users",
        "Distinct people who used this feature in the last 30 days.",
    ),
    ("Uses", "Times the feature was used in the last 30 days."),
    (
        "Succeeded",
        "Distinct people who reached its declared success event in the last 30 days.",
    ),
];

/// Metric column headers (#405, part 2): (header, definition). Every
/// numeric header (`th.num`) goes through [`th_num`]; the test below fails
/// on a literal one left plain, except the counts in [`NOT_METRIC`].
pub(crate) const COLUMNS: &[(&str, &str)] = &[
    (
        "Users",
        "Distinct people in this row, over the page's window. Robots and, unless ticked, the team are left out.",
    ),
    (
        "Sessions",
        "Visits in this row. A session ends after 30 minutes without an event.",
    ),
    (
        "Tasks",
        "Feature uses (events tied to a feature of the map) in this row.",
    ),
    ("Errors", "Captured errors in this row."),
    (
        "Events",
        "Every event received for this row, page views and clicks included.",
    ),
    (
        "Now",
        "The KPI's current value, over the window the goal is measured on.",
    ),
    (
        "Last before leaving",
        "Sessions whose last interaction was this field: where people gave up.",
    ),
    (
        "Came back to it",
        "Sessions that left this field and returned to it later: a sign of hesitation.",
    ),
    (
        "Time spent",
        "Median time spent in this field, from focus to blur.",
    ),
    ("Features", "Features declared in this area of the map."),
    (
        "Uses (30 d)",
        "Times the features were used in the last 30 days.",
    ),
    (
        "Uses (7 d)",
        "Times the feature was used in the last 7 days.",
    ),
    (
        "Went on",
        "Users of the first feature who used the second one afterwards.",
    ),
    (
        "Skipped the first",
        "Users of the second feature who never used the first one.",
    ),
    ("Share", "This row's part of the total."),
    (
        "Length",
        "How long it lasted, from the first failed check to the first good one.",
    ),
    (
        "Reached value",
        "Share of these users who reached a success event (a value moment) in the window.",
    ),
    (
        "Came back",
        "Share of these users active again in a later week.",
    ),
    (
        "Churned",
        "Users with no event for longer than the churn window, ÷ users in this row.",
    ),
    (
        "Stayed",
        "Users still active after this day, ÷ users in this row.",
    ),
    (
        "Tried it",
        "Users who used this feature at least once in the window.",
    ),
    (
        "Did not",
        "Users active in the window who never used this feature.",
    ),
    (
        "Still here at 30 days",
        "Share of this role's users still active 30 days after their first visit.",
    ),
    (
        "Churned users",
        "Churned users whose last session ended here.",
    ),
    (
        "Users who stayed",
        "Users still active who also went through this place.",
    ),
    ("Quiet for", "Time since this user's last event."),
    (
        "Active days, last window",
        "Distinct days with an event in the latest churn window.",
    ),
    (
        "Window before",
        "Distinct days with an event in the window before it: the drop is the risk.",
    ),
    ("Nudged", "Users who saw this flag's nudge (flag on)."),
    (
        "Active, last 7 days",
        "Share of these users with an event in the last 7 days.",
    ),
    (
        "Held out",
        "Users kept without the nudge (flag off), to compare with.",
    ),
    ("This week", "The last 7 days."),
    ("Last week", "The 7 days before."),
    ("Change", "This week compared with last week, in percent."),
    (
        "Sessions with errors",
        "Share of these sessions with at least one captured error.",
    ),
    (
        "Times",
        "Occurrences: how many times it happened in the window.",
    ),
    (
        "Active time",
        "Time with user input in the recording; idle stretches of over 2 minutes are left out. The full length is below.",
    ),
    (
        "Users with / without",
        "Users who had the flag on, and users who did not, in the last 30 days.",
    ),
    ("Avg session", "Mean session length."),
    (
        "Tasks / session",
        "Feature uses per session: higher means more gets done in a visit.",
    ),
    ("Bounce", "Share of sessions with at most one interaction."),
    (
        "% users",
        "Share of active users who used it (last 30 days).",
    ),
    ("Uses / user", "Average uses per user."),
    (
        "Days / user",
        "Average distinct days each user came back to it.",
    ),
    (
        "Completion",
        "Users who reached its success event, ÷ its users.",
    ),
    (
        "Task time",
        "Median time from the first use to the success event (p90 below).",
    ),
    (
        "Succeeded",
        "Users who reached its declared success event, ÷ its users.",
    ),
    (
        "Users with it on",
        "Users of this feature who had the flag on while using it, last 30 days.",
    ),
    ("Uses", "Times the feature was used."),
    (
        "Confidence",
        "Whether the difference can be trusted yet: a two-sided test at p < 0.05, with at least 30 sessions on each side.",
    ),
    (
        "vs baseline",
        "The difference with the baseline variant, and whether it is significant yet (p < 0.05, at least 30 users each).",
    ),
];

/// Numeric headers that are plain facts, not metrics (no definition).
#[cfg(test)]
pub(crate) const NOT_METRIC: &[&str] = &["Size", "Files", "Took", "Comments", "Step"];

thread_local! {
    /// Distinct ids for the tooltips of one page (a header can repeat).
    static SEQ: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The definition of a column header, if there is one.
pub(crate) fn column(label: &str) -> Option<&'static str> {
    COLUMNS.iter().find(|(k, _)| *k == label).map(|(_, d)| *d)
}

fn col_info(label: &str) -> Markup {
    let Some(def) = column(label) else {
        return html! {};
    };
    let id = format!(
        "col-{}",
        SEQ.with(|c| {
            c.set(c.get().wrapping_add(1));
            c.get()
        })
    );
    html! {
        span.kpi-info tabindex="0" aria-describedby=(id) aria-label={ "What is " (label) "?" } {
            "ⓘ"
            span.kpi-def role="tooltip" id=(id) { (def) }
        }
    }
}

/// A numeric (metric) column header with its definition on an ⓘ.
pub(crate) fn th_num(label: &str) -> Markup {
    html! { th.num scope="col" { (label) " " (col_info(label)) } }
}

/// A text column header that holds a computed judgement (significance).
pub(crate) fn th_def(label: &str) -> Markup {
    html! { th scope="col" { (label) " " (col_info(label)) } }
}

/// The definition of a tile label, if there is one.
pub(crate) fn definition(label: &str) -> Option<&'static str> {
    let key = label.split(" (").next().unwrap_or(label);
    KPI.iter().find(|(k, _)| *k == key).map(|(_, d)| *d)
}

/// The ⓘ that shows a definition on hover, focus or tap (a focusable
/// element; `aria-describedby` reads it to screen readers).
pub(crate) fn info(label: &str) -> Markup {
    let Some(def) = definition(label) else {
        return html! {};
    };
    let id: String = format!(
        "def-{}",
        label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            })
            .collect::<String>()
    );
    html! {
        span.kpi-info tabindex="0" aria-describedby=(id) aria-label={ "What is " (label) "?" } {
            "ⓘ"
            span.kpi-def role="tooltip" id=(id) { (def) }
        }
    }
}

/// The ⓘ of a Web Vital (#408): what the visitor feels and the
/// thresholds, from the engine's knowledge table.
pub(crate) fn vital_info(name: &str) -> Markup {
    let Some(g) = openlens_engine::vitals_guide(name) else {
        return html! {};
    };
    let id = format!("def-vital-{}", name.to_ascii_lowercase());
    let show = |v| openlens_engine::vitals::show(name, v);
    html! {
        span.kpi-info tabindex="0" aria-describedby=(id) aria-label={ "What is " (name) "?" } {
            "ⓘ"
            span.kpi-def role="tooltip" id=(id) {
                strong { (g.title) } ". " (g.feels) " Good: " (show(g.good)) " or less; poor: above " (show(g.poor)) "."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    /// Every `tile(` call's label (third argument, a literal) has an entry.
    #[test]
    fn every_tile_has_a_definition() {
        let sources = [
            include_str!("board.rs"),
            include_str!("audience.rs"),
            include_str!("settings.rs"),
            include_str!("manage.rs"),
            include_str!("goals.rs"),
            include_str!("team.rs"),
            include_str!("features.rs"),
            include_str!("explore.rs"),
            include_str!("feature_page.rs"),
        ];
        let mut seen = 0;
        for src in sources {
            // tile_to(&detail_url(.., "/path", f), "accent", "glyph", "Label", …)
            for call in src.split("tile_to(").skip(1) {
                let lits: Vec<&str> = call
                    .split('"')
                    .skip(1)
                    .step_by(2)
                    .filter(|l| !l.starts_with('/') && !l.starts_with('#'))
                    .take(3)
                    .collect();
                if let Some(label) = lits.get(2) {
                    seen += 1;
                    assert!(
                        super::definition(label).is_some(),
                        "tile {label:?} has no glossary entry"
                    );
                }
            }
            for call in src.split("tile(").skip(1) {
                // tile("accent", "glyph", "Label" | &format!("Label (…)", …)
                let args: Vec<&str> = call.splitn(4, ',').collect();
                if args.len() < 3 {
                    continue;
                }
                let raw = args[2].trim();
                let Some(start) = raw.find('"') else { continue };
                let label = raw[start + 1..].split('"').next().unwrap();
                seen += 1;
                assert!(
                    super::definition(label).is_some(),
                    "tile {label:?} has no glossary entry"
                );
            }
        }
        assert!(seen >= 17, "found {seen} tiles");
    }

    /// No literal numeric header is left without a definition, and every
    /// `th_num("…")` / `th_def("…")` label has one.
    #[test]
    fn every_metric_column_has_a_definition() {
        let sources = [
            include_str!("board.rs"),
            include_str!("audience.rs"),
            include_str!("settings.rs"),
            include_str!("manage.rs"),
            include_str!("goals.rs"),
            include_str!("team.rs"),
            include_str!("features.rs"),
            include_str!("explore.rs"),
            include_str!("feature_page.rs"),
        ];
        let mut seen = 0;
        for src in sources {
            for rest in src.split("th.num { \"").skip(1) {
                let label = rest.split('"').next().unwrap();
                assert!(
                    super::NOT_METRIC.contains(&label),
                    "numeric header {label:?}: use th_num, or list it in NOT_METRIC"
                );
            }
            assert!(
                !src.contains("th.num title="),
                "a title= header: use th_num"
            );
            for f in ["th_num(\"", "th_def(\""] {
                for rest in src.split(f).skip(1) {
                    let label = rest.split('"').next().unwrap();
                    seen += 1;
                    assert!(
                        super::column(label).is_some(),
                        "header {label:?} has no definition"
                    );
                }
            }
        }
        assert!(seen >= 70, "found {seen} metric headers");
    }

    #[test]
    fn column_tooltips_have_distinct_ids() {
        let a = super::th_num("Users").into_string();
        let b = super::th_num("Users").into_string();
        assert!(a.contains("aria-describedby=\"col-") && a.contains("role=\"tooltip\""));
        assert_ne!(a, b, "a repeated header gets its own id");
        assert!(
            super::th_def("Confidence")
                .into_string()
                .contains("p &lt; 0.05")
        );
    }

    /// A vital's ⓘ reads the engine's table: its meaning and thresholds.
    #[test]
    fn vital_info_shows_the_table_entry() {
        let g = openlens_engine::vitals_guide("INP").unwrap();
        let html = super::vital_info("INP").into_string();
        assert!(html.contains(g.title) && html.contains("200 ms") && html.contains("500 ms"));
        assert!(html.contains("id=\"def-vital-inp\""));
        assert!(super::vital_info("XYZ").into_string().is_empty());
    }

    #[test]
    fn the_info_mark_is_focusable_and_described() {
        let html = super::info("Stickiness").into_string();
        assert!(html.contains("tabindex=\"0\""));
        assert!(html.contains("aria-describedby=\"def-stickiness\""));
        assert!(html.contains("id=\"def-stickiness\""));
        assert!(
            super::info("Revenue (EUR)")
                .into_string()
                .contains("purchase")
        );
        assert!(super::info("Nothing").into_string().is_empty());
    }
}

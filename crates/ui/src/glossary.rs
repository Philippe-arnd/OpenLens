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

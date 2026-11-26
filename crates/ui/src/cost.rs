//! What a board window costs (#514, D-061): the line under the window a
//! date range or past release read, and the page that asks before one
//! reads more than 30 days of events. No JavaScript: "Run it" is a link
//! to the same board with `confirm=1`.

use super::*;
use crate::settings::bytes;
use openlens_engine::Estimate;

/// A rough time: "under a second", "about 4 s", "about 3 min".
fn rough(secs: f64) -> String {
    if secs < 1.0 {
        "under a second".to_owned()
    } else if secs < 90.0 {
        format!("about {:.0} s", secs.ceil())
    } else {
        format!("about {:.0} min", (secs / 60.0).ceil())
    }
}

/// Days, said: "1 day", "12 days".
fn days(n: usize) -> String {
    if n == 1 {
        "1 day".to_owned()
    } else {
        format!("{n} days")
    }
}

/// What the window read, in a sentence.
pub(crate) fn cost_line(e: &Estimate) -> Markup {
    html! {
        p.muted.small {
            "Read " (days(e.rollup_days)) " from the daily rollups and " (days(e.raw_days))
            " from the events (" (bytes(e.bytes)) ")."
        }
    }
}

/// The page shown instead of the board when the window `e` must be
/// confirmed: the estimate, a "Run it" link to `run` and a way back.
pub fn board_confirm(p: &ProjectInfo<'_>, e: &Estimate, run: &str) -> Markup {
    layout(
        "Focus board",
        Some(p),
        html! {
            div.heading {
                h1 { "Focus board" }
                p.muted { (e.label) ", against the same length before." }
            }
            section.card aria-label="Cost of this period" {
                h2 { "This period reads a lot of events" }
                p {
                    (days(e.raw_days)) " of events to read (" (bytes(e.bytes)) ", "
                    (compact(i64::try_from(e.rows).unwrap_or(i64::MAX))) " events), and "
                    (days(e.rollup_days)) " from the daily rollups. Estimated time: "
                    strong { (rough(e.secs)) } " (rough)."
                }
                p.muted.small {
                    "More than " (openlens_engine::board_cost::CONFIRM_DAYS)
                    " days of events slows the board down while it runs. The time is the bytes at the read speed measured for the board, so the real one may differ."
                }
                p {
                    a.button href=(run) { "Run it" } " "
                    a href={ "/p/" (p.id) } { "Back to the current release" }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_confirm_page_shows_the_estimate_and_runs_with_a_link() {
        let p = ProjectInfo {
            id: "p1",
            name: "P",
            key: "k",
        };
        let e = Estimate {
            label: "From 2026-01-01 to 2026-03-01 (60 days)".into(),
            days: 60,
            raw_days: 118,
            rollup_days: 2,
            read_days: 120,
            bytes: 250_000_000,
            rows: 3_000_000,
            secs: 1.9,
            confirm: true,
            ..Estimate::default()
        };
        let html = board_confirm(&p, &e, "/p/p1?from=2026-01-01&confirm=1").into_string();
        assert!(html.contains("118 days"), "{html}");
        assert!(html.contains("about 2 s"), "{html}");
        assert!(
            html.contains(r#"href="/p/p1?from=2026-01-01&amp;confirm=1""#),
            "{html}"
        );
        assert!(!html.contains("<script"), "no JavaScript");
    }
}

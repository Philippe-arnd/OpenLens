//! What left the board (#620): each block is gone and the page that
//! holds it is one click away. Split from `tests.rs` to keep each file
//! under 1000 lines (#307).

use super::tests::{full, judged, project, used};
use super::*;

/// Each block the board shed (#620): what marked it on the board.
const GONE: [(&str, &str); 7] = [
    ("Web Vitals", r#"id="wv-h""#),
    ("release table", "v2 compared with v1"),
    ("revenue", r#"id="rev-h""#),
    ("role gaps", r#"id="gaps-h""#),
    ("funnels", r#"id="fun-h""#),
    ("flag cohorts", r#"id="flags-h""#),
    ("paths to value", "Paths to value"),
];

/// A board with every source of the blocks it shed (#620).
fn with_everything() -> Board {
    let step = |event: &str, users, conversion| openlens_engine::FunnelStep {
        event: event.into(),
        users,
        conversion,
        median_secs: 30.0,
    };
    Board {
        role_gaps: vec![("Admin ".into(), 3)],
        roles: vec![openlens_engine::RoleUsers {
            role: Some("member".into()),
            users: 9,
        }],
        vitals: vec![openlens_engine::Vital {
            name: "LCP".into(),
            p75: 4800.0,
            rating: "poor".into(),
            samples: 121,
        }],
        funnels: vec![openlens_engine::Funnel {
            feature: "export".into(),
            steps: vec![step("export_open", 10, 1.0), step("export_done", 4, 0.4)],
        }],
        journeys: vec![openlens_engine::Journey {
            success: "export_done".into(),
            feature: "export".into(),
            path: vec!["home".into(), "search".into()],
            users: 5,
            share: 0.5,
        }],
        flag_cohorts: vec![openlens_engine::FlagCohort {
            flag: "new-nav".into(),
            with: openlens_engine::CohortStats::default(),
            without: openlens_engine::CohortStats::default(),
            p_errors: None,
            p_tasks: None,
        }],
        features: vec![used("export", 10, 4)],
        latest_release: Some(judged(8)),
        setup: Some((2, 3)),
        ..full()
    }
}

#[test]
fn the_shed_blocks_are_gone_and_each_page_that_holds_one_is_a_click_away() {
    let b = with_everything();
    let revenue = openlens_engine::Revenue {
        totals: vec![openlens_engine::Money {
            currency: "EUR".into(),
            total: 1234.0,
            payers: 7,
            per_active_user: 8.5,
            ..openlens_engine::Money::default()
        }],
        ..openlens_engine::Revenue::default()
    };
    let html = board(
        &project(),
        &b,
        &Filters::default(),
        "h",
        &[],
        Some(&revenue),
        &[],
    )
    .into_string();
    for (what, mark) in GONE {
        assert!(!html.contains(mark), "{what} left the board: {html}");
    }
    assert!(!html.contains("Who uses the app"), "{html}");
    assert!(!html.contains("new-nav</code> ") && !html.contains("Raw role"));
    // Where each went, one click away: Speed → Pages (Web Vitals);
    // the release cell → Releases; Paths to success → Feature flow; a
    // feature's row → its page (funnel, journeys); In flight → Flags;
    // the Setup cell and "Feature map" → the map (role gaps, the matrix);
    // the Income tile → Revenue; Break down → users per role.
    for href in [
        "/p/p1/pages?",
        "/p/p1/releases?",
        "/p/p1/flows?to=success",
        "/p/p1/features/export",
        "/p/p1/flags",
        "/p/p1/settings",
        "/p/p1/features?",
        "/p/p1/revenue?",
        "/p/p1/breakdown?by=role",
    ] {
        assert!(html.contains(&format!("href=\"{href}")), "{href}: {html}");
    }
}

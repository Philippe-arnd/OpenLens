//! The Focus board's Outcomes tiles (#616, #613), split from `tests.rs`
//! to keep each file under 1000 lines (#307).

use super::tests::{between, full, page, project};
use super::*;

/// One tile of the outcomes (#616), found by its definition's label.
fn outcome<'a>(html: &'a str, label: &str) -> &'a str {
    between(html, r#"id="out-h""#, "</section>")
        .split(r#"<div class="kpi-slot">"#)
        .find(|t| t.contains(&format!("Definition: {label}\"")))
        .unwrap_or_else(|| panic!("no {label} tile: {html}"))
}

/// #613: on a window, Reach's headline is the window's active users, not
/// today's partial day, which comes second and says so.
#[test]
fn reach_counts_the_window_and_today_comes_second() {
    let mut b = full();
    b.users.dau = 3;
    b.users.known = 300;
    b.roles = vec![
        openlens_engine::RoleUsers {
            role: Some("member".into()),
            users: 115,
        },
        openlens_engine::RoleUsers {
            role: None,
            users: 7,
        },
    ];
    let window = b.window.as_ref().unwrap().current.users;
    assert_ne!(
        window, b.users.dau,
        "the two must differ to tell them apart"
    );
    let html = page(&b);
    let reach = outcome(&html, "Active users");
    let headline = between(reach, r#"class="kpi-value""#, "</div>");
    assert!(
        headline.ends_with(&format!(">{}", compact(window))),
        "{headline}"
    );
    assert!(
        !headline.ends_with(&format!(">{}", compact(b.users.dau))),
        "{headline}"
    );
    assert!(
        reach.contains(&format!("{} today, so far", compact(b.users.dau))),
        "{reach}"
    );
    assert!(
        reach.contains(&format!("of {} known", compact(b.users.known))),
        "{reach}"
    );
    // The role split names the roles, not the users without one.
    assert!(
        reach.contains("member: 115") && !reach.contains(": 7"),
        "{reach}"
    );
    assert!(
        reach.contains("Reach"),
        "the question as its eyebrow: {reach}"
    );
}

/// A tile that compares with nothing shows no arrow (#616): Retention
/// says its numbers, Speed its grade against the public threshold.
#[test]
fn a_tile_without_a_comparison_shows_no_delta() {
    let mut b = full();
    let churn = openlens_engine::ChurnLine {
        window: 14,
        active_at_start: 185,
        churned_recent: 41,
        median_days: Some(19),
    };
    b.churn = Some(churn.clone());
    b.vitals = vec![openlens_engine::Vital {
        name: "LCP".into(),
        p75: 4800.0,
        rating: "poor".into(),
        samples: 121,
    }];
    let html = page(&b);
    let retention = outcome(&html, "Churn rate");
    assert!(!retention.contains(r#"class="delta "#), "{retention}");
    assert!(
        retention.contains(&pct(churn.rate().unwrap())),
        "{retention}"
    );
    assert!(
        retention.contains(&format!(
            "half are gone by day {}",
            churn.median_days.unwrap()
        )),
        "{retention}"
    );
    let speed = outcome(&html, "Largest paint p75");
    let good = openlens_engine::vitals_guide("LCP").unwrap().good;
    assert!(!speed.contains(r#"class="delta "#), "{speed}");
    assert!(
        speed.contains(&format!("good ≤ {} s", good / 1000.0)),
        "{speed}"
    );
    assert!(speed.contains(r#"class="status bad""#), "graded: {speed}");
    // With nothing measured, a tile says so instead of a zero.
    b.churn = None;
    b.vitals.clear();
    let html = page(&b);
    for label in ["Churn rate", "Largest paint p75"] {
        let tile = outcome(&html, label);
        assert!(!tile.contains(r#"class="delta "#), "{tile}");
        assert!(tile.contains("—") && !tile.contains("0.0%"), "{tile}");
    }
}

/// Revenue is a sixth tile, and only when there is revenue (#616).
#[test]
fn revenue_gets_a_tile_only_when_there_is_revenue() {
    let b = full();
    let without = page(&b);
    let r = openlens_engine::Revenue {
        totals: vec![openlens_engine::Money {
            currency: "EUR".into(),
            total: 1200.0,
            orders: 30,
            payers: 12,
            per_active_user: 8.3,
        }],
        ..openlens_engine::Revenue::default()
    };
    let with = board(&project(), &b, &Filters::default(), "h", &[], Some(&r), &[]).into_string();
    let slots = |html: &str| {
        between(html, r#"id="out-h""#, "</section>")
            .matches(r#"class="kpi-slot""#)
            .count()
    };
    assert_eq!(slots(&with), slots(&without) + 1);
    assert!(!between(&without, r#"id="out-h""#, "</section>").contains("/revenue"));
    let tile = outcome(&with, "Revenue");
    assert!(tile.contains("/p/p1/revenue?"), "{tile}");
    assert!(tile.contains(&money(1200.0, "EUR")), "{tile}");
}

/// Each outcome opens its page, the board's filters carried along.
#[test]
fn each_outcome_opens_its_page_with_the_board_filters() {
    let f = Filters {
        role: Some("admin".into()),
        ..Filters::default()
    };
    let html = board(&project(), &full(), &f, "h", &[], None, &[]).into_string();
    let tiles = between(&html, r#"id="out-h""#, "</section>");
    for path in ["/audience", "/features", "/churn", "/errors", "/pages"] {
        let href = detail_url("p1", path, &f).replace('&', "&amp;");
        assert!(
            tiles.contains(&format!(r#"href="{href}""#)),
            "{path}: {tiles}"
        );
    }
}

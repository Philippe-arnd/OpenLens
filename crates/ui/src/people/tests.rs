//! The People screens' tests (#550).

#![allow(clippy::unwrap_used)]

use super::*;
use openlens_engine::{AcquisitionRow, ChurnReport};

fn project() -> ProjectInfo<'static> {
    ProjectInfo {
        id: "d",
        name: "D",
        key: "k",
    }
}

fn goal_row(current: f64) -> GoalRow {
    GoalRow {
        goal: openlens_engine::goals::Goal {
            id: format!("g{current}"),
            kpi: "stickiness".into(),
            target: 10.0,
            up: true,
            deadline: "2027-01-01".into(),
            owner: String::new(),
            baseline: 0.0,
            created: "2026-11-01".into(),
            guardrails: Vec::new(),
            releases: None,
        },
        label: "Stickiness".into(),
        share: false,
        current: Some(current),
        projected: None,
        series: Vec::new(),
        status: "on track".into(),
        good: true,
        bad: false,
        days_left: 30,
        state: openlens_engine::goals::Status::OnTrack,
        guards: Vec::new(),
        moves: Vec::new(),
        marks: Vec::new(),
    }
}

/// The widths of the progress bars of a page, in order.
fn bar_widths(html: &str) -> Vec<f64> {
    html.split("<rect class=\"bar\" width=\"")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].parse::<f64>().unwrap())
        .collect()
}

#[test]
fn goal_progress_bars_grow_with_progress_and_stay_inside_their_track() {
    // Below the baseline, on the way, at the target, past it.
    let rows: Vec<GoalRow> = [-3.0, 2.0, 5.0, 10.0, 15.0]
        .into_iter()
        .map(goal_row)
        .collect();
    let html = render(goals(&project(), &rows, &[], &[], false, "2027-01-01"));
    let track = html.matches("<rect class=\"track\" width=\"100\"").count();
    let widths = bar_widths(&html);
    assert_eq!(widths.len(), rows.len(), "one bar per goal: {html}");
    assert_eq!(track, rows.len(), "each bar has its track");
    for w in widths.windows(2) {
        assert!(
            w[1] >= w[0],
            "a bar never shrinks as progress grows: {widths:?}"
        );
    }
    assert!(widths[2] > widths[1], "more progress, a longer bar");
    assert!(
        widths.iter().all(|w| (0.0..=100.0).contains(w)),
        "{widths:?}"
    );
    assert!(
        widths[0] < widths[3],
        "behind the baseline is shorter than reached"
    );
}

fn churn_page(last_place: Vec<(String, i64, f64, f64)>) -> String {
    let c = ChurnReport {
        window: 30,
        last_place,
        ..ChurnReport::default()
    };
    let x = ChurnExtras {
        admin: false,
        churn_event: None,
        exit: Vec::new(),
        audiences: Vec::new(),
    };
    render(churn(
        &project(),
        &Board::default(),
        &Filters::default(),
        &c,
        &x,
    ))
}

#[test]
fn churn_leads_with_a_finding_only_when_a_last_place_stands_out() {
    let usual = churn_page(vec![("home".into(), 20, 0.3, 0.28)]);
    assert!(
        !usual.contains("class=\"card lead\""),
        "no finding, no lead card"
    );
    let none = churn_page(Vec::new());
    assert!(!none.contains("class=\"card lead\""));
    let standing = churn_page(vec![
        ("home".into(), 20, 0.3, 0.28),
        ("billing".into(), 12, 0.4, 0.1),
    ]);
    assert_eq!(standing.matches("class=\"card lead\"").count(), 1);
    let lead = &standing[standing.find("class=\"card lead\"").unwrap()..];
    let lead = &lead[..lead.find("</article>").unwrap()];
    assert!(lead.contains("billing") && !lead.contains("home"), "{lead}");
    assert!(lead.contains("/p/d/watch?churned_at=billing"));
    // The lead comes before the key numbers.
    assert!(standing.find("card lead") < standing.find("Key numbers"));
}

#[test]
fn acquisition_tabs_mark_one_current_tab_each_and_keep_the_filters() {
    let f = Filters {
        role: Some("admin".into()),
        internal: true,
        ..Filters::default()
    };
    let rows = vec![AcquisitionRow {
        value: "search".into(),
        users: 40,
        reached: 10,
        returned: 20,
        judged: 30,
        churned: 6,
    }];
    for (by, last) in [("channel", false), ("campaign", true)] {
        let html = render(acquisition(
            &project(),
            &Board::default(),
            &f,
            &rows,
            by,
            last,
        ));
        let navs: Vec<&str> = html
            .split("<nav class=\"tabs\"")
            .skip(1)
            .map(|s| &s[..s.find("</nav>").unwrap()])
            .collect();
        assert_eq!(navs.len(), 2, "group by, then touch");
        for nav in &navs {
            assert_eq!(nav.matches("aria-current=\"page\"").count(), 1, "{nav}");
            for href in nav.split("href=\"").skip(1) {
                let href = &href[..href.find('"').unwrap()];
                assert!(href.starts_with("/p/d/acquisition?by="), "{href}");
                assert!(
                    href.contains("env=production")
                        && href.contains("role=admin")
                        && href.contains("internal=1"),
                    "{href}"
                );
            }
        }
        let touch = if last { "touch=last" } else { "touch=first" };
        let current = |nav: &str| {
            let a = &nav[..nav.find("aria-current").unwrap()];
            a[a.rfind("href=\"").unwrap()..].to_owned()
        };
        assert!(current(navs[0]).contains(&format!("by={by}")));
        assert!(current(navs[1]).contains(touch));
    }
}

#[test]
fn the_audience_title_is_the_page_title_and_country_names_stay_in_the_map() {
    let a = openlens_engine::Audience {
        countries: vec![("FR".into(), 3), ("DE".into(), 1)],
        devices: vec![("desktop".into(), 3)],
        users: 4,
        ..Default::default()
    };
    let html = render(audience(
        &project(),
        &Board::default(),
        &Filters::default(),
        &a,
        true,
        None,
    ));
    let first = html.find("<title>").unwrap();
    let head_title = &html[first..first + html[first..].find("</title>").unwrap()];
    assert!(head_title.contains("Audience"), "{head_title}");
    assert!(!head_title.contains("France") && !head_title.contains("Germany"));
    // Every other <title> is a country's, inside the map.
    let map = html.find("<svg class=\"world\"").unwrap();
    let end = map + html[map..].find("</svg>").unwrap();
    let rest: Vec<usize> = html
        .match_indices("<title>")
        .map(|(i, _)| i)
        .filter(|i| *i != first)
        .collect();
    assert!(!rest.is_empty());
    assert!(
        rest.iter().all(|i| (map..end).contains(i)),
        "a <title> outside the map"
    );
}

#[test]
fn people_pages_carry_no_style_attribute_and_no_script() {
    let p = project();
    let (b, f) = (Board::default(), Filters::default());
    let rev = openlens_engine::Revenue {
        totals: vec![openlens_engine::Money {
            currency: "EUR".into(),
            total: 120.0,
            orders: 3,
            payers: 2,
            per_active_user: 4.0,
        }],
        by_feature: vec![("checkout".into(), "EUR".into(), 120.0)],
        ..Default::default()
    };
    let rows = [goal_row(4.0)];
    let pages = [
        render(audience(
            &p,
            &b,
            &f,
            &openlens_engine::Audience::default(),
            false,
            None,
        )),
        render(acquisition(&p, &b, &f, &[], "channel", false)),
        churn_page(vec![("billing".into(), 12, 0.4, 0.1)]),
        render(revenue(&p, &b, &f, Some(&rev))),
        render(goals(&p, &rows, &[], &[], true, "2027-01-01")),
    ];
    for html in &pages {
        assert!(
            !html.contains("style=") && !html.contains("<script"),
            "{html}"
        );
    }
}

/// Without payments, Revenue explains which event to send, with the
/// snippet as a code block, in one empty state.
#[test]
fn revenue_without_payments_shows_the_snippet() {
    let html = render(revenue(
        &project(),
        &Board::default(),
        &Filters::default(),
        None,
    ));
    assert_eq!(html.matches("class=\"card empty\"").count(), 1);
    let block = &html[html.find("<pre").unwrap_or(0)..];
    assert!(block.starts_with("<pre") && block.contains("<code>openlens.track("));
    assert!(!html.contains("style=") && !html.contains("<script"));
}

/// A board with two cohorts, and a churn report with a falling curve.
fn retention_page() -> (Board, String) {
    let b = Board {
        retention: vec![
            openlens_engine::RetentionRow {
                week: "2026-10-05".into(),
                users: 41,
                weeks: vec![0.61, 0.5, 0.375],
            },
            openlens_engine::RetentionRow {
                week: "2026-10-12".into(),
                users: 29,
                weeks: vec![0.24, 0.2],
            },
        ],
        ..Board::default()
    };
    let c = ChurnReport {
        window: 14,
        users: 40,
        survival: vec![(0, 1.0), (7, 0.8), (30, 0.45), (60, 0.45), (90, 0.2)],
        median_days: Some(21),
        ..ChurnReport::default()
    };
    let html = render(retention(&project(), &b, &Filters::default(), &c));
    (b, html)
}

#[test]
fn the_retention_tab_shows_the_cohort_numbers_the_board_had() {
    let (b, html) = retention_page();
    // The same table, from the same rows, as the board drew it. The body
    // is compared: the column tooltip's id is numbered per render.
    let alone = cohort_table(&b).into_string();
    let body = &alone[alone.find("<tbody>").unwrap()..alone.find("</tbody>").unwrap()];
    assert!(html.contains(body));
    let table = &html[html.find("<table class=\"heat\"").unwrap()..];
    let table = &table[..table.find("</table>").unwrap()];
    for r in &b.retention {
        let row = &table[table.find(&r.week).unwrap()..];
        let row = &row[..row.find("</tr>").unwrap()];
        for share in &r.weeks {
            assert!(row.contains(&pct(*share)), "{row}");
        }
    }
    assert!(!html.contains(" style="));
}

#[test]
fn the_survival_curve_steps_down_as_users_leave_and_says_its_points() {
    let (_, html) = retention_page();
    let svg = &html[html.find("<svg class=\"survival\"").unwrap()..];
    let svg = &svg[..svg.find("</svg>").unwrap()];
    // A step line: horizontal then vertical moves only.
    let path = svg.split("class=\"km\" d=\"").nth(1).unwrap();
    let path = &path[..path.find('"').unwrap()];
    assert!(path.starts_with('M') && path.contains(" H") && path.contains(" V"));
    assert!(!path.contains('L') && !path.contains('C'), "{path}");
    // Lower share, lower on the chart (larger y); equal shares, same y.
    let ys: Vec<f64> = svg
        .split(" cy=\"")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].parse().unwrap())
        .collect();
    assert_eq!(ys.len(), 5);
    assert!(ys.windows(2).all(|w| w[0] <= w[1]), "{ys:?}");
    assert_eq!(ys[2], ys[3]);
    assert!(ys[0] < ys[4]);
    // The text alternative lists every point.
    let desc = &svg[svg.find("<desc").unwrap()..svg.find("</desc>").unwrap()];
    for (d, s) in [(0, 1.0), (7, 0.8), (30, 0.45), (60, 0.45), (90, 0.2)] {
        assert!(desc.contains(&format!("day {d}: {}", pct(s))), "{desc}");
    }
    assert!(svg.contains("role=\"img\"") && svg.contains("aria-labelledby="));
}

#[test]
fn churn_no_longer_lists_how_long_they_stay() {
    // The curve is on Retention: Churn links there instead of repeating it.
    let html = churn_page(Vec::new());
    assert!(!html.contains("How long they stay"));
    assert!(html.contains("/p/d/retention"));
}

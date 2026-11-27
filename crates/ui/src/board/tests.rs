//! The Focus board's tests (#543 adds the v2 ones at the end).

use super::*;
use openlens_engine::{BoardWindow, Insight, Period, ReleaseRow, RetentionRow, Sessions};

#[test]
fn the_waiting_panel_says_the_wait_and_hides_when_the_board_follows() {
    let p = ProjectInfo {
        id: "p1",
        name: "P",
        key: "k",
    };
    let known = board_shell(&p, Some(4)).into_string();
    assert!(known.contains("Computing the board…"), "{known}");
    assert!(known.contains("about 4 s"), "{known}");
    assert!(
        known.contains(r#"class="board-waiting" role="status" aria-live="polite""#),
        "{known}"
    );
    let unknown = board_shell(&p, None).into_string();
    assert!(unknown.contains("a few seconds"), "{unknown}");
    // The panel is the last thing in `<main>`: the board streamed after
    // it makes it no longer the last child, which hides it.
    let cut = known.rfind("</main>").unwrap();
    assert!(known[..cut].trim_end().ends_with("</div></div>"), "{known}");
    assert!(crate::CSS.contains(".board-waiting:not(:last-child) { display: none; }"));
    assert!(crate::CSS.contains(
        "@media (prefers-reduced-motion: reduce) { .board-waiting .spinner, .board-waiting .waiting-bar span { animation: none; } }"
    ));
}

#[test]
fn the_period_picker_keeps_the_window_and_the_filters() {
    let b = Board {
        window: Some(BoardWindow {
            releases: vec!["v2".into(), "v1".into()],
            ..BoardWindow::default()
        }),
        ..Board::default()
    };
    let day = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
    let f = Filters {
        role: Some("admin".into()),
        window: openlens_engine::Window::Range {
            from: day("2026-10-01"),
            to: day("2026-10-31"),
        },
        ..Filters::default()
    };
    let bar = context_bar("p1", &b, &f, "", true).into_string();
    let html = crate::components::tests::pop_form(&bar, "Period");
    assert!(html.contains(r#"method="get""#), "{html}");
    assert!(html.contains(r#"name="from" value="2026-10-01""#), "{html}");
    assert!(html.contains(r#"name="to" value="2026-10-31""#), "{html}");
    assert!(html.contains(r#"name="role" value="admin""#), "{html}");
    assert!(
        html.contains(r#"<input type="radio" name="since" value="v1">Since v1"#),
        "{html}"
    );
    assert!(!bar.contains("<script"), "no JavaScript");
    // Both date fields point at the format hint (#557), which exists.
    assert_eq!(
        bar.matches(r#"type="date""#).count(),
        bar.matches(r#"aria-describedby="period-date-format""#)
            .count(),
        "{bar}"
    );
    assert!(
        html.contains(r#"id="period-date-format">Dates as YYYY-MM-DD<"#),
        "{html}"
    );
    // A past release is checked, and the dates are left empty.
    let f = Filters {
        window: openlens_engine::Window::Since {
            release: "v1".into(),
        },
        ..Filters::default()
    };
    let bar = context_bar("p1", &b, &f, "", true).into_string();
    let html = crate::components::tests::pop_form(&bar, "Period");
    assert!(html.contains(r#"value="v1" checked"#), "{html}");
    assert!(!html.contains(r#"name="from" value"#), "{html}");
    // The other chips carry the window along.
    let env = crate::components::tests::pop_form(&bar, "Environment");
    assert!(env.contains(r#"name="since" value="v1""#), "{env}");
}

#[test]
fn the_board_names_the_window_it_reads() {
    let mut b = Board::default();
    assert_eq!(period(&b, "last 7 days"), "last 7 days", "fixed windows");
    b.window = Some(BoardWindow {
        release: Some("v2.1".into()),
        days: 4,
        label: "Since v2.1 (4 days)".into(),
        current: Period {
            users: 12,
            ..Period::default()
        },
        previous: Period {
            users: 10,
            ..Period::default()
        },
        ..BoardWindow::default()
    });
    assert_eq!(period(&b, "last 7 days"), "since v2.1 (4 days)");
    // One line under the title (#588), the users in the chart card.
    let head = subtitle(&b, &Filters::default(), "everyone").into_string();
    assert!(head.contains("Since v2.1 (4 days)"), "{head}");
    assert!(head.contains("compared with the 4 days before."), "{head}");
    let users = b.window.as_ref().map(window_users).unwrap_or_default();
    assert_eq!(users, "12 users, +20% against the 4 days before");
}

#[test]
fn the_page_head_says_the_window_once() {
    let html = page(&full());
    let head = between(
        &html,
        r#"<div class="page-head">"#,
        r#"<div class="actions">"#,
    );
    assert_eq!(head.matches("<p").count(), 1, "one subtitle: {head}");
    assert!(head.contains("compared with the 10 days before."), "{head}");
    // The window's users moved to the active users card.
    let au = between(&html, r#"id="au-h""#, "</section>");
    assert!(
        au.contains("144 users, -4% against the 10 days before"),
        "{au}"
    );
}

#[test]
fn the_board_reads_in_the_mockup_order() {
    let mut b = full();
    b.role_gaps = vec![("Admin ".into(), 3)];
    let html = page(&b);
    let at = |needle: &str| {
        html.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing: {html}"))
    };
    let order = [
        r#"class="card lead""#,
        r#"class="card ranked""#,
        r#"id="kpi-h""#,
        r#"id="au-h""#,
        r#"id="fp-h""#,
        "Do they come back?",
        "v2 compared with v1",
        r#"id="wv-h""#,
        r#"id="gaps-h""#,
    ];
    for w in order.windows(2) {
        assert!(at(w[0]) < at(w[1]), "{} before {}", w[0], w[1]);
    }
    // The cohorts are open, the release comparison and the secondary
    // cards closed.
    assert!(html.contains("<details class=\"card disclose\" open>"));
    assert_eq!(
        html.matches(r#"<details class="card disclose secondary">"#)
            .count(),
        2,
        "Web Vitals and role gaps, closed: {html}"
    );
    // On a share link, the secondary cards keep their content, not the
    // links to their pages.
    let shared = shared_page(&b);
    let gaps = between(&shared, r#"id="gaps-h""#, "</details>");
    assert!(gaps.contains("Admin ") && !gaps.contains("<a "), "{gaps}");
}

#[test]
fn ranked_insights_carry_no_next_step() {
    let b = full();
    let html = page(&b);
    let ranked = between(&html, r#"class="card ranked""#, "</ol>");
    assert!(!ranked.contains("Next step"), "{ranked}");
    assert!(!ranked.contains(&b.insights[1].next_step), "{ranked}");
    let lead = between(&html, r#"class="card lead""#, r#"class="card ranked""#);
    assert!(lead.contains("Next step") && lead.contains(&b.insights[0].next_step));
}

#[test]
fn the_release_table_prints_first_seen_as_the_other_screens_do() {
    // #556: the table printed the engine's ISO string (`…T20:49`).
    let p = ProjectInfo {
        id: "p1",
        name: "P",
        key: "k",
    };
    let b = Board {
        total_events: 1,
        releases: vec![openlens_engine::ReleaseRow {
            release: "v2".into(),
            first_seen: "2026-11-16T20:49:31.5Z".into(),
            users: 3,
            sessions: 4,
            ..openlens_engine::ReleaseRow::default()
        }],
        ..Board::default()
    };
    let html = board(&p, &b, &Filters::default(), "h", &[], None, &[]).into_string();
    assert!(html.contains("2026-11-16 20:49"), "{html}");
    assert!(!crate::tests::has_iso_time(&html), "{html}");
}

fn project() -> ProjectInfo<'static> {
    ProjectInfo {
        id: "p1",
        name: "P",
        key: "k",
    }
}

fn insight(kind: &'static str, subject: &str) -> Insight {
    Insight {
        kind,
        title: format!("{kind} title"),
        evidence: format!("{kind} evidence"),
        next_step: format!("{kind} next step"),
        score: 1.0,
        subject: subject.into(),
    }
}

/// A board with three insights, a window compared with the one before,
/// a few days of active users, two releases and two cohorts.
fn full() -> Board {
    let now = Sessions {
        count: 600,
        avg_secs: 200.0,
        bounce_rate: 0.1,
        error_rate: 0.33,
        tasks_per_user_day: 2.0,
    };
    Board {
        total_events: 100,
        insights: vec![
            insight("release_regression", "v2"),
            insight("churn_point", "billing"),
            insight("role_blind_spot", "admin:reports"),
        ],
        sessions: now.clone(),
        window: Some(BoardWindow {
            release: Some("v2".into()),
            days: 10,
            label: "Since v2 (10 days)".into(),
            current: Period {
                users: 144,
                sessions: now,
            },
            previous: Period {
                users: 150,
                sessions: Sessions {
                    count: 700,
                    avg_secs: 250.0,
                    bounce_rate: 0.1,
                    error_rate: 0.03,
                    tasks_per_user_day: 2.0,
                },
            },
            ..BoardWindow::default()
        }),
        dau_series: vec![
            ("2026-11-16".into(), 8),
            ("2026-11-17".into(), 80),
            ("2026-11-18".into(), 40),
        ],
        releases: vec![
            ReleaseRow {
                release: "v2".into(),
                first_seen: "2026-11-16T20:49:31Z".into(),
                users: 144,
                sessions: 623,
                error_rate: 0.334,
                avg_secs: 237.0,
                bounce_rate: 0.0,
                tasks_per_session: 2.48,
            },
            ReleaseRow {
                release: "v1".into(),
                first_seen: "2026-09-03T10:00:00Z".into(),
                users: 300,
                sessions: 4814,
                error_rate: 0.029,
                avg_secs: 272.0,
                bounce_rate: 0.032,
                tasks_per_session: 2.76,
            },
        ],
        retention: vec![
            RetentionRow {
                week: "2026-10-05".into(),
                users: 41,
                weeks: vec![1.0, 0.5],
            },
            RetentionRow {
                week: "2026-10-12".into(),
                users: 29,
                weeks: vec![0.04],
            },
        ],
        ..Board::default()
    }
}

fn page(b: &Board) -> String {
    board(&project(), b, &Filters::default(), "h", &[], None, &[]).into_string()
}

fn shared_page(b: &Board) -> String {
    render_shared(|| board(&project(), b, &Filters::default(), "h", &[], None, &[])).into_string()
}

/// The part of `html` from `from` to the next `to` after it.
fn between<'a>(html: &'a str, from: &str, to: &str) -> &'a str {
    let start = html.find(from).unwrap();
    let end = html[start..].find(to).map_or(html.len(), |e| start + e);
    &html[start..end]
}

#[test]
fn the_first_insight_leads_and_the_others_are_ranked() {
    let b = full();
    let html = page(&b);
    assert_eq!(html.matches(r#"class="card lead""#).count(), 1, "{html}");
    assert_eq!(
        html.matches(r#"<li class="row">"#).count(),
        b.insights.len() - 1,
        "{html}"
    );
    assert!(html.contains(&format!(
        r#"<span class="count">{}</span>"#,
        b.insights.len()
    )));
    let lead = between(&html, r#"class="card lead""#, r#"class="card ranked""#);
    assert!(lead.contains(&b.insights[0].title), "{lead}");
    assert_eq!(lead.matches("btn primary").count(), 1, "one primary action");
    assert!(lead.contains("Create ticket"), "{lead}");
    // The release table holds both error rates: the lead compares them.
    assert!(lead.contains(r#"<dl class="compare">"#), "{lead}");
    assert!(lead.contains("4,814 sessions"), "{lead}");
    // A churn insight carries no two comparable values: no comparison.
    let mut churn = full();
    churn.insights.remove(0);
    let html = page(&churn);
    let lead = between(&html, r#"class="card lead""#, r#"class="card ranked""#);
    assert!(lead.contains(&churn.insights[0].title), "{lead}");
    assert!(!lead.contains("compare"), "{lead}");
    assert_eq!(
        html.matches(r#"<li class="row">"#).count(),
        churn.insights.len() - 1
    );
}

#[test]
fn every_kpi_delta_has_a_sign_and_says_what_it_compares_with() {
    let html = page(&full());
    let tiles = between(&html, r#"id="kpi-h""#, "</section>");
    let deltas: Vec<&str> = tiles
        .split(r#"<span class="delta "#)
        .skip(1)
        .map(|s| s.split("</div>").next().unwrap())
        .collect();
    assert_eq!(deltas.len(), 3, "avg session, tasks, errors: {tiles}");
    for d in &deltas {
        assert!(
            d.contains("#i-up") || d.contains("#i-down") || d.contains("#i-right"),
            "an icon: {d}"
        );
        assert!(d.contains('+') || d.contains('−'), "a sign: {d}");
        assert!(
            d.contains("vs the 10 days before") || d.contains("was "),
            "what it is compared with: {d}"
        );
    }
    // Errors went up: that tile alerts, says it in words, and only it.
    assert_eq!(tiles.matches("is-alert").count(), 1);
    let alert = between(tiles, "is-alert", "</a>");
    assert!(alert.contains("Sessions with errors"), "{alert}");
    assert!(
        alert.contains("(worse)") && alert.contains("was 3.0%"),
        "{alert}"
    );
    // Without a window compared with the one before, no delta at all.
    let mut fixed = full();
    fixed.window = None;
    let html = page(&fixed);
    let tiles = between(&html, r#"id="kpi-h""#, "</section>");
    assert!(!tiles.contains(r#"class="delta "#) && !tiles.contains("is-alert"));
}

#[test]
fn each_kpi_definition_opens_from_its_button() {
    let html = page(&full());
    let tiles = between(&html, r#"id="kpi-h""#, "</section>");
    let targets: Vec<&str> = tiles
        .split("popovertarget=\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap())
        .collect();
    assert_eq!(targets.len(), 5, "one per tile: {tiles}");
    for t in targets {
        assert!(
            tiles.contains(&format!(r#"popover="auto" id="{t}""#)),
            "{t} has its popover"
        );
        // Placed by its own button (the CSP forbids style=).
        assert!(crate::CSS.contains(&format!("#{t}-btn {{ anchor-name: --{t}; }}")));
        assert!(crate::CSS.contains(&format!("#{t} {{ position-anchor: --{t}; }}")));
    }
    assert!(tiles.contains(r#"aria-label="Definition: Sessions with errors""#));
}

#[test]
fn a_bigger_day_has_a_taller_bar() {
    let series: Vec<(String, i64)> = [8, 80, 40, 0, 79]
        .iter()
        .enumerate()
        .map(|(i, n)| (format!("2026-11-{:02}", i + 10), *n))
        .collect();
    let html = charts::dau_chart(&series).into_string();
    let heights: Vec<f64> = html
        .split(" height=\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(heights.len(), series.len(), "{html}");
    for (i, (_, a)) in series.iter().enumerate() {
        for (j, (_, b)) in series.iter().enumerate() {
            if a > b {
                assert!(heights[i] > heights[j], "{a} vs {b}: {heights:?}");
            }
        }
    }
    assert!(
        html.contains("<title>2026-11-11: 80 users</title>"),
        "{html}"
    );
}

#[test]
fn heat_cells_are_shaded_by_tenths() {
    assert!(charts::tenth(1.0) > charts::tenth(0.5));
    assert!(charts::tenth(0.5) > charts::tenth(0.04));
    let html = page(&full());
    let heat = between(&html, r#"<table class="heat">"#, "</table>");
    assert!(heat.contains(r#"class="hc p10">100.0%"#), "{heat}");
    assert!(heat.contains(r#"class="hc p5">50.0%"#), "{heat}");
}

#[test]
fn the_board_has_no_inline_style_or_script() {
    for html in [page(&full()), shared_page(&full())] {
        assert!(!html.contains(" style="), "{html}");
        assert!(!html.contains("<script"), "{html}");
    }
}

#[test]
fn the_shared_board_hides_what_needs_a_login() {
    let b = full();
    let own = page(&b);
    let shared = shared_page(&b);
    assert!(shared.contains(&b.insights[0].title) && shared.contains("Key numbers"));
    for needle in [
        "/tickets/new",
        "Create ticket",
        "/digest",
        "/watch?",
        r#"<a class="card kpi"#,
        "/errors?",
        "/timeline",
    ] {
        assert!(own.contains(needle), "the signed-in board has {needle}");
        assert!(!shared.contains(needle), "the shared board hides {needle}");
    }
}

/// A feature used by `users`, `completed_users` of whom reached its
/// declared success event.
fn used(key: &str, users: i64, completed_users: i64) -> openlens_engine::FeatureUse {
    openlens_engine::FeatureUse {
        key: key.into(),
        name: None,
        uses: users * 3,
        users,
        completed: completed_users,
        completed_users,
        user_days: users,
        declared: true,
    }
}

#[test]
fn a_success_that_fires_on_entry_shows_no_task_time_but_its_finding() {
    let task = |feature: &str, median_secs: f64| openlens_engine::TaskTime {
        feature: feature.into(),
        median_secs,
        p90_secs: median_secs * 2.0,
        tasks: 40,
    };
    // Home's success is its page view: every user reaches it at once.
    let b = Board {
        features: vec![used("export", 40, 10), used("home", 40, 40)],
        task_times: vec![task("export", 60.0), task("home", 0.0)],
        ..full()
    };
    let html = charts::feature_pulse("p1", &Filters::default(), &b).into_string();
    // The Completion cell: the last of the feature's row.
    let cell = |key: &str| {
        let row = between(&html, &format!("/features/{key}"), "</tr>");
        row[row.rfind("<td").unwrap()..].to_owned()
    };
    let (export, home) = (cell("export"), cell("home"));
    // The same table still times a real task and shows its completion.
    assert!(
        export.contains("Task time") && export.contains('%'),
        "{export}"
    );
    assert!(!home.contains("Task time"), "{home}");
    assert!(!home.contains('%'), "no completion either: {home}");
    // It links to the Map health finding, which carries that anchor.
    let prefix = "href=\"/p/p1/features#";
    let at = home.find(prefix).unwrap() + prefix.len();
    let anchor = &home[at..at + home[at..].find('"').unwrap()];
    let h = openlens_engine::MapHealth {
        findings: vec![openlens_engine::maphealth::Finding {
            kind: "success_always",
            level: "warning",
            feature: Some("home".into()),
            message: "every user reaches it".into(),
            evidence: "40 of 40".into(),
            fix: None,
            impact: 40,
        }],
        ..openlens_engine::MapHealth::default()
    };
    let health = crate::features::map_health("p1", &h).into_string();
    assert!(
        health.contains(&format!("id=\"{anchor}\"")),
        "{anchor} in {health}"
    );
    // The shared board, without a login, keeps the dash and drops the link.
    let shared =
        render_shared(|| charts::feature_pulse("p1", &Filters::default(), &b)).into_string();
    assert!(!shared.contains("/features#"), "{shared}");
    assert!(!shared.contains("Task time: median 0"), "{shared}");
}

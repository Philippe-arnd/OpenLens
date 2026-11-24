//! Integration tests for `openlens-engine`: board numbers on a known dataset.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Utc};
use openlens_engine::{EventQuery, Filters, board, event_list, family, recent_events};
use openlens_event::Event;
use openlens_featuremap::parse;
use openlens_query::{Limits, QueryEngine};
use openlens_storage::{FlushPolicy, Writer};

struct E<'a> {
    user: &'a str,
    role: &'a str,
    session: &'a str,
    event: &'a str,
    mins_ago: i64,
    feature: Option<&'a str>,
    release: &'a str,
    props: &'a str,
}

fn mk(e: &E<'_>, n: usize) -> Event {
    let ts = Utc::now() - Duration::minutes(e.mins_ago);
    Event {
        uuid: format!("{}-{}-{n}", e.user, e.mins_ago),
        project: "p".into(),
        event: e.event.into(),
        distinct_id: e.user.into(),
        session_id: Some(e.session.into()),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some("/".into()),
        feature: e.feature.map(Into::into),
        role: Some(e.role.into()),
        account: None,
        plan: None,
        environment: "production".into(),
        release: Some(e.release.into()),
        flags: None,
        properties: e.props.into(),
    }
}

fn dataset() -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let base = |user, role, session, event, mins_ago, feature, release| E {
        user,
        role,
        session,
        event,
        mins_ago,
        feature,
        release,
        props: "{}",
    };
    let rows = vec![
        // admin: a 10-minute session with an export, on v2
        base("a1", "admin", "s1", "$pageview", 20, None, "v2"),
        base("a1", "admin", "s1", "$click", 10, Some("export"), "v2"),
        // member: two sessions, one bounce, one with an error, on v1 then v2
        base("m1", "member", "s2", "$pageview", 60 * 30, None, "v1"),
        base("m1", "member", "s3", "$pageview", 5, None, "v2"),
        base("m1", "member", "s3", "$error", 4, None, "v2"),
        base("m1", "member", "s3", "$click", 3, Some("search"), "v2"),
        base("m2", "member", "s4", "$click", 2, Some("search"), "v2"),
        E {
            props: r#"{"$name":"LCP","$value":1800,"$rating":"good"}"#,
            ..base("a1", "admin", "s1", "$vital", 9, None, "v2")
        },
        E {
            props: r#"{"$name":"LCP","$value":5200,"$rating":"poor"}"#,
            ..base("m1", "member", "s3", "$vital", 2, None, "v2")
        },
    ];
    let evs: Vec<_> = rows.iter().enumerate().map(|(i, e)| mk(e, i)).collect();
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

#[tokio::test]
async fn board_numbers_follow_the_kpi_definitions() {
    let (_d, q) = dataset();
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    assert_eq!(b.users.mau, 3);
    assert!(b.users.dau <= b.users.wau && b.users.wau <= b.users.mau);
    assert!(b.sessions.count >= 3);
    assert!(b.sessions.error_rate > 0.0 && b.sessions.error_rate < 1.0);
    assert!(
        b.sessions.avg_secs > 60.0,
        "the admin session lasted ~10 minutes"
    );
    assert_eq!(b.features[0].key, "search", "ranked by users first");
    assert!(b.roles.iter().any(|r| r.role.as_deref() == Some("member")));
    assert_eq!(b.releases[0].release, "v2", "newest release first");
    let lcp = b.vitals.iter().find(|v| v.name == "LCP").unwrap();
    assert_eq!(lcp.samples, 2);
    assert!(lcp.p75 > 1800.0);
    assert_eq!(b.environments, ["production"]);
}

#[tokio::test]
async fn role_filter_narrows_every_number() {
    let (_d, q) = dataset();
    let all = board(&q, "p", &Filters::default(), None).await.unwrap();
    let admins = board(
        &q,
        "p",
        &Filters {
            role: Some("admin".into()),
            ..Filters::default()
        },
        None,
    )
    .await
    .unwrap();
    assert!(admins.users.mau < all.users.mau);
    assert!(admins.features.iter().all(|f| f.key == "export"));
}

#[tokio::test]
async fn hostile_filter_values_cannot_break_out_of_the_literal() {
    let (_d, q) = dataset();
    let f = Filters {
        role: Some("x' OR '1'='1".into()),
        ..Filters::default()
    };
    let b = board(&q, "p", &f, None).await.unwrap();
    assert_eq!(b.users.mau, 0);
    // Bound as values (#310): quotes, backslashes and `$` stay data.
    for v in [
        r"a\' OR 1=1 --",
        "$env",
        "'; DROP TABLE events; --",
        "rôle ✓",
    ] {
        let f = Filters {
            release: Some(v.into()),
            environment: v.into(),
            ..Filters::default()
        };
        let b = board(&q, "p", &f, None).await.unwrap();
        assert_eq!(b.users.mau, 0, "{v}");
    }
    let bad = Filters {
        role: Some("a\nb".into()),
        ..Filters::default()
    };
    assert!(board(&q, "p", &bad, None).await.is_err());
}

#[tokio::test]
async fn recent_events_are_newest_first() {
    let (_d, q) = dataset();
    let rows = recent_events(&q, "p", &Filters::default(), None, None, 5)
        .await
        .unwrap();
    assert_eq!(rows.len(), 5);
    assert!(rows[0].timestamp >= rows[4].timestamp);
}

#[tokio::test]
async fn events_filter_by_family_and_page_with_a_cursor() {
    let (_d, q) = dataset();
    let f = Filters::default();
    let all = event_list(
        &q,
        "p",
        &f,
        None,
        &EventQuery {
            limit: 200,
            ..EventQuery::default()
        },
    )
    .await
    .unwrap();
    let clicks = event_list(
        &q,
        "p",
        &f,
        None,
        &EventQuery {
            family: Some("click".into()),
            limit: 200,
            ..EventQuery::default()
        },
    )
    .await
    .unwrap();
    assert!(!clicks.is_empty() && clicks.len() < all.len());
    assert!(clicks.iter().all(|r| family(&r.event) == "click"));
    let first = event_list(
        &q,
        "p",
        &f,
        None,
        &EventQuery {
            limit: 3,
            ..EventQuery::default()
        },
    )
    .await
    .unwrap();
    let next = event_list(
        &q,
        "p",
        &f,
        None,
        &EventQuery {
            before: Some(first[2].timestamp.clone()),
            limit: 3,
            ..EventQuery::default()
        },
    )
    .await
    .unwrap();
    assert!(next.iter().all(|r| r.timestamp < first[2].timestamp));
    let by_user = event_list(
        &q,
        "p",
        &f,
        None,
        &EventQuery {
            sort: Some("person".into()),
            limit: 200,
            ..EventQuery::default()
        },
    )
    .await
    .unwrap();
    assert!(
        by_user
            .windows(2)
            .all(|w| w[0].distinct_id <= w[1].distinct_id)
    );
    assert!(
        event_list(
            &q,
            "p",
            &f,
            None,
            &EventQuery {
                family: Some("nope".into()),
                limit: 5,
                ..EventQuery::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(family("signup_done"), "custom");
    assert_eq!(family("$weird"), "other");
    let b = board(&q, "p", &f, None).await.unwrap();
    assert!(b.users.known >= b.users.mau && b.users.dau_share() <= 1.0);
}

#[tokio::test]
async fn feature_map_gives_canonical_roles_gaps_and_unused_features() {
    let (_d, q) = dataset();
    let map = parse(
        "roles:\n  - key: staff\n    match: [admin]\nfeatures:\n  - key: search\n    name: Search\n    success: $click\n    entry: { routes: ['/search'] }\n  - key: billing\n    name: Billing\n    entry: { routes: ['/billing'] }\n",
    )
    .map
    .unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 3)))
        .await
        .unwrap();
    assert_eq!(b.map_version, Some(3));
    assert!(
        b.roles.iter().any(|r| r.role.as_deref() == Some("staff")),
        "admin maps to staff"
    );
    assert!(
        b.role_gaps.iter().any(|(raw, _)| raw == "member"),
        "member is not mapped: a gap; roles={:?} gaps={:?}",
        b.roles,
        b.role_gaps
    );
    let billing = b.features.iter().find(|f| f.key == "billing").unwrap();
    assert!(
        billing.declared && billing.users == 0,
        "declared but never used is listed"
    );
    let search = b.features.iter().find(|f| f.key == "search").unwrap();
    assert_eq!(search.name.as_deref(), Some("Search"));
    assert!(search.completed > 0);

    let staff = board(
        &q,
        "p",
        &Filters {
            role: Some("staff".into()),
            ..Filters::default()
        },
        Some((&map, 3)),
    )
    .await
    .unwrap();
    assert!(
        staff.users.mau >= 1 && staff.users.mau < b.users.mau,
        "filter by canonical role"
    );
}

#[tokio::test]
async fn insights_find_every_planted_pattern() {
    use openlens_testkit::planted;
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(&openlens_testkit::planted_dataset("p")).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    let kinds: Vec<_> = b
        .insights
        .iter()
        .map(|i| (i.kind, i.subject.as_str()))
        .collect();
    let find = |k: &str| b.insights.iter().find(|i| i.kind == k);

    let driver = find("stick_driver").unwrap_or_else(|| panic!("no stick driver in {kinds:?}"));
    assert_eq!(driver.subject, planted::DRIVER, "{kinds:?}");
    assert!(
        b.insights
            .iter()
            .filter(|i| i.kind == "stick_driver")
            .all(|i| i.subject == planted::DRIVER),
        "search has no effect on retention and must not be a driver: {kinds:?}"
    );
    assert_eq!(
        find("churn_point").unwrap().subject,
        planted::CHURN_POINT,
        "{kinds:?}"
    );
    assert_eq!(
        find("role_blind_spot").unwrap().subject,
        format!("{}:{}", planted::BLIND_ROLE, planted::BLIND_FEATURE),
        "{kinds:?}"
    );
    assert_eq!(
        find("release_regression").unwrap().subject,
        planted::BAD_RELEASE,
        "{kinds:?}"
    );
    assert!(find("slow_pages").is_some(), "{kinds:?}");
    // Who uses what: the blind role barely touches the blind feature.
    let used = |role: &str, feat: &str| {
        b.role_features
            .iter()
            .find(|(r, f, _)| r == role && f == feat)
            .map_or(0, |x| x.2)
    };
    assert!(!b.role_features.is_empty());
    assert!(
        used(planted::BLIND_ROLE, planted::BLIND_FEATURE) * 5
            < used("member", planted::BLIND_FEATURE).max(1),
        "{:?}",
        b.role_features
    );
    // The churn-point card leads to the sessions where those users left.
    let left =
        openlens_engine::churned_sessions(&q, "p", None, &Filters::default(), planted::CHURN_POINT)
            .await
            .unwrap();
    assert!(left.len() >= 8, "{}", left.len());
    assert!(b.insights.len() <= 7);
    let mut subjects: Vec<_> = b
        .insights
        .iter()
        .map(|i| (i.kind, i.subject.clone()))
        .collect();
    subjects.sort();
    subjects.dedup();
    assert_eq!(
        subjects.len(),
        b.insights.len(),
        "no duplicate cards: {kinds:?}"
    );
    assert!(b.readiness.judged_users > 100);
}

#[tokio::test]
async fn no_insights_without_enough_users() {
    let (_d, q) = dataset();
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    assert!(
        b.insights
            .iter()
            .all(|i| i.kind == "slow_pages" || i.kind == "release_regression")
    );
    assert!(b.readiness.judged_users < b.readiness.needed);
}

#[tokio::test]
async fn journeys_show_the_common_path_into_a_success() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for u in 0..6 {
        let user = format!("j{u}");
        let steps: &[(&str, Option<&str>, &str)] = &[
            ("$pageview", None, "/home"),
            ("$pageview", None, "/home"),
            ("$click", Some("search"), "/search"),
            ("$pageview", None, "/entries/new"),
            ("entry_saved", None, "/entries/new"),
        ];
        for (i, (name, feat, path)) in steps.iter().enumerate() {
            let e = E {
                user: &user,
                role: "member",
                session: "s",
                event: name,
                mins_ago: 50 - i as i64,
                feature: *feat,
                release: "v1",
                props: "{}",
            };
            let mut ev = mk(&e, u * 10 + i);
            ev.path = Some((*path).into());
            evs.push(ev);
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let map = parse("features:\n  - key: write\n    name: Write\n    success: entry_saved\n    entry: { routes: ['/entries/new'] }\n").map.unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    let j = b.journeys.first().expect("a journey");
    assert_eq!(j.success, "entry_saved");
    assert_eq!(
        j.path,
        ["/home", "search", "write"],
        "repeats collapse; route resolves to the feature"
    );
    assert_eq!(j.users, 6);
    assert!((j.share - 1.0).abs() < 1e-9);
}

#[tokio::test]
async fn a_flag_with_more_errors_shows_up_as_its_own_cohort() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for u in 0..120 {
        let flagged = u % 2 == 0;
        let user = format!("f{u}");
        let session = format!("fs{u}");
        for (i, name) in [
            "$pageview",
            "$click",
            if flagged && u % 4 == 0 {
                "$error"
            } else {
                "$pageview"
            },
        ]
        .iter()
        .enumerate()
        {
            let e = E {
                user: &user,
                role: "member",
                session: &session,
                event: name,
                mins_ago: 30 - i as i64,
                feature: None,
                release: "v1",
                props: "{}",
            };
            let mut ev = mk(&e, u * 10 + i);
            if flagged {
                ev.flags = Some("new-editor".into());
            }
            evs.push(ev);
        }
    }
    let mut hostile = evs[0].clone();
    hostile.uuid = "hostile".into();
    hostile.flags = Some("x') OR 1=1 --,ok-flag".into());
    evs.push(hostile);
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    let c = b
        .flag_cohorts
        .iter()
        .find(|c| c.flag == "new-editor")
        .expect("cohort");
    assert_eq!(c.with.users, 60);
    assert!(c.p_errors.unwrap() < 0.05, "{c:?}");
    assert!(c.with.error_rate > c.without.error_rate);
    assert!(
        b.flag_cohorts.iter().all(|c| c.flag != "x') OR 1=1 --"),
        "hostile flag names are ignored"
    );
    assert!(
        b.insights
            .iter()
            .any(|i| i.kind == "flag_regression" && i.subject == "new-editor")
    );
}

#[tokio::test]
async fn funnels_follow_declared_steps_in_order_and_flag_the_big_drop() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    // 20 users open the editor, 16 fill a section, 4 save. Plus 5 users who
    // save without opening (out of order: must not count).
    for u in 0..25 {
        let user = format!("fu{u}");
        let steps: Vec<&str> = if u >= 20 {
            vec!["entry_saved"]
        } else if u < 4 {
            vec!["open_editor", "fill_section", "entry_saved"]
        } else if u < 16 {
            vec!["open_editor", "fill_section"]
        } else {
            vec!["open_editor"]
        };
        for (i, name) in steps.iter().enumerate() {
            let e = E {
                user: &user,
                role: "member",
                session: "s",
                event: name,
                mins_ago: 40 - (i as i64) * 5,
                feature: None,
                release: "v1",
                props: "{}",
            };
            evs.push(mk(&e, u * 10 + i));
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let map = parse("features:\n  - key: write\n    name: Write\n    steps: [open_editor, fill_section]\n    success: entry_saved\n").map.unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    let f = &b.funnels[0];
    let users: Vec<i64> = f.steps.iter().map(|s| s.users).collect();
    assert_eq!(
        users,
        [20, 16, 4],
        "success appended; out-of-order saves excluded"
    );
    assert!(f.steps[1].median_secs > 200.0, "~5 minutes between steps");
    let card = b
        .insights
        .iter()
        .find(|i| i.kind == "funnel_drop")
        .expect("drop card");
    assert!(
        card.title.contains("fill_section") && card.title.contains("entry_saved"),
        "{}",
        card.title
    );
}

#[tokio::test]
async fn errors_group_by_normalised_message_and_source() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for u in 0..9 {
        let user = format!("e{u}");
        let (msg, source) = if u < 6 {
            (
                format!("Cannot read properties of undefined (reading 'id{u}') at row {u}"),
                format!("https://app.test/assets/app.js?v={u}:10:{u}"),
            )
        } else {
            (
                "Network down".to_owned(),
                "https://app.test/assets/net.js:3:1".to_owned(),
            )
        };
        let props = serde_json::json!({"$message": msg, "$source": source}).to_string();
        let e = E {
            user: &user,
            role: if u % 2 == 0 { "admin" } else { "member" },
            session: &user,
            event: "$error",
            mins_ago: 10,
            feature: None,
            release: "v2",
            props: &props,
        };
        evs.push(mk(&e, u));
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let groups =
        openlens_engine::error_groups(&q, "p", &openlens_engine::source(None), &Filters::default())
            .await
            .unwrap();
    assert_eq!(groups.len(), 2, "{groups:#?}");
    assert_eq!(groups[0].users, 6, "six variants of one bug are one group");
    assert!(
        groups[0]
            .fingerprint
            .ends_with("@ https://app.test/assets/app.js"),
        "{}",
        groups[0].fingerprint
    );
    assert!(groups[0].roles.iter().any(|(r, n)| r == "admin" && *n == 3));
    assert_eq!(groups[0].releases[0].0, "v2");
}

#[tokio::test]
async fn pages_show_scroll_depth_hotspots_and_rage_clicks() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for u in 0..10 {
        let user = format!("p{u}");
        let depth = if u < 3 { 95 } else { 30 };
        let rows = [
            ("$pageview", "{}".to_owned()),
            (
                "$click",
                r#"{"$tag":"button","$text":"Save draft"}"#.to_owned(),
            ),
            (
                "$click",
                if u < 2 {
                    r#"{"$tag":"button","$text":"Pay","$rage":true}"#.to_owned()
                } else {
                    r#"{"$tag":"a","$text":"Help"}"#.to_owned()
                },
            ),
            ("$scroll", format!(r#"{{"$depth":{depth}}}"#)),
        ];
        for (i, (name, props)) in rows.iter().enumerate() {
            let e = E {
                user: &user,
                role: "member",
                session: &user,
                event: name,
                mins_ago: 20 - i as i64,
                feature: None,
                release: "v1",
                props,
            };
            let mut ev = mk(&e, u * 10 + i);
            ev.path = Some("/checkout".into());
            evs.push(ev);
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let pages =
        openlens_engine::pages(&q, "p", &openlens_engine::source(None), &Filters::default())
            .await
            .unwrap();
    let p = &pages[0];
    assert_eq!((p.path.as_str(), p.views, p.users), ("/checkout", 10, 10));
    assert_eq!(p.hotspots[0].label, "Save draft");
    assert!(p.hotspots.iter().any(|h| h.label == "Pay" && h.rage == 2));
    assert!(
        (p.reach_end - 0.3).abs() < 1e-9 && p.reach_half < 0.5,
        "3 of 10 reached the end"
    );
}

#[tokio::test]
async fn task_times_and_resurrected_users_follow_the_kpi_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let ev =
        |user: &'static str, event: &'static str, mins_ago: i64, feature: Option<&'static str>| E {
            user,
            role: "member",
            session: user,
            event,
            mins_ago,
            feature,
            release: "v1",
            props: "{}",
        };
    let evs: Vec<Event> = [
        // 60 s task (two entries: the task starts at the first one).
        ev("a", "$click", 12, Some("export")),
        ev("a", "$click", 11, Some("export")),
        ev("a", "export_done", 11, None),
        // 300 s task.
        ev("b", "$click", 20, Some("export")),
        ev("b", "export_done", 15, None),
        // Abandoned: the success comes 100 min after the only entry.
        ev("c", "$click", 200, Some("export")),
        ev("c", "export_done", 100, None),
        // Resurrected: away for ~60 days, back yesterday.
        ev("r", "$pageview", 60 * 1440, None),
        ev("r", "$pageview", 1440, None),
        // Regular user: no gap.
        ev("a", "$pageview", 2 * 1440, None),
    ]
    .iter()
    .enumerate()
    .map(|(i, e)| mk(e, i))
    .collect();
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let map = parse("features:\n  - key: export\n    name: Export\n    success: export_done\n")
        .map
        .unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    let t = b
        .task_times
        .iter()
        .find(|t| t.feature == "export")
        .expect("export task times");
    assert_eq!(t.tasks, 2, "the abandoned attempt is not a task: {t:?}");
    assert!((50.0..=310.0).contains(&t.median_secs), "{t:?}");
    assert!(t.p90_secs >= 240.0 && t.p90_secs <= 310.0, "{t:?}");
    assert_eq!(b.users.resurrected, 1);
}

#[tokio::test]
async fn weekly_retention_cohorts_count_returning_users() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    // Monday 3 weeks ago, noon: 10 users start; 5 come back the next week,
    // 2 the week after.
    let now = Utc::now();
    let monday = now.date_naive()
        - Duration::days(i64::from(
            chrono::Datelike::weekday(&now.date_naive()).num_days_from_monday(),
        ));
    let start = monday - Duration::days(21);
    let at = |days: i64| {
        (now - (start + Duration::days(days))
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc())
        .num_minutes()
    };
    let users: Vec<String> = (0..10).map(|i| format!("r{i}")).collect();
    let mut evs = Vec::new();
    for (i, u) in users.iter().enumerate() {
        let mut add = |d: i64| {
            evs.push(mk(
                &E {
                    user: u,
                    role: "member",
                    session: u,
                    event: "$pageview",
                    mins_ago: at(d),
                    feature: None,
                    release: "v1",
                    props: "{}",
                },
                evs.len(),
            ))
        };
        add(0);
        if i < 5 {
            add(8);
        }
        if i < 2 {
            add(15);
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    let c = b
        .retention
        .iter()
        .find(|r| r.week == start.to_string())
        .expect("the cohort");
    assert_eq!(c.users, 10, "{:?}", b.retention);
    assert_eq!(c.weeks.len(), 3, "three full weeks have started since");
    assert!(
        (c.weeks[0] - 0.5).abs() < 1e-9 && (c.weeks[1] - 0.2).abs() < 1e-9,
        "{c:?}"
    );
    assert_eq!(c.weeks[2], 0.0);
}

/// Users are identified people, a bounce is a session with one interaction
/// at most, and a returning user is not a new cohort (docs/kpis.md).
#[tokio::test]
async fn users_bounces_and_cohorts_are_counted_as_documented() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let e = |user, session, event, mins_ago| E {
        user,
        role: "member",
        session,
        event,
        mins_ago,
        feature: None,
        release: "v1",
        props: "{}",
    };
    let rows = [
        // alice first logged in 120 days ago and is back today
        e("alice", "old", "$identify", 120 * 24 * 60),
        e("alice", "old", "$pageview", 120 * 24 * 60),
        e("alice", "s1", "$identify", 30),
        e("alice", "s1", "$pageview", 30),
        e("alice", "s1", "$click", 25),
        // bob is new: one page, then only scroll and vitals: a bounce
        e("bob", "s2", "$identify", 20),
        e("bob", "s2", "$pageview", 20),
        e("bob", "s2", "$scroll", 15),
        e("bob", "s2", "$vital", 14),
        // a visitor who never logged in
        e("anon-1", "s3", "$pageview", 10),
    ];
    let evs: Vec<_> = rows.iter().enumerate().map(|(i, e)| mk(e, i)).collect();
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();

    assert_eq!(
        (b.users.dau, b.users.mau),
        (2, 2),
        "anonymous visitors are not users"
    );
    assert_eq!(b.users.anonymous, 1);
    assert_eq!(b.sessions.count, 3);
    assert!(
        (b.sessions.bounce_rate - 2.0 / 3.0).abs() < 1e-9,
        "bob's and the visitor's sessions bounce: {}",
        b.sessions.bounce_rate
    );
    let newest = b.retention.last().unwrap();
    assert_eq!(newest.users, 1, "only bob is new this week, not alice");
}

/// The team's own users (`identify(id, { internal: true })`) are left out
/// of the board unless asked for.
#[tokio::test]
async fn internal_users_are_left_out_unless_included() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let e = |user, event, props| E {
        user,
        role: "member",
        session: user,
        event,
        mins_ago: 10,
        feature: None,
        release: "v1",
        props,
    };
    let rows = [
        e("customer", "$identify", "{}"),
        e("customer", "$pageview", "{}"),
        e("dev", "$identify", r#"{"$internal":true}"#),
        e("dev", "$pageview", r#"{"$internal":true}"#),
    ];
    let evs: Vec<_> = rows.iter().enumerate().map(|(i, e)| mk(e, i)).collect();
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    assert_eq!(b.users.mau, 1, "the team is left out by default");
    let all = Filters {
        internal: true,
        ..Filters::default()
    };
    let b = board(&q, "p", &all, None).await.unwrap();
    assert_eq!(b.users.mau, 2);
}

/// Days are the project's days (D-017): an event at 00:30 UTC belongs to the
/// day before in Los Angeles.
#[tokio::test]
async fn days_follow_the_project_time_zone() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let now = Utc::now();
    let mut at = now.date_naive().and_hms_opt(0, 30, 0).unwrap().and_utc();
    if at > now {
        at -= Duration::days(1);
    }
    let e = E {
        user: "tz",
        role: "member",
        session: "tz",
        event: "$pageview",
        mins_ago: (now - at).num_minutes(),
        feature: None,
        release: "v1",
        props: "{}",
    };
    w.append(&[mk(&e, 0)]).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let day = |tz: &str| Filters {
        tz: tz.into(),
        ..Filters::default()
    };
    let utc = board(&q, "p", &day("UTC"), None).await.unwrap();
    let la = board(&q, "p", &day("America/Los_Angeles"), None)
        .await
        .unwrap();
    let paris = board(&q, "p", &day("Europe/Paris"), None).await.unwrap();
    let d = |b: &openlens_engine::Board| {
        b.dau_series
            .iter()
            .map(|(d, _)| d.clone())
            .collect::<Vec<_>>()
    };
    let utc_day = at.date_naive();
    assert_eq!(d(&utc), vec![utc_day.to_string()]);
    assert_eq!(d(&la), vec![(utc_day - Duration::days(1)).to_string()]);
    assert_eq!(d(&paris), vec![utc_day.to_string()]);
    // A name that is not a time zone is refused, not interpolated.
    assert!(board(&q, "p", &day("x'; DROP"), None).await.is_err());
}

/// By environment, the breakdown drops the environment filter and shows
/// every environment side by side (regression: #310 bound it as `$env`).
#[tokio::test]
async fn breakdown_by_environment_shows_every_environment() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let e = E {
        user: "u1",
        role: "admin",
        session: "s1",
        event: "$pageview",
        mins_ago: 5,
        feature: None,
        release: "1.0",
        props: "{}",
    };
    let mut staging = mk(&e, 1);
    staging.environment = "staging".into();
    w.append(&[mk(&e, 0), staging]).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let rows = openlens_engine::breakdown(&q, "p", None, &Filters::default(), "environment")
        .await
        .unwrap();
    let mut envs: Vec<Option<String>> = rows.into_iter().map(|r| r.value).collect();
    envs.sort();
    assert_eq!(envs, [Some("production".into()), Some("staging".into())]);
}

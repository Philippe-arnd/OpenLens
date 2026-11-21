//! Integration tests for `openlens-engine`: board numbers on a known dataset.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Utc};
use openlens_engine::{Filters, board, recent_events};
use openlens_event::Event;
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
    let b = board(&q, "p", &Filters::default()).await.unwrap();
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
    let all = board(&q, "p", &Filters::default()).await.unwrap();
    let admins = board(
        &q,
        "p",
        &Filters {
            role: Some("admin".into()),
            ..Filters::default()
        },
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
    let b = board(&q, "p", &f).await.unwrap();
    assert_eq!(b.users.mau, 0);
    let bad = Filters {
        role: Some("a\nb".into()),
        ..Filters::default()
    };
    assert!(board(&q, "p", &bad).await.is_err());
}

#[tokio::test]
async fn recent_events_are_newest_first() {
    let (_d, q) = dataset();
    let rows = recent_events(&q, "p", &Filters::default(), None, 5)
        .await
        .unwrap();
    assert_eq!(rows.len(), 5);
    assert!(rows[0].timestamp >= rows[4].timestamp);
}

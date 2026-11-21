//! Integration tests for `openlens-query`.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Utc};
use openlens_event::Event;
use openlens_query::{Limits, QueryEngine};
use openlens_storage::{FlushPolicy, Writer};

fn ev(project: &str, role: &str, days_ago: i64) -> Event {
    let ts = Utc::now() - Duration::days(days_ago);
    Event {
        uuid: format!("{project}-{role}-{days_ago}-{}", rand_suffix()),
        project: project.into(),
        event: "$pageview".into(),
        distinct_id: format!("u-{role}"),
        session_id: None,
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some("/".into()),
        feature: None,
        role: Some(role.into()),
        account: None,
        plan: None,
        environment: "production".into(),
        release: None,
        properties: "{}".into(),
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn seeded() -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(&[
        ev("a", "admin", 0),
        ev("a", "admin", 1),
        ev("a", "member", 0),
        ev("b", "admin", 0),
    ])
    .unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

#[tokio::test]
async fn queries_see_only_their_own_project() {
    let (_d, q) = seeded();
    let a = q
        .sql("a", "SELECT count(*) AS n FROM events")
        .await
        .unwrap();
    let b = q
        .sql("b", "SELECT count(*) AS n FROM events")
        .await
        .unwrap();
    assert!(a.rows[0]["n"].as_i64() > b.rows[0]["n"].as_i64());
    assert_eq!(b.rows[0]["n"], 1);
}

#[tokio::test]
async fn group_by_role_and_partition_columns_work() {
    let (_d, q) = seeded();
    let r = q
        .sql("a", "SELECT role, count(*) AS n FROM events WHERE date >= '2000-01-01' GROUP BY role ORDER BY n DESC")
        .await
        .unwrap();
    assert_eq!(r.columns, ["role", "n"]);
    assert_eq!(r.rows[0]["role"], "admin");
}

#[tokio::test]
async fn writes_ddl_and_file_access_are_refused() {
    let (_d, q) = seeded();
    for sql in [
        "CREATE EXTERNAL TABLE x STORED AS CSV LOCATION '/etc/passwd'",
        "INSERT INTO events SELECT * FROM events",
        "DROP TABLE events",
        "SET datafusion.execution.batch_size = 1",
        "SELECT * FROM '/etc/passwd'",
    ] {
        assert!(q.sql("a", sql).await.is_err(), "{sql} should be refused");
    }
}

#[tokio::test]
async fn row_cap_truncates_and_says_so() {
    let (_d, _) = seeded();
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let evs: Vec<_> = (0..50).map(|i| ev("a", &format!("r{i}"), 0)).collect();
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(
        dir.path(),
        Limits {
            max_rows: 10,
            ..Limits::default()
        },
    );
    let r = q.sql("a", "SELECT * FROM events").await.unwrap();
    assert_eq!(r.rows.len(), 10);
    assert!(r.truncated);
}

#[tokio::test]
async fn empty_project_returns_zero_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let r = q
        .sql("fresh", "SELECT count(*) AS n FROM events")
        .await
        .unwrap();
    assert_eq!(r.rows[0]["n"], 0);
}

#[tokio::test]
async fn invalid_project_ids_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    assert!(q.sql("../x", "SELECT 1").await.is_err());
}

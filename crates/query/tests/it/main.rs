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
        flags: None,
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

#[tokio::test]
async fn snapshots_are_per_project_bounded_and_read_only() {
    let (_d, q) = seeded();
    let s = q
        .snapshot(
            "a",
            "snap",
            "SELECT * FROM events WHERE role = 'admin'",
            100,
        )
        .await
        .unwrap()
        .unwrap();
    let n = s.sql("a", "SELECT count(*) AS n FROM snap").await.unwrap();
    assert_eq!(n.rows[0]["n"], 2);
    // Other projects do not see it; the original engine is unchanged.
    assert!(s.sql("b", "SELECT count(*) AS n FROM snap").await.is_err());
    assert!(q.sql("a", "SELECT count(*) AS n FROM snap").await.is_err());
    // Too many rows: no snapshot, the caller falls back.
    assert!(
        q.snapshot("a", "snap", "SELECT * FROM events", 1)
            .await
            .unwrap()
            .is_none()
    );
    // Still read-only, and names are restricted.
    assert!(s.sql("a", "DROP TABLE snap").await.is_err());
    assert!(q.snapshot("a", "x; drop", "SELECT 1", 10).await.is_err());
}

#[tokio::test]
async fn anonymous_events_before_identify_belong_to_the_user() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut before = ev("a", "user", 0);
    before.distinct_id = "anon-1".into();
    let mut identify = ev("a", "user", 0);
    identify.event = "$identify".into();
    identify.distinct_id = "alice".into();
    identify.properties = r#"{"$anon_distinct_id":"anon-1","$lib":"web"}"#.into();
    let mut after = ev("a", "user", 0);
    after.distinct_id = "alice".into();
    let mut stranger = ev("a", "user", 0);
    stranger.distinct_id = "anon-2".into();
    w.append(&[before, identify, after, stranger]).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());

    let r = q
        .sql("a", "SELECT distinct_id, count(*) AS n FROM events GROUP BY distinct_id ORDER BY distinct_id")
        .await
        .unwrap();
    let got: Vec<(String, i64)> = r
        .rows
        .iter()
        .map(|row| {
            (
                row["distinct_id"].as_str().unwrap().to_owned(),
                row["n"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(got, vec![("alice".into(), 3), ("anon-2".into(), 1)]);

    // The stored ids stay available, and the date column survives the view.
    let raw = q
        .sql(
            "a",
            "SELECT count(DISTINCT distinct_id) AS n FROM raw_events",
        )
        .await
        .unwrap();
    assert_eq!(raw.rows[0]["n"], 3);
    let persons = q.sql("a", "SELECT person FROM persons").await.unwrap();
    assert_eq!(persons.rows.len(), 1);
    assert_eq!(persons.rows[0]["person"], "alice");
    let star = q
        .sql("a", "SELECT * FROM events WHERE date >= '2000-01-01'")
        .await
        .unwrap();
    assert_eq!(star.rows.len(), 4);
    assert_eq!(star.columns.last().map(String::as_str), Some("date"));
}

/// Regression: DataFusion caches directory listings forever by default, so
/// one engine (one shared runtime) stopped seeing new segments after its
/// first query, until a restart.
#[tokio::test]
async fn new_segments_are_seen_by_the_same_engine() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(&[ev("a", "admin", 0)]).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let count = |q: QueryEngine| async move {
        q.sql("a", "SELECT count(*) AS n FROM raw_events")
            .await
            .unwrap()
            .rows[0]["n"]
            .as_i64()
    };
    assert_eq!(count(q.clone()).await, Some(1));
    // A new segment in the same day, then one in a new day.
    w.append(&[ev("a", "member", 0)]).unwrap();
    w.flush().unwrap();
    assert_eq!(count(q.clone()).await, Some(2));
    w.append(&[ev("a", "member", 3)]).unwrap();
    w.flush().unwrap();
    assert_eq!(count(q.clone()).await, Some(3));
}

#[tokio::test]
async fn aggregate_sql_hides_people_and_small_groups() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut rows = Vec::new();
    for i in 0..6 {
        let mut e = ev("c", "member", 0);
        e.uuid = format!("c-{i}");
        e.distinct_id = format!("person-{i}");
        e.session_id = Some(format!("s-{i}"));
        e.feature = Some("export".into());
        e.url = Some("https://app.test/x".into());
        rows.push(e);
    }
    let mut lone = ev("c", "member", 0);
    lone.uuid = "c-lone".into();
    lone.distinct_id = "person-lone".into();
    lone.feature = Some("rare".into());
    rows.push(lone);
    let mut identify = ev("c", "member", 0);
    identify.uuid = "c-identify".into();
    identify.event = "$identify".into();
    identify.distinct_id = "person-0".into();
    identify.properties = r#"{"$anon_distinct_id":"anon-0"}"#.into();
    rows.push(identify);
    w.append(&rows).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let key = b"project key";

    for leak in [
        "SELECT *, count(*) OVER () FROM events LIMIT 5",
        "SELECT count(*), array_agg(person) FROM persons",
        "SELECT anon, person, count(*) FROM person_aliases GROUP BY 1, 2",
        "SELECT distinct_id, count(*) FROM events GROUP BY 1",
        "SELECT user_key FROM events",
        "SELECT count(*), (SELECT min(user_key) FROM events) FROM events",
        "SELECT count(*) FROM events UNION ALL SELECT count(*) FROM events",
    ] {
        assert!(
            q.sql_aggregate("c", "events", key, leak).await.is_err(),
            "{leak}"
        );
    }
    // A group of one person is dropped, even when its key is chosen.
    let per_user = q
        .sql_aggregate(
            "c",
            "events",
            key,
            "SELECT user_key, count(*) AS n FROM events GROUP BY 1",
        )
        .await
        .unwrap();
    assert!(per_user.rows.is_empty());

    let r = q
        .sql_aggregate(
            "c",
            "events",
            key,
            "SELECT feature, count(DISTINCT user_key) AS users FROM events GROUP BY 1",
        )
        .await
        .unwrap();
    let text = serde_json::to_string(&r.rows).unwrap();
    assert!(text.contains("export"));
    assert!(!text.contains("rare") && !text.contains("person-"));
    assert_eq!(r.columns, ["feature", "users"]);

    // The feature map's projection is the `events` the query sees.
    let mapped = "(SELECT event, timestamp, path, 'mapped' AS feature, role, plan, release, \
                  environment, flags, date, distinct_id, session_id FROM events) AS events";
    let r = q
        .sql_aggregate(
            "c",
            mapped,
            key,
            "SELECT feature, count(*) AS n FROM events GROUP BY 1",
        )
        .await
        .unwrap();
    assert!(serde_json::to_string(&r.rows).unwrap().contains("mapped"));
    let full = q
        .sql_over("c", mapped, "SELECT DISTINCT feature FROM events")
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&full.rows)
            .unwrap()
            .contains("mapped")
    );
}

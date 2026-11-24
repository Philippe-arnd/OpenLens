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
async fn concurrent_shared_snapshots_are_built_once() {
    let (_d, q) = seeded();
    let mut role = openlens_sqlgen::Params::new();
    let unused = role.set("role", "admin").unwrap();
    assert_eq!(unused, "$role");
    let mut other = openlens_sqlgen::Params::new();
    other.set("role", "viewer").unwrap();
    let sql = "SELECT * FROM events";
    // Different values the SQL does not read: still one snapshot.
    let (qa, qb) = (q.with_params(&role), q.with_params(&other));
    let before = q.snapshots_built();
    let (a, b, c, d) = tokio::join!(
        qa.shared_snapshot("a", "snap", sql, 100, 7),
        qb.shared_snapshot("a", "snap", sql, 100, 7),
        qa.shared_snapshot("a", "snap", sql, 100, 7),
        qb.shared_snapshot("a", "snap", sql, 100, 7),
    );
    for s in [a, b, c, d] {
        let s = s.unwrap().unwrap();
        let n = s.sql("a", "SELECT count(*) AS n FROM snap").await.unwrap();
        assert!(n.rows[0]["n"].as_i64().unwrap() > 0);
    }
    assert_eq!(q.snapshots_built(), before + 1, "one build for four callers");
    // A new data generation builds a new one.
    q.shared_snapshot("a", "snap", sql, 100, 8)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(q.snapshots_built(), before + 2);
    // Too large: remembered, not retried at once.
    assert!(q.shared_snapshot("a", "big", sql, 1, 7).await.unwrap().is_none());
    assert!(q.shared_snapshot("a", "big", sql, 1, 7).await.unwrap().is_none());
    assert_eq!(q.snapshots_built(), before + 2);
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
        // NSEC-01: a column renamed to `user_key` under FROM must not be
        // trusted as the real pseudonym.
        "SELECT array_agg(path) FROM (SELECT path, CAST(timestamp AS VARCHAR) AS user_key \
         FROM events WHERE feature = 'rare') t",
        "SELECT array_agg(path) FROM (SELECT path, session_key AS user_key FROM events \
         WHERE feature = 'rare') t",
        // NSEC-01: the real pseudonym must not be listed out either.
        "SELECT role, array_agg(DISTINCT user_key) FROM events GROUP BY 1",
        "SELECT role, string_agg(user_key, ',') FROM events GROUP BY 1",
        "SELECT min(user_key) FROM events",
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

/// Rollups (#322) answer like the raw events in any time zone, people
/// merged with their anonymous visits included, and go away when someone
/// is erased.
#[tokio::test]
async fn rollups_match_raw_queries_in_every_zone() {
    use chrono::{DateTime, NaiveDate};
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let at = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
    let mut evs = Vec::new();
    // Around two UTC midnights, so Paris (+1) and Kolkata (+5:30) days
    // cut the UTC days in other places.
    let times = [
        "2026-03-01T17:20:00Z",
        "2026-03-01T18:40:00Z",
        "2026-03-01T22:50:00Z",
        "2026-03-01T23:10:00Z",
        "2026-03-02T00:05:00Z",
        "2026-03-02T18:29:00Z",
        "2026-03-02T18:31:00Z",
        "2026-03-02T23:59:00Z",
    ];
    for (i, t) in times.iter().enumerate() {
        for (j, who) in ["anon-1", "bob", "carol"].iter().enumerate() {
            if (i + j) % 3 == 2 {
                continue;
            }
            let mut e = ev("a", if j == 1 { "admin" } else { "member" }, 0);
            e.distinct_id = (*who).into();
            e.timestamp = at(t);
            e.received_at = at(t);
            e.event = if i % 4 == 3 {
                "$error".into()
            } else {
                "$pageview".into()
            };
            e.feature = (i % 2 == 0).then(|| "billing".into());
            evs.push(e);
        }
    }
    let mut identify = ev("a", "member", 0);
    identify.event = "$identify".into();
    identify.distinct_id = "alice".into();
    identify.timestamp = at("2026-03-02T00:06:00Z");
    identify.received_at = identify.timestamp;
    identify.properties = r#"{"$anon_distinct_id":"anon-1"}"#.into();
    evs.push(identify);
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());

    let days = ["2026-03-01", "2026-03-02"];
    for day in days {
        let batches = q.rollup_day("a", day).await.unwrap();
        let named: Vec<(&str, _)> = batches.iter().map(|(n, b)| (*n, b.clone())).collect();
        let d = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
        let g = openlens_storage::rollup::generation();
        assert!(openlens_storage::rollup::write(dir.path(), "a", d, g, &named).unwrap());
    }

    let rows = |r: openlens_query::QueryResult| -> Vec<String> {
        r.rows
            .iter()
            .map(|row| serde_json::Value::Object(row.clone()).to_string())
            .collect()
    };
    for zone in ["UTC", "Europe/Paris", "Asia/Kolkata"] {
        let local = |col: &str| {
            format!(
                "date_trunc('day', to_local_time(arrow_cast({col}, 'Timestamp(Microsecond, Some(\"{zone}\"))')))"
            )
        };
        let raw = q
            .sql(
                "a",
                &format!(
                    "SELECT {d} AS d, count(DISTINCT distinct_id) AS dau, count(*) AS n, \
                     sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors \
                     FROM events GROUP BY 1 ORDER BY 1",
                    d = local("timestamp")
                ),
            )
            .await
            .unwrap();
        let rolled = q
            .sql(
                "a",
                &format!(
                    "SELECT {d} AS d, count(DISTINCT person) AS dau, sum(events) AS n, \
                     sum(errors) AS errors FROM person_q GROUP BY 1 ORDER BY 1",
                    d = local("bucket")
                ),
            )
            .await
            .unwrap();
        assert_eq!(rows(raw), rows(rolled), "people per day in {zone}");
        let raw = q
            .sql(
                "a",
                &format!(
                    "SELECT {d} AS d, event, feature, count(*) AS n FROM events \
                     GROUP BY 1, 2, 3 ORDER BY 1, 2, 3",
                    d = local("timestamp")
                ),
            )
            .await
            .unwrap();
        let rolled = q
            .sql(
                "a",
                &format!(
                    "SELECT {d} AS d, event, feature, sum(events) AS n FROM metric_q \
                     GROUP BY 1, 2, 3 ORDER BY 1, 2, 3",
                    d = local("bucket")
                ),
            )
            .await
            .unwrap();
        assert_eq!(rows(raw), rows(rolled), "events per day in {zone}");
    }
    // The anonymous visits count as alice's.
    let people = q
        .sql("a", "SELECT DISTINCT person FROM person_q ORDER BY person")
        .await
        .unwrap();
    assert_eq!(
        rows(people),
        [
            r#"{"person":"alice"}"#,
            r#"{"person":"bob"}"#,
            r#"{"person":"carol"}"#
        ]
    );

    // Erasing someone drops the rollups, to be rebuilt without them.
    let gone: std::collections::HashSet<String> = ["bob".to_owned()].into();
    assert!(openlens_storage::erase(dir.path(), "a", &gone).unwrap() > 0);
    let d = NaiveDate::parse_from_str(days[0], "%Y-%m-%d").unwrap();
    assert!(!openlens_storage::rollup::has(dir.path(), "a", d).unwrap());
    let empty = q
        .sql("a", "SELECT count(*) AS n FROM person_q")
        .await
        .unwrap();
    assert_eq!(empty.rows[0]["n"], 0);
}

/// The page rollups (#324) hold the click map, the hotspots and the scroll
/// depth of a day as the raw events give them.
#[tokio::test]
async fn page_rollups_match_the_raw_clicks_and_scrolls() {
    use chrono::{DateTime, NaiveDate};
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let t: DateTime<Utc> = "2026-03-01T10:00:00Z".parse().unwrap();
    let mut evs = Vec::new();
    for (i, (x, y, text, rage)) in [
        (0.10, 40, "Pay", false),
        (0.11, 45, "Pay", true),
        (0.90, 400, "Help", false),
    ]
    .iter()
    .enumerate()
    {
        let mut e = ev("a", "member", 0);
        e.event = "$click".into();
        e.timestamp = t + Duration::seconds(i as i64);
        e.received_at = e.timestamp;
        e.path = Some("/bill".into());
        e.properties =
            format!(r#"{{"$text":"{text}","$x":{x},"$y":{y},"$vw":1000,"$rage":{rage}}}"#);
        evs.push(e);
    }
    for (s, d) in [("s1", 35), ("s1", 72), ("s2", 100)] {
        let mut e = ev("a", "member", 0);
        e.event = "$scroll".into();
        e.session_id = Some(s.into());
        e.timestamp = t;
        e.received_at = t;
        e.path = Some("/bill".into());
        e.properties = format!(r#"{{"$depth":{d}}}"#);
        evs.push(e);
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let batches = q.rollup_day("a", "2026-03-01").await.unwrap();
    let d = NaiveDate::parse_from_str("2026-03-01", "%Y-%m-%d").unwrap();
    let g = openlens_storage::rollup::generation();
    assert!(openlens_storage::rollup::write(dir.path(), "a", d, g, &batches).unwrap());

    let one = |sql: &'static str| {
        let q = q.clone();
        async move {
            let r = q.sql("a", sql).await.unwrap();
            serde_json::Value::Array(r.rows.into_iter().map(serde_json::Value::Object).collect())
                .to_string()
        }
    };
    assert_eq!(
        one("SELECT cx, cy, sum(clicks) AS n, sum(rages) AS r, sum(vw_sum) / sum(vw_n) AS vw FROM click_q GROUP BY 1, 2 ORDER BY 1, 2").await,
        r#"[{"cx":5,"cy":2,"n":2,"r":1,"vw":1000.0},{"cx":45,"cy":20,"n":1,"r":0,"vw":1000.0}]"#
    );
    assert_eq!(
        one("SELECT label, sum(clicks) AS n, sum(rages) AS r FROM label_q GROUP BY 1 ORDER BY 1")
            .await,
        r#"[{"label":"Help","n":1,"r":0},{"label":"Pay","n":2,"r":1}]"#
    );
    assert_eq!(
        one("SELECT depth, sum(sessions) AS n FROM scroll_q GROUP BY 1 ORDER BY 1").await,
        r#"[{"depth":7,"n":1},{"depth":10,"n":1}]"#
    );
}

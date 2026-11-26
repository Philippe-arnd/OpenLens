//! The daily per-session rollup (#493, D-041): a session re-assembled from
//! its days gives what the board computes from the events, a person erased
//! leaves it, and a day lacking only this table gets only this table.

use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, Utc};
use openlens_query::{DayFeatures, INTERACTION, QueryEngine};
use openlens_storage::rollup;

const DAYS: [&str; 2] = ["2026-03-01", "2026-03-02"];

fn features() -> DayFeatures {
    DayFeatures {
        feature: "feature".into(),
        successes: vec!["paid".into()],
        map_version: Some(2),
    }
}

/// `(session, person, release, time, event)`, with `None` for an event
/// outside any session.
type Row = (
    Option<&'static str>,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const EVENTS: [Row; 16] = [
    // Crosses midnight: interactions, an error and a task on both sides.
    (Some("s-night"), "alice", "v1", "2026-03-01T23:50:00Z", "$pageview"),
    (Some("s-night"), "alice", "v1", "2026-03-01T23:52:00Z", "$error"),
    (Some("s-night"), "alice", "v1", "2026-03-01T23:55:00Z", "paid"),
    (Some("s-night"), "alice", "v1", "2026-03-02T00:05:00Z", "$click"),
    (Some("s-night"), "alice", "v1", "2026-03-02T00:10:00Z", "paid"),
    // A bounce: one page view, then only a scroll.
    (Some("s-bounce"), "bob", "v1", "2026-03-01T10:00:00Z", "$pageview"),
    (Some("s-bounce"), "bob", "v1", "2026-03-01T10:01:00Z", "$scroll"),
    // Over a deploy: two releases in one session.
    (Some("s-deploy"), "bob", "v1", "2026-03-02T09:00:00Z", "$pageview"),
    (Some("s-deploy"), "bob", "v2", "2026-03-02T09:30:00Z", "$pageview"),
    (Some("s-deploy"), "bob", "v2", "2026-03-02T09:40:00Z", "$error"),
    // An anonymous visitor identified as alice the next day.
    (Some("s-anon"), "anon-1", "v1", "2026-03-01T08:00:00Z", "$pageview"),
    (Some("s-anon"), "anon-1", "v1", "2026-03-01T08:05:00Z", "signup"),
    (Some("s-carol"), "carol", "v2", "2026-03-02T12:00:00Z", "$pageview"),
    (Some("s-carol"), "carol", "v2", "2026-03-02T12:01:00Z", "paid"),
    // Outside any session: not in the table, as not in the board.
    (None, "carol", "v2", "2026-03-02T13:00:00Z", "$pageview"),
    (None, "carol", "v2", "2026-03-02T13:01:00Z", "paid"),
];

fn seed(root: &std::path::Path) {
    let (mut w, _) = openlens_storage::Writer::open(root, Default::default()).unwrap();
    let mut evs = Vec::new();
    for (i, (session, who, release, at, event)) in EVENTS.into_iter().enumerate() {
        let mut e = crate::ev("a", "member", 0);
        e.uuid = format!("e-{i}");
        e.distinct_id = who.into();
        e.session_id = session.map(Into::into);
        e.timestamp = at.parse::<DateTime<Utc>>().unwrap();
        e.received_at = e.timestamp;
        e.release = Some(release.into());
        e.event = event.into();
        evs.push(e);
    }
    let mut identify = crate::ev("a", "member", 0);
    identify.uuid = "identify".into();
    identify.event = "$identify".into();
    identify.distinct_id = "alice".into();
    identify.session_id = Some("s-night".into());
    identify.release = Some("v1".into());
    identify.timestamp = "2026-03-02T00:01:00Z".parse().unwrap();
    identify.received_at = identify.timestamp;
    identify.properties = r#"{"$anon_distinct_id":"anon-1"}"#.into();
    evs.push(identify);
    w.append(&evs).unwrap();
    w.flush().unwrap();
}

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// Roll up the tables `days` lack, write them, and return their names.
async fn roll_up(q: &QueryEngine, root: &std::path::Path, days: &[&str]) -> Vec<&'static str> {
    let mut names = Vec::new();
    for d in days {
        let lacking = rollup::lacking(root, "a", day(d));
        let batches = q
            .rollup_day("a", d, Some(&lacking), &features())
            .await
            .unwrap();
        names.extend(batches.iter().map(|(n, _)| *n));
        let g = rollup::generation();
        assert!(rollup::write(root, "a", day(d), g, &batches).unwrap());
    }
    names
}

async fn rows(q: &QueryEngine, sql: &str) -> Vec<String> {
    q.sql("a", sql)
        .await
        .unwrap()
        .rows
        .into_iter()
        .map(|r| serde_json::Value::Object(r).to_string())
        .collect()
}

/// The board's per-session facts (`board_with`'s sessions and releases),
/// from the events.
fn raw_sessions(by: &str) -> String {
    format!(
        "SELECT {by} session_id, min(distinct_id) AS uid, \
         sum(CASE WHEN {INTERACTION} THEN 1 ELSE 0 END) AS n, \
         date_part('epoch', max(timestamp)) - date_part('epoch', min(timestamp)) AS secs, \
         sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors, \
         sum(CASE WHEN event = 'paid' THEN 1 ELSE 0 END) AS tasks \
         FROM events WHERE session_id IS NOT NULL GROUP BY {by} session_id"
    )
}

/// The same facts, re-assembled from `session_day` across days.
fn rolled_sessions(by: &str) -> String {
    format!(
        "SELECT {by} session_id, min(person) AS uid, sum(interactions) AS n, \
         date_part('epoch', max(last_ts)) - date_part('epoch', min(first_ts)) AS secs, \
         sum(errors) AS errors, sum(tasks) AS tasks \
         FROM session_day GROUP BY {by} session_id"
    )
}

/// The board's sessions block over a per-session subquery.
fn board(sessions: &str) -> String {
    format!(
        "SELECT count(*) AS sessions, avg(CASE WHEN n > 1 THEN secs END) AS avg_secs, \
         avg(CASE WHEN n <= 1 THEN 1.0 ELSE 0.0 END) AS bounce_rate, \
         avg(CASE WHEN errors > 0 THEN 1.0 ELSE 0.0 END) AS error_rate, \
         sum(tasks) AS tasks FROM ({sessions}) s"
    )
}

async fn assert_matches_events(q: &QueryEngine) {
    for by in ["", "release,"] {
        let (raw, rolled) = (raw_sessions(by), rolled_sessions(by));
        let order = |s: &str| format!("{s} ORDER BY {by} session_id");
        let expected = rows(q, &order(&raw)).await;
        assert!(expected.len() > 1, "{raw}");
        assert_eq!(expected, rows(q, &order(&rolled)).await, "{rolled}");
        assert_eq!(
            rows(q, &board(&raw)).await,
            rows(q, &board(&rolled)).await,
            "{by}"
        );
    }
}

#[tokio::test]
async fn sessions_reassembled_from_their_days_match_the_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed(root);
    let q = QueryEngine::new(root, openlens_query::Limits::default());
    roll_up(&q, root, &DAYS).await;
    for d in DAYS {
        assert!(rollup::lacking(root, "a", day(d)).is_empty());
    }
    // The night session has a row on each day, and its parts differ.
    let night = rows(
        &q,
        "SELECT date, interactions, errors, tasks FROM session_day \
         WHERE session_id = 's-night' ORDER BY date",
    )
    .await;
    assert_eq!(night.len(), 2);
    assert_ne!(night[0], night[1]);
    assert_matches_events(&q).await;
    // The anonymous visit is alice's, as in every query; no session-less row.
    let people = rows(&q, "SELECT DISTINCT person FROM session_day ORDER BY 1").await;
    assert_eq!(
        people,
        [
            r#"{"person":"alice"}"#,
            r#"{"person":"bob"}"#,
            r#"{"person":"carol"}"#
        ]
    );
    let nulls = rows(
        &q,
        "SELECT count(*) AS n FROM session_day WHERE session_id IS NULL",
    )
    .await;
    assert_eq!(nulls, [r#"{"n":0}"#]);
    let version = rows(&q, "SELECT DISTINCT map_version FROM session_day").await;
    assert_eq!(version, [r#"{"map_version":2}"#]);

    // A day rolled up before `session_day` existed gets only that table.
    let all = "SELECT * FROM session_day ORDER BY date, session_id, person, release, role";
    let before = rows(&q, all).await;
    let dir1 = rollup::rollups_dir(root, "a")
        .unwrap()
        .join(format!("date={}", DAYS[1]));
    let metric = std::fs::read(dir1.join("metric_q.parquet")).unwrap();
    std::fs::remove_file(dir1.join("session_day.parquet")).unwrap();
    assert!(!rollup::has(root, "a", day(DAYS[1])).unwrap());
    assert_eq!(roll_up(&q, root, &DAYS).await, ["session_day"]);
    assert_eq!(
        std::fs::read(dir1.join("metric_q.parquet")).unwrap(),
        metric
    );
    assert_eq!(rows(&q, all).await, before);

    // The first day is past retention: erasing bob trims its rows there;
    // the second day is dropped and rolled up again without him.
    std::fs::remove_dir_all(
        openlens_storage::project_dir(root, "a")
            .unwrap()
            .join(format!("date={}", DAYS[0])),
    )
    .unwrap();
    let bob: HashSet<String> = ["bob".to_owned()].into();
    openlens_storage::erase(root, "a", &bob).unwrap();
    assert!(rollup::has(root, "a", day(DAYS[0])).unwrap());
    assert!(!rollup::has(root, "a", day(DAYS[1])).unwrap());
    roll_up(&q, root, &DAYS[1..]).await;
    let others = before
        .iter()
        .filter(|r| !r.contains(r#""person":"bob""#))
        .cloned()
        .collect::<Vec<_>>();
    assert!(others.len() < before.len());
    assert_eq!(rows(&q, all).await, others);
}

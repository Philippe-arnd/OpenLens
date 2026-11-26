//! The daily per-person rollup (#451, D-038): what it holds matches the
//! raw events, a person erased leaves it, and rolling a day up again
//! changes nothing.

use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, Utc};
use openlens_query::{DayFeatures, QueryEngine};
use openlens_storage::rollup;

const DAYS: [&str; 3] = ["2026-03-01", "2026-03-02", "2026-03-03"];

/// The feature lookup of the test: the event's own feature, else the
/// success event's, else a route, as a feature map resolves them.
const FEATURE: &str = "CASE WHEN feature IS NOT NULL THEN feature \
     WHEN event = 'paid' THEN 'billing' WHEN path LIKE '/bill%' THEN 'billing' ELSE NULL END";

fn features() -> DayFeatures {
    DayFeatures {
        feature: FEATURE.into(),
        successes: vec!["paid".into()],
        map_version: Some(3),
    }
}

/// Three days of people with features, roles, releases, sessions, errors
/// and tasks, plus an anonymous visitor later identified as alice.
fn seed(root: &std::path::Path) {
    let (mut w, _) = openlens_storage::Writer::open(root, Default::default()).unwrap();
    let mut evs = Vec::new();
    let people = [("anon-1", "member"), ("bob", "admin"), ("carol", "member")];
    for (d, day) in DAYS.iter().enumerate() {
        for i in 0..9usize {
            let (who, role) = people[(i + d) % 3];
            if d == 2 && who == "carol" {
                continue;
            }
            let mut e = crate::ev("a", role, 0);
            e.uuid = format!("{day}-{i}");
            e.distinct_id = who.into();
            e.session_id = Some(format!("{who}-{d}-{}", i % 2));
            e.timestamp = format!("{day}T{:02}:15:00Z", 6 + i)
                .parse::<DateTime<Utc>>()
                .unwrap();
            e.received_at = e.timestamp;
            e.release = Some(format!("v{}", 1 + (i % 2)));
            (e.event, e.path, e.feature) = match i % 5 {
                0 => ("$pageview".into(), Some("/billing".into()), None),
                1 => ("paid".into(), Some("/checkout".into()), None),
                2 => ("$error".into(), Some("/x".into()), Some("export".into())),
                3 => ("$feature".into(), Some("/".into()), Some("export".into())),
                _ => ("$pageview".into(), Some("/".into()), None),
            };
            evs.push(e);
        }
    }
    let mut identify = crate::ev("a", "member", 0);
    identify.uuid = "identify".into();
    identify.event = "$identify".into();
    identify.distinct_id = "alice".into();
    identify.timestamp = "2026-03-02T05:00:00Z".parse().unwrap();
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

/// Per UTC day, per feature, per role and release, and row by row: what
/// the rollup gives equals what the events give.
async fn assert_matches_events(q: &QueryEngine) {
    let pairs = [
        (
            "SELECT date, count(DISTINCT distinct_id) AS people, count(*) AS events, \
             sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors, \
             sum(CASE WHEN event = 'paid' THEN 1 ELSE 0 END) AS successes \
             FROM events GROUP BY 1 ORDER BY 1"
                .to_owned(),
            "SELECT date, count(DISTINCT person) AS people, sum(events) AS events, \
             sum(errors) AS errors, sum(successes) AS successes \
             FROM person_day GROUP BY 1 ORDER BY 1"
                .to_owned(),
        ),
        (
            format!(
                "SELECT {FEATURE} AS feature, count(DISTINCT distinct_id) AS users, \
                 count(DISTINCT concat(distinct_id, '|', date)) AS user_days, count(*) AS uses \
                 FROM events GROUP BY 1 ORDER BY 1"
            ),
            "SELECT feature, count(DISTINCT person) AS users, \
             count(DISTINCT concat(person, '|', date)) AS user_days, sum(events) AS uses \
             FROM person_day GROUP BY 1 ORDER BY 1"
                .to_owned(),
        ),
        (
            "SELECT role, release, count(DISTINCT distinct_id) AS users, count(*) AS events \
             FROM events GROUP BY 1, 2 ORDER BY 1, 2"
                .to_owned(),
            "SELECT role, release, count(DISTINCT person) AS users, sum(events) AS events \
             FROM person_day GROUP BY 1, 2 ORDER BY 1, 2"
                .to_owned(),
        ),
        (
            format!(
                "SELECT date, distinct_id AS person, release, role, {FEATURE} AS feature, \
                 count(DISTINCT session_id) AS sessions, min(timestamp) AS first_ts, \
                 max(timestamp) AS last_ts FROM events GROUP BY 1, 2, 3, 4, 5 \
                 ORDER BY 1, 2, 3, 4, 5"
            ),
            "SELECT date, person, release, role, feature, sessions, first_ts, last_ts \
             FROM person_day ORDER BY 1, 2, 3, 4, 5"
                .to_owned(),
        ),
    ];
    for (raw, rolled) in pairs {
        let expected = rows(q, &raw).await;
        assert!(expected.len() > 1, "{raw}");
        assert_eq!(expected, rows(q, rolled.as_str()).await, "{rolled}");
    }
}

#[tokio::test]
async fn person_day_matches_the_events_and_forgets_the_erased() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed(root);
    let q = QueryEngine::new(root, openlens_query::Limits::default());
    roll_up(&q, root, &DAYS).await;
    for d in DAYS {
        assert!(rollup::lacking(root, "a", day(d)).is_empty());
    }
    assert_matches_events(&q).await;
    // The anonymous visits count as alice's, as in every query.
    let people = rows(&q, "SELECT DISTINCT person FROM person_day ORDER BY 1").await;
    assert_eq!(
        people,
        [
            r#"{"person":"alice"}"#,
            r#"{"person":"bob"}"#,
            r#"{"person":"carol"}"#
        ]
    );
    let version = rows(&q, "SELECT DISTINCT map_version FROM person_day").await;
    assert_eq!(version, [r#"{"map_version":3}"#]);
    let all = "SELECT * FROM person_day ORDER BY date, person, release, role, feature";
    let before = rows(&q, all).await;

    // Rolling a day up again (all its tables) gives the same rows.
    let batches = q.rollup_day("a", DAYS[1], None, &features()).await.unwrap();
    let g = rollup::generation();
    assert!(rollup::write(root, "a", day(DAYS[1]), g, &batches).unwrap());
    assert_eq!(rows(&q, all).await, before);

    // A day rolled up before `person_day` existed gets only that table,
    // with the same rows, and keeps the others as they were.
    let dir1 = rollup::rollups_dir(root, "a")
        .unwrap()
        .join(format!("date={}", DAYS[0]));
    let metric = std::fs::read(dir1.join("metric_q.parquet")).unwrap();
    std::fs::remove_file(dir1.join("person_day.parquet")).unwrap();
    assert!(!rollup::has(root, "a", day(DAYS[0])).unwrap());
    assert_eq!(roll_up(&q, root, &DAYS).await, ["person_day"]);
    assert_eq!(
        std::fs::read(dir1.join("metric_q.parquet")).unwrap(),
        metric
    );
    assert_eq!(rows(&q, all).await, before);

    // The first day is past retention (its events are gone): erasing bob
    // trims its rollup; the other days are dropped, to be rolled up again.
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
    let left = rows(&q, "SELECT DISTINCT person FROM person_day ORDER BY 1").await;
    assert_eq!(left, [r#"{"person":"alice"}"#, r#"{"person":"carol"}"#]);
    let others = before
        .iter()
        .filter(|r| !r.contains(r#""person":"bob""#))
        .cloned()
        .collect::<Vec<_>>();
    assert!(others.len() < before.len());
    assert_eq!(rows(&q, all).await, others);
    // Done once: erasing again changes nothing.
    let after = rows(&q, all).await;
    openlens_storage::erase(root, "a", &bob).unwrap();
    assert!(rollup::has(root, "a", day(DAYS[1])).unwrap());
    assert_eq!(rows(&q, all).await, after);
}

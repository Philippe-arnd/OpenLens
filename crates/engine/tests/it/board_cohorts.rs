//! The board's insight cards, retention, resurrected users and journeys
//! read the people rows (`person_day`), the per-session rows
//! (`session_day`) and the first use of each feature per session
//! (`session_feature_day`) for closed days, and the events for the rest
//! (#452, D-059); success events and the first success of each journey
//! read `success_day` the same way (D-062): the numbers are the same as
//! from the events alone, in
//! UTC and in a zone off UTC, and a closed day without its events still
//! counts.

use chrono::{Duration, Utc};
use openlens_engine::{Board, Filters, board, rollups::day_features};
use openlens_event::Event;
use openlens_featuremap::{FeatureMap, parse};
use openlens_query::{Limits, QueryEngine};
use openlens_storage::{FlushPolicy, Writer, rollup};

fn map() -> FeatureMap {
    parse(
        "features:\n  - key: search\n    name: Search\n    entry: { routes: ['/search'] }\n\
         \x20 - key: export\n    name: Export\n\
         \x20 - key: billing\n    name: Billing\n    success: paid\n    entry: { routes: ['/pay'] }\n",
    )
    .map
    .unwrap()
}

fn event(who: &str, session: &str, event: &str, ts: chrono::DateTime<Utc>, path: &str) -> Event {
    let feature = match path {
        "/export" => Some("export".to_owned()),
        _ => None,
    };
    Event {
        uuid: format!("{who}-{session}-{event}-{}", ts.timestamp()),
        project: "p".into(),
        event: event.into(),
        distinct_id: who.into(),
        session_id: (event != "$identify").then(|| session.to_owned()),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some(path.into()),
        feature,
        role: Some(
            if who.ends_with('3') {
                "admin"
            } else {
                "member"
            }
            .into(),
        ),
        account: None,
        plan: None,
        environment: "production".into(),
        release: Some(
            if ts > Utc::now() - Duration::days(10) {
                "v2"
            } else {
                "v1"
            }
            .into(),
        ),
        flags: None,
        properties: "{}".into(),
    }
}

/// 24 people first seen 20 to 34 days ago, every other day: the even ones
/// stay (and use export in their first week, and pay on their first day),
/// the odd ones stop after a few days, last seen paying. One person comes
/// back after five weeks away. Everyone is identified today, an open day.
fn seed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let now = Utc::now();
    let at = |k: i64, hour: u32, minute: u32| {
        (now.date_naive() - Duration::days(k))
            .and_hms_opt(hour, minute, 0)
            .unwrap()
            .and_utc()
    };
    let mut evs = Vec::new();
    let mut people: Vec<String> = Vec::new();
    for i in 0..24i64 {
        let who = format!("p{i}");
        let start = 20 + i % 15;
        let stop = if i % 2 == 0 { 1 } else { start - 4 };
        let hour = 9 + (i % 5) as u32;
        let mut k = start;
        while k >= stop {
            let s = format!("{who}-{k}");
            evs.push(event(&who, &s, "$pageview", at(k, hour, 30), "/"));
            evs.push(event(&who, &s, "$click", at(k, hour, 32), "/search"));
            if i % 2 == 0 && k >= start - 7 {
                evs.push(event(&who, &s, "$click", at(k, hour, 34), "/export"));
            }
            if i % 2 == 0 || k == stop {
                evs.push(event(&who, &s, "$pageview", at(k, hour, 36), "/pay"));
            }
            if i % 2 == 0 && k == start {
                evs.push(event(&who, &s, "paid", at(k, hour, 38), "/pay"));
            }
            k -= 2;
        }
        people.push(who);
    }
    for k in [40, 3] {
        evs.push(event(
            "back",
            &format!("back-{k}"),
            "$pageview",
            at(k, 15, 30),
            "/",
        ));
    }
    people.push("back".into());
    for who in &people {
        evs.push(event(
            who,
            "id",
            "$identify",
            now - Duration::minutes(30),
            "/",
        ));
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    dir
}

/// Roll up every closed day, as the server does.
async fn roll_up(root: &std::path::Path) -> Vec<chrono::NaiveDate> {
    let q = QueryEngine::new(root, Limits::default());
    let m = map();
    let features = day_features(Some((&m, 1)));
    let days = rollup::missing(root, "p", Utc::now()).unwrap();
    assert!(days.len() >= 20, "most of the data is on closed days");
    for d in &days {
        let label = d.format("%Y-%m-%d").to_string();
        let batches = q.rollup_day("p", &label, None, &features).await.unwrap();
        assert!(rollup::write(root, "p", *d, rollup::generation(), &batches).unwrap());
    }
    days
}

/// The cohort blocks of a board, in a stable order.
fn cohorts(b: &Board) -> String {
    let mut cards: Vec<String> = b
        .insights
        .iter()
        .map(|c| format!("{} {} | {} | {}", c.kind, c.subject, c.title, c.evidence))
        .collect();
    cards.sort();
    let mut completed: Vec<_> = b
        .features
        .iter()
        .map(|x| (x.key.clone(), x.completed, x.completed_users))
        .collect();
    completed.sort();
    format!(
        "readiness {}\nresurrected {}\nretention {:?}\njourneys {:?}\ncards {cards:?}\ncompleted {completed:?}",
        b.readiness.judged_users, b.users.resurrected, b.retention, b.journeys
    )
}

async fn boards(root: &std::path::Path) -> Vec<String> {
    let q = QueryEngine::new(root, Limits::default());
    let m = map();
    let mut out = Vec::new();
    for tz in ["UTC", "Europe/Paris"] {
        let f = Filters {
            tz: tz.into(),
            ..Filters::default()
        };
        let b = board(&q, "p", &f, Some((&m, 1))).await.unwrap();
        out.push(cohorts(&b));
    }
    out
}

#[tokio::test]
#[ignore = "heavy: rolls up every closed day; run by `cargo xtask ci --heavy` (nightly, release tags)"]
async fn cohort_blocks_read_closed_days_from_the_rollups_with_the_same_numbers() {
    let dir = seed();
    let root = dir.path();
    let events_only = boards(root).await;
    let utc = &events_only[0];
    assert!(utc.contains("resurrected 1"), "{utc}");
    assert!(utc.contains("journeys [Journey"), "{utc}");
    assert!(!utc.contains("readiness 0"), "{utc}");
    assert!(utc.contains("retention [RetentionRow"), "{utc}");
    // Payments in the last 30 days, all on closed days.
    assert!(utc.contains("completed [(\"billing\""), "{utc}");
    assert!(!utc.contains("(\"billing\", 0, 0)"), "{utc}");
    let rolled = roll_up(root).await;
    assert_eq!(boards(root).await, events_only);
    // A closed day without its events counts the same (nobody pays or
    // starts on it).
    let day = rolled[rolled.len() - 5];
    let dir_of_day = openlens_storage::project_dir(root, "p")
        .unwrap()
        .join(format!("date={}", day.format("%Y-%m-%d")));
    std::fs::remove_dir_all(dir_of_day).unwrap();
    assert_eq!(boards(root).await, events_only);
}

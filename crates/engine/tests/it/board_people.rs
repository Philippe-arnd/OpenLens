//! The board's people blocks read `person_day` for closed days and the
//! events for the rest (#492, D-040): the numbers are the same as from the
//! events alone, and a day rolled with another feature map is read from
//! its events.

use chrono::{Duration, Utc};
use openlens_engine::{Board, Filters, board, rollups::day_features};
use openlens_event::Event;
use openlens_featuremap::{FeatureMap, parse};
use openlens_query::{Limits, QueryEngine};
use openlens_storage::{FlushPolicy, Writer, rollup};

/// Version 2 moves billing from `/billing` to `/pay`.
fn map(version: i64) -> FeatureMap {
    let billing = if version == 1 { "/billing" } else { "/pay" };
    parse(&format!(
        "roles:\n  - key: staff\n    match: [admin]\n  - key: member\n    match: [member]\n\
         features:\n  - key: search\n    name: Search\n    entry: {{ routes: ['/search'] }}\n\
         \x20 - key: billing\n    name: Billing\n    entry: {{ routes: ['{billing}'] }}\n"
    ))
    .map
    .unwrap()
}

/// 40 days of people with roles, features and releases, a few of them
/// anonymous, plus some staging traffic; nobody is identified on a day
/// that gets rolled up, so dropping such a day's events keeps `persons`.
fn seed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let now = Utc::now();
    let people = [
        ("a1", "admin"),
        ("a2", "Admin"),
        ("m1", "member"),
        ("m2", "member"),
        ("m3", "member"),
        ("g1", "guest"),
        ("anon-1", "member"),
        ("anon-2", "guest"),
    ];
    let paths = ["/search", "/billing", "/pay", "/", "/search/x"];
    let mut evs = Vec::new();
    for k in 0..40i64 {
        for (i, (who, role)) in people.iter().enumerate() {
            if (k + i as i64) % 3 == 0 {
                continue;
            }
            for n in 0..(1 + (k as usize + i) % 3) {
                // Half past an hour: never within minutes of a window's edge.
                let ts = now
                    - Duration::days(k)
                    - Duration::minutes(60 * (1 + n as i64 + i as i64) + 30);
                let path = paths[(k as usize + i + n) % paths.len()];
                let (event, feature) = if n == 2 {
                    ("$click", Some("search"))
                } else {
                    ("$pageview", None)
                };
                evs.push(Event {
                    uuid: format!("{who}-{k}-{n}"),
                    project: "p".into(),
                    event: event.into(),
                    distinct_id: (*who).into(),
                    session_id: Some(format!("{who}-{k}")),
                    timestamp: ts,
                    received_at: ts,
                    url: None,
                    path: Some(path.into()),
                    feature: feature.map(Into::into),
                    role: Some((*role).into()),
                    account: None,
                    plan: None,
                    environment: if (k + n as i64) % 7 == 0 {
                        "staging"
                    } else {
                        "production"
                    }
                    .into(),
                    release: Some(format!("v{}", 1 + (k + i as i64) % 2)),
                    flags: None,
                    properties: "{}".into(),
                });
            }
        }
    }
    for who in ["a1", "a2", "m1", "m2", "m3", "g1"] {
        let ts = now - Duration::minutes(30);
        evs.push(Event {
            uuid: format!("identify-{who}"),
            project: "p".into(),
            event: "$identify".into(),
            distinct_id: who.into(),
            session_id: None,
            timestamp: ts,
            received_at: ts,
            url: None,
            path: None,
            feature: None,
            role: None,
            account: None,
            plan: None,
            environment: "production".into(),
            release: None,
            flags: None,
            properties: "{}".into(),
        });
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    dir
}

/// Roll up every closed day with the map `version`, as the server does.
async fn roll_up(root: &std::path::Path, version: i64) -> Vec<chrono::NaiveDate> {
    let q = QueryEngine::new(root, Limits::default());
    let m = map(version);
    let features = day_features(Some((&m, version)));
    let days = rollup::missing(root, "p", Utc::now()).unwrap();
    assert!(days.len() > 30, "most of the window is closed");
    for d in &days {
        let label = d.format("%Y-%m-%d").to_string();
        let batches = q.rollup_day("p", &label, None, &features).await.unwrap();
        assert!(rollup::write(root, "p", *d, rollup::generation(), &batches).unwrap());
    }
    days
}

/// The people blocks of a board, in a stable order.
fn people(b: &Board) -> String {
    let mut features: Vec<_> = b
        .features
        .iter()
        .map(|f| (f.key.clone(), f.uses, f.users, f.user_days))
        .collect();
    features.sort();
    let mut roles: Vec<_> = b.roles.iter().map(|r| (r.role.clone(), r.users)).collect();
    roles.sort();
    let mut gaps = b.role_gaps.clone();
    gaps.sort();
    let mut role_features = b.role_features.clone();
    role_features.sort();
    let u = &b.users;
    format!(
        "users {} {} {} known {} anonymous {}\nseries {:?}\nfeatures {features:?}\nroles {roles:?} {gaps:?}\nrole x feature {role_features:?}",
        u.dau, u.wau, u.mau, u.known, u.anonymous, b.dau_series
    )
}

/// The boards the test compares: every filter that changes how the
/// rollup is read.
async fn boards(root: &std::path::Path, version: i64) -> Vec<String> {
    let q = QueryEngine::new(root, Limits::default());
    let m = map(version);
    let mut out = Vec::new();
    for f in [
        Filters::default(),
        Filters {
            role: Some("staff".into()),
            ..Filters::default()
        },
        Filters {
            release: Some("v2".into()),
            ..Filters::default()
        },
        Filters {
            environment: "staging".into(),
            ..Filters::default()
        },
    ] {
        let b = board(&q, "p", &f, Some((&m, version))).await.unwrap();
        out.push(people(&b));
    }
    out
}

/// Drop the stored events of `day` (as retention or the cold tier would).
fn drop_events(root: &std::path::Path, day: chrono::NaiveDate) {
    let dir = openlens_storage::project_dir(root, "p")
        .unwrap()
        .join(format!("date={}", day.format("%Y-%m-%d")));
    std::fs::remove_dir_all(dir).unwrap();
}

/// The sum of feature uses over the default board.
async fn uses(root: &std::path::Path, version: i64) -> i64 {
    let q = QueryEngine::new(root, Limits::default());
    let m = map(version);
    let b = board(&q, "p", &Filters::default(), Some((&m, version)))
        .await
        .unwrap();
    b.features.iter().map(|f| f.uses).sum()
}

#[tokio::test]
async fn people_blocks_read_closed_days_from_person_day_with_the_same_numbers() {
    let dir = seed();
    let root = dir.path();
    let events_only = boards(root, 1).await;
    assert!(events_only[0].contains("staff"), "{}", events_only[0]);
    let rolled = roll_up(root, 1).await;
    assert_eq!(boards(root, 1).await, events_only);
    // Read from the rollup: a closed day without its events counts the same.
    drop_events(root, rolled[rolled.len() - 5]);
    assert_eq!(boards(root, 1).await, events_only);
}

#[tokio::test]
async fn a_day_rolled_with_an_older_map_is_read_from_its_events() {
    let dir = seed();
    let root = dir.path();
    let events_only = boards(root, 2).await;
    assert_ne!(events_only, boards(root, 1).await, "the maps differ");
    let rolled = roll_up(root, 1).await;
    assert_eq!(boards(root, 2).await, events_only);
    // The map in force reads that day's events, so losing them shows; the
    // map it was rolled with reads the rollup.
    let (v1, v2) = (uses(root, 1).await, uses(root, 2).await);
    drop_events(root, rolled[rolled.len() - 5]);
    assert_eq!(uses(root, 1).await, v1);
    assert!(uses(root, 2).await < v2);
}

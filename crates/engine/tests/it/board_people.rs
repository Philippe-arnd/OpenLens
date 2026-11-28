//! The board's people blocks read `person_day` for closed days and the
//! events for the rest (#492, D-040), and its sessions block and releases
//! table read `session_day` the same way (#498, D-047): the numbers are
//! the same as from the events alone, a session crossing midnight from a
//! rolled day into an open one is still one session, and a day rolled with
//! another feature map is read from its events.

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

/// 12 days of people with roles, features and releases, a few of them
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
    for k in 0..12i64 {
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
    // A session over the midnight after the last closed day: a page view
    // before it, then a page view and an error after it. Each half alone
    // would be a bounce; together the session is not.
    let midnight = (rollup::last_closed(now) + Duration::days(1))
        .and_time(chrono::NaiveTime::MIN)
        .and_utc();
    for (n, (event, minutes)) in [("$pageview", -10), ("$pageview", 10), ("$error", 20)]
        .into_iter()
        .enumerate()
    {
        let ts = midnight + Duration::minutes(minutes);
        evs.push(Event {
            uuid: format!("cross-{n}"),
            project: "p".into(),
            event: event.into(),
            distinct_id: "m1".into(),
            session_id: Some("cross".into()),
            timestamp: ts,
            received_at: ts,
            url: None,
            path: Some("/search".into()),
            feature: None,
            role: Some("member".into()),
            account: None,
            plan: None,
            environment: "production".into(),
            release: Some("v1".into()),
            flags: None,
            properties: "{}".into(),
        });
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
    assert!(days.len() >= 10, "most of the data is on closed days");
    for d in &days {
        let label = d.format("%Y-%m-%d").to_string();
        let batches = q.rollup_day("p", &label, None, &features).await.unwrap();
        assert!(rollup::write(root, "p", *d, rollup::generation(), &batches).unwrap());
    }
    days
}

/// The people blocks, the sessions block and the releases table of a
/// board, in a stable order.
fn people(b: &Board) -> String {
    format!("{}\n{}", people_only(b), sessions(b))
}

/// The sessions block and the releases table of a board (averages
/// rounded: the order of a parallel sum may change their last bits).
fn sessions(b: &Board) -> String {
    let s = &b.sessions;
    let sessions = format!(
        "sessions {} {:.3} {:.6} {:.6} {:.6}",
        s.count, s.avg_secs, s.bounce_rate, s.error_rate, s.tasks_per_user_day
    );
    let releases: Vec<String> = b
        .releases
        .iter()
        .map(|r| {
            format!(
                "{} {} {} {} {:.6} {:.3} {:.6} {:.6}",
                r.release,
                r.first_seen,
                r.users,
                r.sessions,
                r.error_rate,
                r.avg_secs,
                r.bounce_rate,
                r.tasks_per_session
            )
        })
        .collect();
    format!("{sessions}\nreleases {releases:?}")
}

/// The people blocks of a board, in a stable order.
fn people_only(b: &Board) -> String {
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
    boards_with(root, version, true).await
}

/// [`boards`], or only the unfiltered one when `all` is false (the
/// feature-map test needs no filters, and each board costs ~15 queries).
async fn boards_with(root: &std::path::Path, version: i64, all: bool) -> Vec<String> {
    let q = QueryEngine::new(root, Limits::default());
    let m = map(version);
    let mut out = Vec::new();
    let filters = [
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
        // Sessions count user-days on the project's days, which are not the
        // rollup's UTC days here (the crossing session starts on Paris's
        // next day). Only sessions and releases are compared: the daily
        // series and features of such a zone stay on the events (D-040),
        // so a day without its events shows there.
        Filters {
            tz: "Europe/Paris".into(),
            ..Filters::default()
        },
    ];
    let n = if all { filters.len() } else { 1 };
    for f in filters.into_iter().take(n) {
        let utc = f.tz == "UTC";
        let b = board(&q, "p", &f, Some((&m, version))).await.unwrap();
        out.push(if utc { people(&b) } else { sessions(&b) });
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
#[ignore = "heavy: rolls up every closed day; run by `cargo xtask ci --heavy` (nightly, release tags)"]
async fn board_blocks_read_closed_days_from_the_rollups_with_the_same_numbers() {
    let dir = seed();
    let root = dir.path();
    let events_only = boards(root, 1).await;
    assert!(events_only[0].contains("staff"), "{}", events_only[0]);
    assert!(
        events_only[0].contains("releases [\"v"),
        "{}",
        events_only[0]
    );
    let rolled = roll_up(root, 1).await;
    assert_eq!(boards(root, 1).await, events_only);
    // Read from the rollup: a closed day without its events counts the
    // same, including the last one, where the crossing session starts.
    drop_events(root, rolled[rolled.len() - 5]);
    if let Some(last) = rolled.last() {
        drop_events(root, *last);
    }
    assert_eq!(boards(root, 1).await, events_only);
}

#[tokio::test]
#[ignore = "heavy: rolls up every closed day; run by `cargo xtask ci --heavy` (nightly, release tags)"]
async fn a_day_rolled_with_an_older_map_is_read_from_its_events() {
    let dir = seed();
    let root = dir.path();
    let events_only = boards_with(root, 2, false).await;
    assert_ne!(
        events_only,
        boards_with(root, 1, false).await,
        "the maps differ"
    );
    let rolled = roll_up(root, 1).await;
    assert_eq!(boards_with(root, 2, false).await, events_only);
    // The map in force reads that day's events, so losing them shows; the
    // map it was rolled with reads the rollup.
    let (v1, v2) = (uses(root, 1).await, uses(root, 2).await);
    let lost = rolled[rolled.len() - 5];
    drop_events(root, lost);
    assert_eq!(uses(root, 1).await, v1);
    assert!(uses(root, 2).await < v2);
    // The board with the map in force names the day it cannot count
    // (#489); the map it was rolled with still reads it.
    let q = QueryEngine::new(root, Limits::default());
    let (m1, m2) = (map(1), map(2));
    let f = Filters::default();
    let b2 = board(&q, "p", &f, Some((&m2, 2))).await.unwrap();
    assert_eq!(
        b2.unavailable_days,
        vec![lost.format("%Y-%m-%d").to_string()]
    );
    let b1 = board(&q, "p", &f, Some((&m1, 1))).await.unwrap();
    assert!(b1.unavailable_days.is_empty());
}

#[tokio::test]
async fn a_day_with_neither_its_events_nor_a_matching_rollup_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let now = Utc::now();
    let (gone, kept) = (
        now.date_naive() - Duration::days(10),
        now.date_naive() - Duration::days(9),
    );
    let (mut w, _) = Writer::open(root, FlushPolicy::default()).unwrap();
    let evs: Vec<Event> = [kept, now.date_naive()]
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let ts = d.and_hms_opt(0, 30, 0).unwrap().and_utc().min(now);
            Event {
                uuid: format!("e{i}"),
                project: "p".into(),
                event: "$pageview".into(),
                distinct_id: "m1".into(),
                session_id: Some(format!("s{i}")),
                timestamp: ts,
                received_at: ts,
                url: None,
                path: Some("/search".into()),
                feature: None,
                role: Some("member".into()),
                account: None,
                plan: None,
                environment: "production".into(),
                release: Some("v1".into()),
                flags: None,
                properties: "{}".into(),
            }
        })
        .collect();
    w.append(&evs).unwrap();
    w.flush().unwrap();
    // Both days were rolled up with nothing the map in force can read
    // (empty tables, as with another map's rows); only `gone` lost its
    // events, as the cold tier or the retention would.
    for d in [gone, kept] {
        assert!(rollup::write(root, "p", d, rollup::generation(), &rollup::empty()).unwrap());
    }
    let q = QueryEngine::new(root, Limits::default());
    let m = map(1);
    let want = vec![gone.format("%Y-%m-%d").to_string()];
    let b = board(&q, "p", &Filters::default(), Some((&m, 1)))
        .await
        .unwrap();
    assert_eq!(b.unavailable_days, want);
    let h = openlens_engine::map_health_in(&q, "p", &m, Some(1), &Filters::default())
        .await
        .unwrap();
    assert_eq!(h.unavailable_days, want);
}

/// The release lifecycle of every environment, in a stable order.
async fn lifecycle(root: &std::path::Path, tz: &str) -> String {
    let q = QueryEngine::new(root, Limits::default());
    let r = openlens_engine::releases::releases(&q, "p", tz, None, &[], &[])
        .await
        .unwrap();
    let mut rows: Vec<String> = r
        .releases
        .iter()
        .map(|x| {
            format!(
                "{} {} {:?} {:.6} {} {} {} {}",
                x.environment,
                x.release,
                x.state,
                x.share,
                x.users,
                x.events,
                x.first_seen,
                x.last_seen
            )
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

#[tokio::test]
#[ignore = "heavy: rolls up every closed day; run by `cargo xtask ci --heavy` (nightly, release tags)"]
async fn the_release_lifecycle_reads_closed_days_from_person_day_with_the_same_numbers() {
    let dir = seed();
    let root = dir.path();
    let (utc, paris) = (
        lifecycle(root, "UTC").await,
        lifecycle(root, "Europe/Paris").await,
    );
    assert!(utc.contains("production v1"), "{utc}");
    let rolled = roll_up(root, 1).await;
    assert_eq!(lifecycle(root, "UTC").await, utc);
    assert_eq!(lifecycle(root, "Europe/Paris").await, paris);
    // A rolled day whose events are gone still counts: the totals read the
    // rollup (#729), and so does the UTC daily share.
    drop_events(root, rolled[rolled.len() - 5]);
    assert_eq!(lifecycle(root, "UTC").await, utc);
}

/// The filter bar's latest releases (Errors, Pages, Events), in a stable
/// form.
async fn filter_releases(root: &std::path::Path, f: &Filters) -> String {
    let q = QueryEngine::new(root, Limits::default());
    let b = openlens_engine::filter_options(&q, "p", f, None)
        .await
        .unwrap();
    let mut rows: Vec<String> = b
        .releases
        .iter()
        .map(|r| format!("{} {} {}", r.release, r.first_seen, r.users))
        .collect();
    rows.sort();
    rows.join("\n")
}

#[tokio::test]
#[ignore = "heavy: rolls up every closed day; run by `cargo xtask ci --heavy` (nightly, release tags)"]
async fn the_filter_bar_releases_read_closed_days_from_person_day_with_the_same_numbers() {
    let dir = seed();
    let root = dir.path();
    let staging = Filters {
        environment: "staging".into(),
        ..Filters::default()
    };
    let (all, stage) = (
        filter_releases(root, &Filters::default()).await,
        filter_releases(root, &staging).await,
    );
    assert!(all.contains('v'), "{all}");
    let rolled = roll_up(root, 1).await;
    // A rolled day without its events still counts: it is read from the
    // rollup (#748).
    drop_events(root, rolled[rolled.len() - 5]);
    assert_eq!(filter_releases(root, &Filters::default()).await, all);
    assert_eq!(filter_releases(root, &staging).await, stage);
}

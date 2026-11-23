//! Integration tests for `openlens-storage`.

// Test helpers may unwrap: a failure here is a test failure (clippy.toml
// already allows it inside #[test] functions; this extends it to helpers).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, TimeZone, Utc};
use openlens_event::Event;
use openlens_storage::{FlushPolicy, Writer, clean_urls, compact, count_rows, valid_project};

fn ev(project: &str, ts: chrono::DateTime<Utc>) -> Event {
    Event {
        uuid: uuid::Uuid::now_v7().to_string(),
        project: project.into(),
        event: "$pageview".into(),
        distinct_id: "u1".into(),
        session_id: Some("s1".into()),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some("/".into()),
        feature: None,
        role: Some("admin".into()),
        account: None,
        plan: None,
        environment: "production".into(),
        release: Some("v1".into()),
        flags: None,
        properties: "{}".into(),
    }
}

#[test]
fn flushed_events_land_in_hive_partitions_and_count_back() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, recovered) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    assert_eq!(recovered, 0);
    let t = Utc.with_ymd_and_hms(2026, 11, 21, 9, 30, 0).unwrap();
    w.append(&[ev("p1", t), ev("p1", t + Duration::hours(1)), ev("p2", t)])
        .unwrap();
    assert_eq!(w.flush().unwrap(), 3);
    assert_eq!(count_rows(dir.path(), "p1").unwrap(), 2);
    assert_eq!(count_rows(dir.path(), "p2").unwrap(), 1);
    assert!(
        dir.path()
            .join("events/project=p1/date=2026-11-21")
            .is_dir()
    );
}

#[test]
fn unflushed_events_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
        w.append(&[ev("p", Utc::now()), ev("p", Utc::now())])
            .unwrap();
        // Dropped without flush: simulates a crash.
    }
    let (_w, recovered) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    assert_eq!(recovered, 2);
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 2);
}

#[test]
fn torn_wal_line_is_skipped_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    {
        let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
        w.append(&[ev("p", Utc::now())]).unwrap();
    }
    let wal = std::fs::read_dir(dir.path().join("wal"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(wal).unwrap();
    f.write_all(b"{\"uuid\":\"trunc").unwrap();
    drop(f);
    let (_w, recovered) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    assert_eq!(recovered, 1);
}

#[test]
fn compaction_merges_closed_days_and_keeps_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let old = Utc.with_ymd_and_hms(2026, 11, 20, 8, 0, 0).unwrap();
    for _ in 0..3 {
        w.append(&[ev("p", old), ev("p", old)]).unwrap();
        w.flush().unwrap();
    }
    let day = dir.path().join("events/project=p/date=2026-11-20");
    let files = || std::fs::read_dir(&day).unwrap().count();
    assert!(files() > 1);
    assert_eq!(compact(dir.path(), "p", Utc::now()).unwrap(), 1);
    assert_eq!(files(), 1);
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 6);
}

#[test]
fn flush_policy_triggers_on_size() {
    let dir = tempfile::tempdir().unwrap();
    let policy = FlushPolicy {
        max_events: 2,
        max_age: std::time::Duration::from_secs(3600),
    };
    let (mut w, _) = Writer::open(dir.path(), policy).unwrap();
    w.append(&[ev("p", Utc::now())]).unwrap();
    assert!(!w.should_flush());
    w.append(&[ev("p", Utc::now())]).unwrap();
    assert!(w.should_flush());
}

#[test]
fn project_ids_that_escape_the_tree_are_refused() {
    for bad in ["", "../etc", "a/b", "a b", &"x".repeat(65)] {
        assert!(!valid_project(bad), "{bad:?} should be invalid");
    }
    assert!(valid_project("default"));
}

#[test]
fn retention_drops_days_older_than_the_window_only() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let keep = openlens_storage::EVENT_RETENTION_DAYS - 1;
    let drop = openlens_storage::EVENT_RETENTION_DAYS + 2;
    w.append(&[
        ev("p", now),
        ev("p", now - Duration::days(keep)),
        ev("p", now - Duration::days(drop)),
    ])
    .unwrap();
    w.flush().unwrap();
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 3);
    assert_eq!(openlens_storage::prune(dir.path(), "p", now).unwrap(), 1);
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 2);
    assert_eq!(openlens_storage::prune(dir.path(), "p", now).unwrap(), 0);
    assert_eq!(
        openlens_storage::prune(dir.path(), "empty", now).unwrap(),
        0
    );
}

/// The SDK resends a failed batch with the same uuids: it is stored once,
/// also when the resend comes after a restart (compaction removes it).
#[test]
fn a_resent_batch_is_stored_once() {
    let dir = tempfile::tempdir().unwrap();
    let old = Utc.with_ymd_and_hms(2026, 11, 20, 8, 0, 0).unwrap();
    let batch = [ev("p", old), ev("p", old)];
    {
        let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
        w.append(&batch).unwrap();
        w.append(&batch).unwrap();
        w.flush().unwrap();
        assert_eq!(count_rows(dir.path(), "p").unwrap(), 2, "resent at once");
    }
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(&batch).unwrap();
    w.flush().unwrap();
    assert_eq!(
        count_rows(dir.path(), "p").unwrap(),
        4,
        "after a restart, until compaction"
    );
    compact(dir.path(), "p", Utc::now()).unwrap();
    assert_eq!(
        count_rows(dir.path(), "p").unwrap(),
        2,
        "compaction keeps one of each"
    );
}

#[test]
fn stored_query_strings_are_cleaned_once_and_rows_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let t = Utc.with_ymd_and_hms(2026, 11, 21, 9, 30, 0).unwrap();
    let mut dirty = ev("p", t);
    dirty.url = Some("https://app.test/reset?token=SECRET123".into());
    let mut clean = ev("p", t);
    clean.url = Some("https://app.test/".into());
    w.append(&[dirty, clean]).unwrap();
    w.flush().unwrap();
    assert!(clean_urls(dir.path(), "p").unwrap() > 0);
    assert_eq!(clean_urls(dir.path(), "p").unwrap(), 0);
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 2);
}

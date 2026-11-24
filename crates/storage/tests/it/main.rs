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

#[test]
fn appends_go_on_while_a_taken_flush_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let t = Utc.with_ymd_and_hms(2026, 11, 21, 9, 30, 0).unwrap();
    w.append(&[ev("p", t), ev("p", t)]).unwrap();
    let pending = w.take().unwrap().unwrap();
    // The writer is free: a new append lands in the fresh WAL.
    w.append(&[ev("p", t)]).unwrap();
    assert_eq!(pending.write().unwrap(), 2);
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 2);
    drop(w);
    // Restart: only the event appended after the take is replayed.
    let (_w, recovered) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    assert_eq!(recovered, 1);
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 3);
}

#[test]
fn a_failed_flush_write_is_retried_by_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let t = Utc.with_ymd_and_hms(2026, 11, 21, 9, 30, 0).unwrap();
    w.append(&[ev("p", t)]).unwrap();
    let pending = w.take().unwrap().unwrap();
    w.restore(pending);
    w.append(&[ev("p", t)]).unwrap();
    assert_eq!(w.flush().unwrap(), 2);
    drop(w);
    let (_w, recovered) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    assert_eq!(recovered, 0, "both WAL files were removed");
    assert_eq!(count_rows(dir.path(), "p").unwrap(), 2);
}

#[test]
fn concurrent_appends_share_fsyncs_and_all_survive_a_restart() {
    use std::sync::{Arc, Mutex};
    let dir = tempfile::tempdir().unwrap();
    let (w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let w = Arc::new(Mutex::new(w));
    let t = Utc.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap();
    let (threads, each) = (8u64, 50u64);
    std::thread::scope(|s| {
        for _ in 0..threads {
            let w = w.clone();
            s.spawn(move || {
                for _ in 0..each {
                    // As capture does (#333): write under the lock, wait for
                    // the disk outside it.
                    let (ticket, durable) = {
                        let mut g = w.lock().unwrap();
                        (g.write(&[ev("p1", t)]).unwrap(), g.durable())
                    };
                    durable.wait(ticket).unwrap();
                }
            });
        }
    });
    let syncs = w.lock().unwrap().durable().syncs();
    assert!(
        syncs < threads * each,
        "{syncs} fsyncs for {} writes",
        threads * each
    );
    drop(w);
    // Nothing flushed to Parquet: everything comes back from the WAL.
    let (_, recovered) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    assert_eq!(recovered as u64, threads * each);
}

#[test]
fn a_resent_batch_is_dropped_but_its_ticket_covers_the_first_copy() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let t = Utc.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap();
    let batch = [ev("p1", t), ev("p1", t)];
    let first = w.write(&batch).unwrap();
    let again = w.write(&batch).unwrap();
    assert!(again >= first, "the retry waits for the first copy's fsync");
    w.durable().wait(again).unwrap();
    assert_eq!(w.buffered(), 2, "the resent events were dropped");
}

/// Every stored `properties` value of project `p`.
fn all_properties(root: &std::path::Path) -> Vec<String> {
    use arrow::array::{Array, StringArray};
    let mut out = Vec::new();
    let pdir = openlens_storage::project_dir(root, "p").unwrap();
    for day in std::fs::read_dir(pdir).unwrap() {
        for f in std::fs::read_dir(day.unwrap().path()).unwrap() {
            let f = f.unwrap().path();
            if f.extension().is_none_or(|x| x != "parquet") {
                continue;
            }
            let r = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                std::fs::File::open(&f).unwrap(),
            )
            .unwrap()
            .build()
            .unwrap();
            for b in r {
                let b = b.unwrap();
                let c = b
                    .column_by_name("properties")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .clone();
                out.extend((0..c.len()).map(|i| c.value(i).to_owned()));
            }
        }
    }
    out
}

/// Clicks older than 30 days lose their label and position once the day
/// is rolled up (#324); `$rage` and other events stay, and a day is read
/// again only when its files change.
#[test]
fn old_clicks_are_trimmed_once_rolled_up() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let now = Utc::now();
    let old = now - Duration::days(40);
    let (mut w, _) = Writer::open(root, FlushPolicy::default()).unwrap();
    let mut click = ev("p", old);
    click.event = "$click".into();
    click.properties = r#"{"$text":"Pay Ada","$x":0.5,"$y":120,"$vw":1280,"$rage":true}"#.into();
    let mut view = ev("p", old);
    view.properties = r#"{"$text":"kept"}"#.into();
    w.append(&[click, view]).unwrap();
    w.flush().unwrap();
    let day = old.date_naive();
    // No rollup yet: nothing is trimmed.
    assert_eq!(openlens_storage::trim_clicks(root, "p", now).unwrap(), 0);
    let tables: Vec<_> = openlens_storage::rollup::tables()
        .into_iter()
        .map(|(n, s)| (n, arrow::array::RecordBatch::new_empty(s)))
        .collect();
    let g = openlens_storage::rollup::generation();
    assert!(openlens_storage::rollup::write(root, "p", day, g, &tables).unwrap());
    assert_eq!(openlens_storage::trim_clicks(root, "p", now).unwrap(), 1);
    assert_eq!(openlens_storage::trim_clicks(root, "p", now).unwrap(), 0);
    let props = all_properties(root);
    assert!(props.contains(&r#"{"$rage":true}"#.to_owned()), "{props:?}");
    assert!(
        props.contains(&r#"{"$text":"kept"}"#.to_owned()),
        "{props:?}"
    );
    assert_eq!(count_rows(root, "p").unwrap(), 2);
}

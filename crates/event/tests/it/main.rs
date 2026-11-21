//! Integration tests for `openlens-event`.

use chrono::{Duration, TimeZone, Utc};
use openlens_event::{CaptureBatch, EventError, RawEvent, limits, normalize_batch};
use serde_json::json;

fn raw(event: &str, props: serde_json::Value) -> RawEvent {
    RawEvent {
        event: event.into(),
        distinct_id: Some("u1".into()),
        properties: props.as_object().cloned(),
        ..Default::default()
    }
}

#[test]
fn promoted_dimensions_leave_the_properties_blob() {
    let now = Utc::now();
    let batch = CaptureBatch {
        batch: vec![raw(
            "$pageview",
            json!({"$role": "owner", "$feature": "export", "$pathname": "/a", "kept": 1}),
        )],
        ..Default::default()
    };
    let out = normalize_batch("p", batch, now).unwrap();
    let ev = &out.events[0];
    assert_eq!(ev.role.as_deref(), Some("owner"));
    assert_eq!(ev.feature.as_deref(), Some("export"));
    assert!(!ev.properties.contains("$role"));
    assert!(ev.properties.contains("kept"));
}

#[test]
fn one_bad_event_does_not_drop_the_batch() {
    let mut bad = raw("x", json!({}));
    bad.distinct_id = None;
    let batch = CaptureBatch {
        batch: vec![raw("ok", json!({})), bad, raw("", json!({}))],
        ..Default::default()
    };
    let out = normalize_batch("p", batch, Utc::now()).unwrap();
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.rejected.len(), 2);
    assert!(
        out.rejected
            .iter()
            .any(|(_, e)| *e == EventError::MissingDistinctId)
    );
}

#[test]
fn client_clock_skew_is_corrected_by_sent_at() {
    let received = Utc.with_ymd_and_hms(2026, 11, 21, 12, 0, 0).unwrap();
    // The client clock runs 2 hours behind: it sent "10:00" at real 12:00.
    let mut ev = raw("x", json!({}));
    ev.timestamp = Some("2026-11-21T09:59:00Z".into());
    let batch = CaptureBatch {
        sent_at: Some("2026-11-21T10:00:00Z".into()),
        batch: vec![ev],
        ..Default::default()
    };
    let out = normalize_batch("p", batch, received).unwrap();
    let ts = out.events[0].timestamp;
    assert!(ts < received && ts > received - Duration::minutes(5));
}

#[test]
fn far_future_timestamps_fall_back_to_receive_time() {
    let received = Utc::now();
    let mut ev = raw("x", json!({}));
    ev.timestamp = Some("2099-01-01T00:00:00Z".into());
    let out = normalize_batch(
        "p",
        CaptureBatch {
            batch: vec![ev],
            ..Default::default()
        },
        received,
    )
    .unwrap();
    assert_eq!(out.events[0].timestamp, received);
}

#[test]
fn oversized_and_deep_input_is_rejected_not_accepted() {
    let mut nested = json!(1);
    for _ in 0..(limits::MAX_DEPTH + 5) {
        nested = json!([nested]);
    }
    let batch = CaptureBatch {
        batch: vec![
            raw("x", json!({ "deep": nested })),
            raw(&"e".repeat(limits::MAX_EVENT_NAME + 1), json!({})),
            raw(
                "x",
                json!({ "big": "b".repeat(limits::MAX_PROPERTIES_BYTES) }),
            ),
        ],
        ..Default::default()
    };
    let out = normalize_batch("p", batch, Utc::now()).unwrap();
    assert!(out.events.is_empty());
    assert_eq!(out.rejected.len(), 3);

    let too_many = CaptureBatch {
        batch: vec![raw("x", json!({})); limits::MAX_BATCH_EVENTS + 1],
        ..Default::default()
    };
    assert!(normalize_batch("p", too_many, Utc::now()).is_err());
}

#[test]
fn hostile_bodies_never_panic() {
    for body in openlens_testkit::hostile_capture_bodies() {
        if let Ok(batch) = serde_json::from_slice::<CaptureBatch>(&body) {
            let _ = normalize_batch("p", batch, Utc::now());
        }
    }
}

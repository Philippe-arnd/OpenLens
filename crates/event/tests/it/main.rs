//! Integration tests for `openlens-event`.

// Test helpers may unwrap: a failure here is a test failure (clippy.toml
// already allows it inside #[test] functions; this extends it to helpers).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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

#[test]
fn environment_defaults_to_production_and_aliases_fold() {
    let batch = CaptureBatch {
        batch: vec![
            raw("x", json!({})),
            raw("x", json!({"$environment": "PROD"})),
            raw("x", json!({"$environment": "local", "$release": "v1.2.0"})),
            raw("x", json!({"$environment": "qa-eu"})),
        ],
        ..Default::default()
    };
    let out = normalize_batch("p", batch, Utc::now()).unwrap();
    let envs: Vec<_> = out.events.iter().map(|e| e.environment.as_str()).collect();
    assert_eq!(envs, ["production", "production", "development", "qa-eu"]);
    assert_eq!(out.events[2].release.as_deref(), Some("v1.2.0"));
    assert!(!out.events[2].properties.contains("$release"));
}

#[test]
fn stored_urls_drop_query_and_fragment_but_keep_campaign_tags() {
    let now = Utc::now();
    let url = "https://app.test/reset?token=SECRET123&utm_source=news&email=jean%40exemple.fr&gclid=g1#frag";
    let batch = CaptureBatch {
        batch: vec![raw("$pageview", json!({ "$current_url": url }))],
        ..Default::default()
    };
    let out = normalize_batch("p", batch, now).unwrap();
    let stored = out.events[0].url.clone().unwrap();
    assert!(stored.starts_with("https://app.test/reset"));
    for secret in ["SECRET123", "jean", "frag"] {
        assert!(!stored.contains(secret), "{stored}");
    }
    assert!(stored.contains("utm_source=news") && stored.contains("gclid=g1"));
    assert!(!out.events[0].properties.contains("SECRET123"));
}

/// The server reads the environment the browser SDK would detect (D-034),
/// so the two must agree, pattern by pattern, with `sdk/js/src/env.ts`.
#[test]
fn hostname_patterns_agree_with_the_browser_sdk() {
    use openlens_event::{detected_environment, sdk_guess};
    for (host, env) in [
        ("localhost", Some("development")),
        ("127.0.0.1", Some("development")),
        ("app.local", Some("development")),
        ("shop.test", Some("development")),
        ("staging.app.com", Some("staging")),
        ("app-stg.example.org", Some("staging")),
        ("pr-42.app.com", Some("preview")),
        ("app-pr-7.example.org", Some("preview")),
        ("x.vercel.app", Some("preview")),
        ("review.app.com", Some("preview")),
        // Look-alikes the SDK does not match.
        ("prestaging.app.com", None),
        ("pr-x.app.com", None),
        ("pr-12abc.app.com", None),
        ("kanban-test.philapps.com", None),
        ("app.com", None),
    ] {
        assert_eq!(detected_environment(host), env, "{host}");
    }
    assert_eq!(sdk_guess("kanban-test.philapps.com"), "production");
}

#[test]
fn the_host_of_a_page_has_no_port_path_or_case() {
    use openlens_event::host_of;
    assert_eq!(
        host_of("https://App.Example.com:8443/a/b?x=1").as_deref(),
        Some("app.example.com")
    );
    assert_eq!(host_of("http://[::1]:3000/").as_deref(), Some("[::1]"));
    assert_eq!(host_of("/relative/path"), None);
    assert_eq!(host_of("https:///nohost"), None);
}

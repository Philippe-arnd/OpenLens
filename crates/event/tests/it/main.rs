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
        if let Ok(batch) = serde_json::from_slice::<CaptureBatch>(&body)
            && let Ok(out) = normalize_batch("p", batch, Utc::now())
        {
            // What the writer then reads from each stored event (#449, #482).
            for e in &out.events {
                let _ = openlens_event::facts(&e.event, &e.properties);
            }
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

/// The typed facts of a `$vital` (#449) keep a value only where the
/// pattern the engine used on the text would have found it, so old files
/// (read through the pattern) and new ones agree; hostile text is bounded.
#[test]
fn vital_facts_keep_what_the_patterns_found_and_nothing_else() {
    use openlens_event::vital;
    let props = |extra: &str| format!(r#"{{"$name":"INP","$value":250{extra}}}"#);
    let v = vital("$vital", &props(r#","$input":"pointer","$processing":80"#)).unwrap();
    assert_eq!(v.value, Some(250.0));
    assert_eq!(
        v.kind.as_deref(),
        Some("pointer"),
        "$input when no $el_type"
    );
    assert_eq!(v.parts.iter().flatten().count(), 1);

    let at = |n: usize| "a".repeat(n);
    let el = |s: &str| {
        vital("$vital", &props(&format!(r#","$el":{}"#, json!(s))))
            .unwrap()
            .el
    };
    assert_eq!(el(&at(120)).map(|e| e.len()), Some(120));
    for bad in [
        at(121),
        "a\"b".into(),
        "a\\b".into(),
        "a\nb".into(),
        String::new(),
    ] {
        assert_eq!(el(&bad), None, "{bad:?}");
    }
    // Non-ASCII counts in characters, as the pattern does.
    assert!(el(&"é".repeat(120)).is_some());

    let one = |extra: &str| vital("$vital", &props(extra)).unwrap();
    assert_eq!(one(r#","$device":"watch""#).device, None);
    assert_eq!(one(r#","$el_type":"Image""#).kind, None);
    let text_value = vital("$vital", r#"{"$name":"INP","$value":"250"}"#).unwrap();
    assert_eq!(text_value.value, None);

    // No valid name, not a vital, not an object, too large: no facts.
    let huge = props(&format!(r#","$x":"{}""#, at(limits::MAX_PROPERTIES_BYTES)));
    for (event, text) in [
        ("$vital", r#"{"$name":"lcp","$value":1}"#.to_owned()),
        ("$pageview", props("")),
        ("$vital", "[1,2]".into()),
        ("$vital", "{".into()),
        ("$vital", huge),
    ] {
        let head: String = text.chars().take(40).collect();
        assert!(vital(event, &text).is_none(), "{event} {head}");
    }
}

/// The error, acquisition and friction facts (#482) are what the engine's
/// old expressions returned from the text, escapes included, and nothing
/// where they found nothing; hostile text is bounded.
#[test]
fn error_acquisition_and_friction_facts_are_what_the_patterns_returned() {
    use openlens_event::{Facts, facts};
    let stored = |v: serde_json::Value| v.to_string();

    // `((?:[^"\\]|\\.)*)` returns the string as spelled; `([^"]*)` stops
    // at the first quote, the one of an escaped `\"`.
    let e = facts(
        "$error",
        &stored(json!({
            "$message": "bad \"x\"\nline",
            "$source": "https://a.test/a\"b.js",
            "$stack": "at f (a.js:1:2)\n",
        })),
    );
    assert_eq!(e.message.as_deref(), Some(r#"bad \"x\"\nline"#));
    assert_eq!(e.source.as_deref(), Some(r"https://a.test/a\"));
    assert_eq!(e.stack.as_deref(), Some(r"at f (a.js:1:2)\n"));
    let e = facts("$error", &stored(json!({"$message": 5, "$source": ""})));
    assert_eq!((e.message, e.source.as_deref()), (None, Some("")));

    // `([^"]{1,200})`: up to the first quote, 1 to 200 characters.
    let a = |props: serde_json::Value| facts("$pageview", &stored(props)).acquisition;
    assert_eq!(
        a(json!({"$channel": "search", "$utm_campaign": "a\"b", "$ref_host": "é".repeat(200)}))
            .map(|v| v.map(|s| s.chars().count())),
        [Some(6), None, Some(2), Some(200)]
    );
    for bad in [String::new(), "x".repeat(201)] {
        assert_eq!(a(json!({ "$utm_source": bad }))[1], None, "{bad:?}");
    }
    // A leading quote leaves its backslash, as the pattern did.
    assert_eq!(a(json!({"$utm_source": "\"x"}))[1].as_deref(), Some("\\"));
    assert_eq!(a(json!({"$channel": ["search"]}))[0], None);

    // `LIKE '%"$rage":true%'`, on clicks only.
    let rage = |event: &str, props: &str| facts(event, props).rage;
    assert_eq!(rage("$click", r#"{"$rage":true}"#), Some(true));
    for no in [r#"{"$rage":"true"}"#, r#"{"$rage":false}"#, "{}"] {
        assert_eq!(rage("$click", no), Some(false), "{no}");
    }
    assert_eq!(rage("$pageview", r#"{"$rage":true}"#), None);

    // `$rating` even without a valid `$name`; only on `$vital`.
    let v = facts("$vital", r#"{"$name":"lcp","$rating":"poor"}"#);
    assert_eq!((v.vital, v.rating.as_deref()), (None, Some("poor")));
    assert_eq!(facts("$vital", r#"{"$rating":"POOR"}"#).rating, None);

    // Another event, not an object, too large: nothing.
    let huge = stored(json!({"$message": "a".repeat(limits::MAX_PROPERTIES_BYTES)}));
    for (event, text) in [
        ("$custom", stored(json!({"$message": "m", "$channel": "x"}))),
        ("$error", "[1]".to_owned()),
        ("$error", "{".to_owned()),
        ("$error", huge),
    ] {
        let head: String = text.chars().take(40).collect();
        assert_eq!(facts(event, &text), Facts::default(), "{event} {head}");
    }
}

/// #707 (D-111): a signup date is kept only when it can be one: not in
/// the future, not a placeholder before 1990, not unreadable.
#[test]
fn a_signup_date_is_ignored_when_it_cannot_be_one() {
    let now = Utc::now();
    let year_ago = (now - Duration::days(365)).to_rfc3339();
    let kept = openlens_event::signup_of(&json!({ "$created_at": year_ago }).to_string(), now);
    assert!(kept.is_some_and(|t| t < now));
    for bad in [
        (now + Duration::days(2)).to_rfc3339(),
        "1970-01-01T00:00:00Z".to_owned(),
        "yesterday".to_owned(),
        "9".repeat(100),
    ] {
        let props = json!({ "$created_at": bad }).to_string();
        assert!(openlens_event::signup_of(&props, now).is_none(), "{bad}");
    }
    assert!(openlens_event::signup_of(r#"{"$created_at":12}"#, now).is_none());
}

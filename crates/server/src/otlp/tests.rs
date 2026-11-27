use super::*;

fn sample(start: u64, time: u64, count: u64, buckets: Vec<u64>) -> Sample {
    Sample {
        series: Some("k".into()),
        start,
        time,
        count,
        sum: count as f64 * 0.01,
        min: 0.0,
        max: 1.0,
        buckets,
        bounds: vec![0.1],
        layout: 7,
        method: "GET".into(),
        target: Target::Route("/a".into()),
        status: 200,
        release: None,
        environment: None,
        exemplars: Vec::new(),
    }
}

#[test]
fn a_series_first_seen_is_counted_only_when_it_started_after_the_horizon() {
    let s = sample(100, 200, 5, vec![5, 0]);
    assert!(matches!(step(None, &s, 50), Step::Whole(100)));
    assert!(matches!(step(None, &s, 150), Step::Prime));
}

#[test]
fn a_retry_adds_nothing_and_a_counter_that_went_down_starts_over() {
    let prev = Last::of(&sample(100, 200, 10, vec![6, 4]));
    assert!(matches!(
        step(Some(&prev), &sample(100, 200, 10, vec![6, 4]), 0),
        Step::Stale
    ));
    assert!(matches!(
        step(Some(&prev), &sample(100, 300, 3, vec![3, 0]), 0),
        Step::Whole(200)
    ));
    let next = sample(100, 300, 15, vec![9, 6]);
    let Step::Since(p) = step(Some(&prev), &next, 0) else {
        panic!("a later point adds its difference");
    };
    let d = since(&next, &p).unwrap();
    assert_eq!(d.count, next.count - prev.count);
    assert_eq!(d.buckets.iter().sum::<u64>(), d.count);
}

fn kv(k: &str, v: &str) -> wire::KeyValue {
    wire::KeyValue {
        key: k.into(),
        value: Some(wire::AnyValue {
            string_value: Some(v.into()),
            ..wire::AnyValue::default()
        }),
    }
}

fn ex(time: u64, ms: f64, trace: Vec<u8>, attrs: Vec<wire::KeyValue>) -> wire::Exemplar {
    wire::Exemplar {
        filtered_attributes: attrs,
        time_unix_nano: time,
        as_double: Some(ms),
        as_int: None,
        trace_id: trace,
    }
}

#[test]
fn a_point_keeps_its_slowest_exemplar_and_first_5xx_and_only_the_session() {
    let p = wire::HistogramDataPoint {
        time_unix_nano: 100,
        exemplars: vec![
            ex(
                20,
                30.0,
                vec![0xab; 16],
                vec![
                    kv("session.id", "s1"),
                    kv("http.url", "/x?token=secret"),
                    kv("enduser.id", "u1"),
                ],
            ),
            ex(
                10,
                5.0,
                vec![0; 16],
                vec![kv("session.id", "bad id;"), kv("cookie", "sid=1")],
            ),
            ex(30, 900.0, vec![0xcd; 3], vec![]),
            ex(40, f64::NAN, vec![], vec![]),
        ],
        ..wire::HistogramDataPoint::default()
    };
    // ms: the unit's factor.
    let ok = exemplars(&p, 1e-3, 200);
    assert_eq!(ok.len(), 1, "a point below 500 keeps its slowest only");
    assert!((ok[0].secs - 0.9).abs() < 1e-9 && ok[0].trace_id.is_none());
    let bad = exemplars(&p, 1e-3, 503);
    assert_eq!(bad.len(), 2);
    let first = &bad[1];
    assert_eq!(first.time, 10);
    assert!(
        first.trace_id.is_none() && first.session.is_none(),
        "zero trace id and a bad session are left out"
    );
    let all = exemplars_json(&exemplars(
        &wire::HistogramDataPoint {
            exemplars: vec![ex(
                20,
                30.0,
                vec![0xab; 16],
                vec![
                    kv("session.id", "s1"),
                    kv("http.url", "/x?token=secret"),
                    kv("enduser.id", "u1"),
                    kv("user.id", "u2"),
                ],
            )],
            ..wire::HistogramDataPoint::default()
        },
        1e-3,
        200,
    ));
    let text = serde_json::to_string(&all).unwrap();
    assert!(text.contains("s1") && text.contains(&"ab".repeat(16)));
    assert!(
        !text.contains("secret") && !text.contains("http.url"),
        "other attributes are dropped"
    );
    assert!(
        !text.contains("u1") && !text.contains("u2") && !text.contains("user"),
        "an exemplar keeps the session, never the user"
    );
}

#[test]
fn a_server_address_keeps_the_host_and_nothing_else() {
    assert_eq!(
        host_of(Some(" API.Stripe.com ".into())).as_deref(),
        Some("api.stripe.com")
    );
    assert_eq!(host_of(Some("[::1]".into())).as_deref(), Some("::1"));
    for bad in [
        "https://api.stripe.com/v1/charges",
        "admin:hunter2\u{40}api.example",
        "api.stripe.com/v1?key=sk",
        "",
    ] {
        assert!(host_of(Some(bad.into())).is_none(), "{bad}");
    }
    assert!(host_of(Some("a".repeat(MAX_HOST + 1))).is_none());
}

#[test]
fn rebucketing_never_moves_a_count_below_its_upper_bound() {
    // Source buckets: (.., 30 ms], (30, 300 ms], (300 ms, ..).
    let out = rebucket(&[1, 2, 3], &[0.03, 0.3], 6);
    assert_eq!(out.iter().sum::<u64>(), 6);
    for (i, n) in out.iter().enumerate() {
        if *n > 0 && i < BOUNDS_MS.len() {
            assert!(BOUNDS_MS[i] >= 30.0);
        }
    }
    assert_eq!(out[BUCKETS - 1], 3, "the open bucket stays open");
    // OpenTelemetry's default bounds map one to one.
    let same: Vec<f64> = BOUNDS_MS.iter().map(|b| b / 1000.0).collect();
    let counts: Vec<u64> = (1..=BUCKETS as u64).collect();
    assert_eq!(rebucket(&counts, &same, counts.iter().sum()), counts);
}

//! OTLP metrics ingestion (#605, D-085): `http.server.request.duration`
//! from an OpenTelemetry exporter, protobuf or JSON, lands in the same
//! route stats as the Node SDK's.

use std::collections::HashMap;

use openlens_testkit::otlp::{Data, Metric, Point, request};

use super::*;

const SEC: u64 = 1_000_000_000;
/// OpenTelemetry's default bounds of the metric, seconds.
const BOUNDS_S: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

fn now_ns() -> u64 {
    // A millisecond ahead, so a series that starts now starts after the
    // harness (and its server) did.
    u64::try_from(chrono::Utc::now().timestamp_nanos_opt().unwrap()).unwrap() + 1_000_000
}

/// `n` requests in default bucket `i`.
fn one_bucket(i: usize, n: u64) -> Vec<u64> {
    let mut b = vec![0; BOUNDS_S.len() + 1];
    b[i] = n;
    b
}

fn duration(temporality: i32, points: Vec<Point>) -> Metric {
    Metric {
        name: "http.server.request.duration".into(),
        unit: "s".into(),
        data: Data::Histogram(temporality, points),
    }
}

async fn send(
    h: &Harness,
    content_type: &str,
    body: Vec<u8>,
    auth: Option<(&str, &str)>,
) -> (StatusCode, String, Vec<u8>) {
    let mut req = Request::post("/v1/metrics").header("content-type", content_type);
    if let Some((name, value)) = auth {
        req = req.header(name, value);
    }
    let resp = router(h.state.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, ct, bytes.to_vec())
}

async fn send_pb(h: &Harness, body: Vec<u8>) -> StatusCode {
    let key = h.key.clone();
    send(
        h,
        "application/x-protobuf",
        body,
        Some(("x-openlens-key", &key)),
    )
    .await
    .0
}

/// The routes of `env` over the last day, by `METHOD /route`.
async fn routes(h: &Harness, env: &str) -> HashMap<String, serde_json::Value> {
    openlens_server::flush_now(&h.state).unwrap();
    let r = get(
        h,
        &format!("/api/projects/default/routes?days=1&env={env}"),
        None,
        true,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    v["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                format!(
                    "{} {}",
                    r["method"].as_str().unwrap(),
                    r["route"].as_str().unwrap()
                ),
                r.clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn protobuf_and_json_exports_both_land_in_the_routes() {
    let h = harness();
    let t0 = now_ns();
    // What a Collector sends: protobuf, gzipped, the key in a header.
    let pb = request(
        &[
            ("service.name", "api"),
            ("service.version", "v7"),
            ("deployment.environment.name", "production"),
        ],
        &[duration(
            1,
            vec![Point {
                start: t0,
                time: t0 + 60 * SEC,
                count: 40,
                sum: Some(2.0),
                max: Some(0.2),
                buckets: one_bucket(6, 40),
                bounds: BOUNDS_S.to_vec(),
                exemplar: true,
                ..Point::route("GET", "/api/items/:id", 200)
            }],
        )],
    );
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&pb).unwrap();
    let key = h.key.clone();
    let req = Request::post("/v1/metrics")
        .header("content-type", "application/x-protobuf")
        .header("content-encoding", "gzip")
        .header("x-openlens-key", &key);
    let resp = router(h.state.clone())
        .oneshot(req.body(Body::from(gz.finish().unwrap())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "application/x-protobuf");

    // What the Node `exporter-metrics-otlp-http` sends: JSON, a bearer key.
    let json = serde_json::json!({ "resourceMetrics": [{
        "resource": { "attributes": [
            { "key": "service.version", "value": { "stringValue": "v7" } },
            { "key": "deployment.environment.name", "value": { "stringValue": "staging" } },
        ]},
        "scopeMetrics": [{ "scope": { "name": "@opentelemetry/instrumentation-http" }, "metrics": [{
            "name": "http.server.request.duration", "unit": "s",
            "histogram": { "aggregationTemporality": 1, "dataPoints": [{
                "attributes": [
                    { "key": "http.request.method", "value": { "stringValue": "POST" } },
                    { "key": "http.route", "value": { "stringValue": "/api/items" } },
                    { "key": "http.response.status_code", "value": { "intValue": "503" } },
                ],
                "startTimeUnixNano": t0.to_string(), "timeUnixNano": (t0 + 60 * SEC).to_string(),
                "count": "12", "sum": 1.2, "min": 0.01, "max": 0.4,
                "bucketCounts": one_bucket(7, 12).iter().map(u64::to_string).collect::<Vec<_>>(),
                "explicitBounds": BOUNDS_S,
                "exemplars": [{ "timeUnixNano": t0.to_string(), "asDouble": 0.3,
                    "traceId": "5b8efff798038103d269b633813fc60c", "spanId": "eee19b7ec3c1b174" }],
            }]},
        }]}],
    }]});
    let auth = format!("Bearer {}", h.key);
    let (status, ct, body) = send(
        &h,
        "application/json",
        json.to_string().into_bytes(),
        Some(("authorization", &auth)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(ct, "application/json", "answered in the request's encoding");

    let prod = routes(&h, "production").await;
    assert_eq!(prod["GET /api/items/:id"]["requests"], 40);
    assert!(
        !prod.contains_key("POST /api/items"),
        "the resource's environment is kept"
    );
    let staging = routes(&h, "staging").await;
    let post = &staging["POST /api/items"];
    assert_eq!(post["requests"], 12);
    assert_eq!(post["errors"], post["requests"], "the status code is read");

    // Server events with the resource's release, no person, no session.
    let s = h.state.clone();
    let r = s
        .query
        .sql(
            "default",
            "SELECT count(*) AS n, count(DISTINCT distinct_id) AS ids, count(session_id) AS sessions \
             FROM raw_events WHERE event = '$request_stats' AND release = 'v7'",
        )
        .await
        .unwrap();
    assert_eq!(r.rows[0]["n"], 2);
    assert_eq!(r.rows[0]["ids"], 1);
    assert_eq!(r.rows[0]["sessions"], 0);
}

/// The route's totals after `steps` (requests, default bucket) are sent
/// as delta or as cumulative points, one export each.
async fn totals(cumulative: bool, steps: &[(u64, usize)]) -> serde_json::Value {
    let h = harness();
    let t0 = now_ns();
    let mut running = vec![0u64; BOUNDS_S.len() + 1];
    for (i, (n, bucket)) in steps.iter().enumerate() {
        let delta = one_bucket(*bucket, *n);
        for (r, d) in running.iter_mut().zip(&delta) {
            *r += d;
        }
        let end = t0 + (i as u64 + 1) * 60 * SEC;
        let p = if cumulative {
            Point {
                start: t0,
                time: end,
                count: running.iter().sum(),
                sum: Some(running.iter().sum::<u64>() as f64 * 0.1),
                buckets: running.clone(),
                ..Point::route("GET", "/a", 200)
            }
        } else {
            Point {
                start: end - 60 * SEC,
                time: end,
                count: *n,
                sum: Some(*n as f64 * 0.1),
                buckets: delta,
                ..Point::route("GET", "/a", 200)
            }
        };
        let p = Point {
            max: Some(9.0),
            bounds: BOUNDS_S.to_vec(),
            ..p
        };
        let temporality = if cumulative { 2 } else { 1 };
        let status = send_pb(&h, request(&[], &[duration(temporality, vec![p])])).await;
        assert_eq!(status, StatusCode::OK);
    }
    routes(&h, "production").await["GET /a"].clone()
}

#[tokio::test]
async fn cumulative_and_delta_exports_give_the_same_totals() {
    let steps = [(10, 2), (25, 7), (5, 12)];
    let delta = totals(false, &steps).await;
    let cumulative = totals(true, &steps).await;
    let sent: u64 = steps.iter().map(|(n, _)| n).sum();
    assert_eq!(delta["requests"], sent);
    for k in ["requests", "errors", "buckets", "p95_ms"] {
        assert_eq!(delta[k], cumulative[k], "{k}");
    }
}

#[tokio::test]
async fn a_restart_or_a_retry_never_counts_twice() {
    let h = harness();
    let s1 = now_ns();
    let cumulative = |route: &str, start: u64, time: u64, count: u64| {
        request(
            &[("service.instance.id", "a")],
            &[duration(
                2,
                vec![Point {
                    start,
                    time,
                    count,
                    sum: Some(count as f64 * 0.05),
                    max: Some(0.3),
                    buckets: one_bucket(6, count),
                    bounds: BOUNDS_S.to_vec(),
                    ..Point::route("GET", route, 200)
                }],
            )],
        )
    };
    let mut sent = 0;
    for (time, count) in [(60, 10), (120, 30)] {
        assert_eq!(
            send_pb(&h, cumulative("/r", s1, s1 + time * SEC, count)).await,
            StatusCode::OK
        );
        sent = count;
    }
    // The exporter retries its last export: nothing new.
    assert_eq!(
        send_pb(&h, cumulative("/r", s1, s1 + 120 * SEC, 30)).await,
        StatusCode::OK
    );
    // The app restarts: a new start time, its counter starts over.
    let s2 = s1 + 150 * SEC;
    let after = 5;
    assert_eq!(
        send_pb(&h, cumulative("/r", s2, s2 + 30 * SEC, after)).await,
        StatusCode::OK
    );
    // A series that started before this server did may have been counted
    // already (before a server restart): only what it adds from now on.
    let old = s1 - 3600 * SEC;
    for (time, count) in [(120, 1000), (180, 1010)] {
        assert_eq!(
            send_pb(&h, cumulative("/old", old, s1 + time * SEC, count)).await,
            StatusCode::OK
        );
    }
    let r = routes(&h, "production").await;
    assert_eq!(r["GET /r"]["requests"], sent + after);
    assert_eq!(r["GET /old"]["requests"], 1010 - 1000);
}

#[tokio::test]
async fn other_bucket_bounds_are_rebucketed_with_p95_as_an_upper_bound() {
    let h = harness();
    let t0 = now_ns();
    // A layout of our own, in milliseconds: every request is in
    // (30 ms, 300 ms].
    let n = 100;
    let body = request(
        &[],
        &[Metric {
            name: "http.server.request.duration".into(),
            unit: "ms".into(),
            data: Data::Histogram(
                1,
                vec![Point {
                    start: t0,
                    time: t0 + 60 * SEC,
                    count: n,
                    sum: Some(n as f64 * 100.0),
                    max: Some(300.0),
                    buckets: vec![0, n, 0],
                    bounds: vec![30.0, 300.0],
                    ..Point::route("GET", "/custom", 200)
                }],
            ),
        }],
    );
    assert_eq!(send_pb(&h, body).await, StatusCode::OK);
    let r = &routes(&h, "production").await["GET /custom"];
    assert_eq!(r["requests"], n);
    assert!(
        r["p95_ms"].as_f64().unwrap() >= 300.0,
        "p95 is never below the source bucket's upper bound: {r}"
    );
    assert!(
        r["max_ms"].as_i64().unwrap() < 1000,
        "milliseconds were not read as seconds: {r}"
    );
}

#[tokio::test]
async fn other_metrics_and_points_are_counted_as_rejected() {
    let h = harness();
    let t0 = now_ns();
    let point = |route: Option<&str>| {
        let mut attrs = vec![serde_json::json!(
            { "key": "http.request.method", "value": { "stringValue": "GET" } }
        )];
        if let Some(r) = route {
            attrs.push(serde_json::json!({ "key": "http.route", "value": { "stringValue": r } }));
        }
        serde_json::json!({
            "attributes": attrs, "startTimeUnixNano": t0, "timeUnixNano": t0 + 60 * SEC,
            "count": 2, "sum": 0.2, "bucketCounts": [2], "explicitBounds": [],
        })
    };
    let histogram = |name: &str, points: Vec<serde_json::Value>| {
        serde_json::json!({ "name": name, "unit": "s",
            "histogram": { "aggregationTemporality": 1, "dataPoints": points } })
    };
    let json = serde_json::json!({ "resourceMetrics": [{ "scopeMetrics": [{ "metrics": [
        histogram("http.server.request.duration", vec![point(Some("/kept")), point(None)]),
        histogram("http.client.request.duration", vec![point(Some("/x")), point(Some("/y"))]),
        { "name": "process.cpu.utilization", "unit": "1",
          "gauge": { "dataPoints": [{ "asDouble": 0.5 }, { "asDouble": 0.6 }, { "asDouble": 0.7 }] } },
    ]}]}]});
    let rejected_sent = 1 + 2 + 3;
    let key = h.key.clone();
    let (status, _, body) = send(
        &h,
        "application/json",
        json.to_string().into_bytes(),
        Some(("x-openlens-key", &key)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let partial = &v["partialSuccess"];
    let rejected: u64 = partial["rejectedDataPoints"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(rejected, rejected_sent);
    assert!(!partial["errorMessage"].as_str().unwrap().is_empty());
    let r = routes(&h, "production").await;
    assert!(r.contains_key("GET /kept"));
    assert!(
        !r.contains_key("GET /x"),
        "client durations are not read yet"
    );
}

#[tokio::test]
async fn a_wrong_or_missing_key_is_unauthorized() {
    let h = harness();
    let body = request(&[], &[]);
    for auth in [
        Some(("x-openlens-key", "pk_wrong")),
        Some(("authorization", "Bearer pk_wrong")),
        None,
    ] {
        let (status, ct, _) = send(&h, "application/x-protobuf", body.clone(), auth).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(ct, "application/x-protobuf");
    }
    let key = h.key.clone();
    let (status, _, _) = send(&h, "text/plain", body, Some(("x-openlens-key", &key))).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn hostile_otlp_bodies_get_a_400_not_a_crash() {
    let h = harness();
    let key = h.key.clone();
    for (i, (ct, body)) in openlens_testkit::otlp::hostile_otlp_bodies()
        .into_iter()
        .enumerate()
    {
        let (status, _, _) = send(&h, ct, body, Some(("x-openlens-key", &key))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {i} ({ct})");
    }
    // The server still works afterwards.
    let (status, _, _) = send(
        &h,
        "application/x-protobuf",
        request(&[], &[]),
        Some(("x-openlens-key", &key)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

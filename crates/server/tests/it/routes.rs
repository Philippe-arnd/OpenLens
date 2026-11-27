//! Server route stats (#593, #594): what the Node SDK sends is read back
//! per route by `GET /api/projects/{id}/routes`, shown on the Errors page,
//! and never counted as user activity. Runtime stats (#610) the same way,
//! per release by `GET /api/projects/{id}/runtime`.

use super::*;

fn stats(uuid: u32, status: i64, count: i64, buckets: [i64; 15]) -> serde_json::Value {
    // One `http.server.request.duration` data point, as the Node SDK sends it.
    serde_json::json!({
        "event": "$request_stats",
        "distinct_id": "$server",
        "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
        "properties": {
            "$lib": "node", "$environment": "production", "$release": "v2",
            "$metric": "http.server.request.duration", "$unit": "s",
            "http.request.method": "POST", "http.route": "/api/auth/sign-in",
            "http.response.status_code": status,
            "count": count, "sum": count as f64 * 0.03, "min": 0.001, "max": 0.9,
            "bucket_counts": buckets, "$window_s": 600,
        },
    })
}

#[tokio::test]
async fn route_stats_are_summed_per_route_shown_and_kept_out_of_user_activity() {
    let h = harness();
    let mut ok = [0; 15];
    ok[3] = 90;
    let mut bad = [0; 15];
    bad[8] = 10;
    let events = vec![
        stats(1, 200, 90, ok),
        stats(2, 503, 10, bad),
        serde_json::json!({
            "event": "$error", "distinct_id": "$server",
            "uuid": "0191f0c0-0000-7000-8000-000000000003",
            "properties": { "$lib": "node", "$environment": "production", "$kind": "server",
                "$message": "Error: db down", "$source": "/app/dist/auth.js:3:4" },
        }),
    ];
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    openlens_server::flush_now(&h.state).unwrap();

    let r = get(&h, "/api/projects/default/routes?days=1", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let route = &v["routes"][0];
    assert_eq!(route["route"], "/api/auth/sign-in");
    assert_eq!(route["requests"], 100);
    assert_eq!(route["errors"], 10);
    assert!(route["p95_ms"].as_f64().unwrap() > route["p50_ms"].as_f64().unwrap());
    assert!(
        route["max_ms"].as_i64().unwrap() >= 900,
        "seconds read as ms"
    );

    // Stats are not user activity: the events view never has them.
    let n = |sql: &'static str| {
        let s = h.state.clone();
        async move {
            s.query.sql("default", sql).await.unwrap().rows[0]["n"]
                .as_i64()
                .unwrap()
        }
    };
    assert_eq!(
        n("SELECT count(*) AS n FROM events WHERE event = '$request_stats'").await,
        0
    );
    assert!(n("SELECT count(*) AS n FROM raw_events WHERE event = '$request_stats'").await > 0);

    let cookie = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/errors", Some(&cookie), false).await).await;
    assert!(page.contains("Server routes") && page.contains("/api/auth/sign-in"));
    assert!(
        page.contains("db down"),
        "the server error is an error group"
    );

    let bad_days = get(&h, "/api/projects/default/routes?days=0", None, true).await;
    assert_eq!(bad_days.status(), StatusCode::BAD_REQUEST);
}

fn runtime(uuid: u32, props: serde_json::Value) -> serde_json::Value {
    // A `$runtime_stats` event as the Node SDK sends it (#610).
    let mut p =
        serde_json::json!({ "$lib": "node", "$environment": "production", "$release": "v3" });
    if let (Some(a), Some(b)) = (p.as_object_mut(), props.as_object()) {
        a.extend(b.clone());
    }
    serde_json::json!({
        "event": "$runtime_stats",
        "distinct_id": "$server",
        "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
        "properties": p,
    })
}

#[tokio::test]
async fn runtime_stats_are_read_per_release_and_never_count_as_users_or_events() {
    let h = harness();
    let events = vec![
        runtime(
            11,
            serde_json::json!({ "process.start": true, "process.memory.usage": 50_000_000, "v8js.memory.heap.used": 20_000_000 }),
        ),
        runtime(
            12,
            serde_json::json!({
                "process.memory.usage": 300_000_000, "v8js.memory.heap.used": 120_000_000,
                "nodejs.eventloop.delay.p50": 0.011, "nodejs.eventloop.delay.p99": 0.25, "$window_s": 600,
            }),
        ),
        runtime(
            13,
            serde_json::json!({
                "process.memory.usage": 100_000_000, "v8js.memory.heap.used": 40_000_000,
                "nodejs.eventloop.delay.p50": 0.010, "nodejs.eventloop.delay.p99": 0.02, "$window_s": 600,
            }),
        ),
    ];
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    openlens_server::flush_now(&h.state).unwrap();

    let r = get(&h, "/api/projects/default/runtime", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let rel = &v["releases"][0];
    assert_eq!(rel["release"], "v3");
    let stats = &rel["stats"];
    assert_eq!(stats["starts"], 1, "one process start");
    assert!(
        stats["rss_max_bytes"].as_i64().unwrap() >= 300_000_000,
        "the largest RSS, not the last or the first"
    );
    let (p50, p99) = (
        stats["loop_delay_p50_ms"].as_f64().unwrap(),
        stats["loop_delay_p99_ms"].as_f64().unwrap(),
    );
    assert!(p99 > p50 && p50 >= 10.0, "seconds read as ms");
    assert!(
        rel["restarts"].is_null(),
        "no release before to compare with"
    );

    // Runtime stats are not user activity: no event, no user in `events`,
    // which the board, the rollups and every KPI read.
    let n = |sql: &'static str| {
        let s = h.state.clone();
        async move {
            s.query.sql("default", sql).await.unwrap().rows[0]["n"]
                .as_i64()
                .unwrap()
        }
    };
    assert_eq!(
        n("SELECT count(*) AS n FROM events WHERE event = '$runtime_stats'").await,
        0
    );
    assert_eq!(
        n("SELECT count(DISTINCT distinct_id) AS n FROM events WHERE distinct_id = '$server'")
            .await,
        0
    );
    assert!(n("SELECT count(*) AS n FROM raw_events WHERE event = '$runtime_stats'").await > 0);

    // Another environment has none: a note, not zeros.
    let r = get(&h, "/api/projects/default/runtime?env=staging", None, true).await;
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert!(v["releases"].as_array().unwrap().is_empty() && v["note"].is_string());
}

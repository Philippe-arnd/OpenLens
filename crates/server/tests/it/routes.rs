//! Server route stats (#593, #594): what the Node SDK sends is read back
//! per route by `GET /api/projects/{id}/routes`, shown on Quality › Routes
//! (#622) and summed up on the Errors page, and never counted as user
//! activity. Runtime stats (#610) the same way, per release by
//! `GET /api/projects/{id}/runtime`.

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
    let cookie = login_cookie(&h).await;
    // Without route stats, the page explains how to send them (rule 11).
    let empty = body_text(get(&h, "/p/default/routes", Some(&cookie), false).await).await;
    assert!(empty.contains("@openlens-obs/node") && !empty.contains("/api/auth/sign-in"));

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

    let routes = body_text(get(&h, "/p/default/routes", Some(&cookie), false).await).await;
    assert!(routes.contains("/api/auth/sign-in") && !routes.contains("@openlens-obs/node"));
    let page = body_text(get(&h, "/p/default/errors", Some(&cookie), false).await).await;
    assert!(
        page.contains("requests answered 5xx"),
        "the Errors page sums the routes up"
    );
    assert!(
        page.contains("db down"),
        "the server error is an error group"
    );

    let bad_days = get(&h, "/api/projects/default/routes?days=0", None, true).await;
    assert_eq!(bad_days.status(), StatusCode::BAD_REQUEST);
}

/// #624: the Errors page tags each group Browser or Server, filters on
/// it, and a server error sent with a recorded browser session links to
/// that replay. A value it does not know shows every group.
#[tokio::test]
async fn the_errors_page_tells_the_origin_and_links_a_server_error_to_its_replay() {
    let h = harness();
    let uuid = |n: u32| format!("0191f0c0-0000-7000-8000-{n:012}");
    let events = vec![
        serde_json::json!({
            "event": "$pageview", "distinct_id": "u1", "session_id": "rec-1", "uuid": uuid(11),
            "properties": { "$environment": "production", "$pathname": "/" },
        }),
        serde_json::json!({
            "event": "$error", "distinct_id": "$server", "session_id": "rec-1", "uuid": uuid(12),
            "properties": { "$lib": "node", "$environment": "production", "$kind": "server",
                "$message": "Error: db down", "$source": "/app/dist/auth.js:3:4" },
        }),
        serde_json::json!({
            "event": "$error", "distinct_id": "u2", "session_id": "s-2", "uuid": uuid(13),
            "properties": { "$environment": "production",
                "$message": "TypeError: x is undefined", "$source": "https://app.test/a.js:1:2" },
        }),
    ];
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    openlens_server::flush_now(&h.state).unwrap();
    let chunk = serde_json::value::RawValue::from_string("[{\"type\":2}]".into()).unwrap();
    openlens_replay::append(
        &h.state.data_dir,
        "default",
        "rec-1",
        "u1",
        0,
        &chunk,
        chrono::Utc::now(),
    )
    .unwrap();

    let cookie = login_cookie(&h).await;
    let page = |q: &'static str| {
        let (h, cookie) = (&h, cookie.clone());
        async move {
            body_text(get(h, &format!("/p/default/errors{q}"), Some(&cookie), false).await).await
        }
    };
    let (server_tag, browser_tag, replay) = (
        "tag tone-accent",
        "tag tone-info",
        "title=\"Replay this session\"",
    );
    let all = page("").await;
    assert!(all.contains("db down") && all.contains("x is undefined"));
    assert!(all.contains(server_tag) && all.contains(browser_tag));
    assert!(
        all.contains(replay),
        "the recorded session links to its replay"
    );

    let server = page("?origin=server").await;
    assert!(server.contains("db down") && !server.contains("x is undefined"));
    assert!(server.contains(replay) && !server.contains(browser_tag));

    let browser = page("?origin=browser").await;
    assert!(browser.contains("x is undefined") && !browser.contains("db down"));
    assert!(!browser.contains(server_tag) && !browser.contains(replay));

    let hostile = page("?origin=%27%3Cscript%3E").await;
    assert!(hostile.contains("db down") && hostile.contains("x is undefined"));

    let api = get(&h, "/api/projects/default/errors?origin=server", None, true).await;
    assert_eq!(api.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(api).await).unwrap();
    let groups = v["groups"].as_array().unwrap();
    assert!(!groups.is_empty() && groups.iter().all(|g| g["server"] == true));
    assert!(
        groups[0]["linked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session"] == "rec-1" && s["replay"] == true)
    );
}

/// A window's data point of `GET /api/export` answered `status`, its
/// slowest request taking `max` seconds in session `session` (#607).
fn window(uuid: u32, hours_ago: i64, status: i64, max: f64, session: &str) -> serde_json::Value {
    let start = chrono::Utc::now() - chrono::Duration::hours(hours_ago);
    let mut b = [0; 15];
    b[12] = 4;
    serde_json::json!({
        "event": "$request_stats",
        "distinct_id": "$server",
        "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
        "timestamp": start.to_rfc3339(),
        "properties": {
            "$lib": "node", "$environment": "production", "$release": "v4",
            "$metric": "http.server.request.duration", "$unit": "s",
            "http.request.method": "GET", "http.route": "/api/export",
            "http.response.status_code": status,
            "count": 4, "sum": max * 2.0, "min": 0.01, "max": max,
            "bucket_counts": b, "$window_s": 600,
            "exemplars": [{
                "value": max,
                "time": (start + chrono::Duration::minutes(3)).to_rfc3339(),
                "trace_id": "5b8efff798038103d269b633813fc60c",
                // A user id sent anyway (an older SDK) is never read back (D-089).
                "filtered_attributes": { "session.id": session, "user.id": "u1" },
            }],
        },
    })
}

#[tokio::test]
async fn the_routes_api_returns_the_slowest_request_and_first_5xx_of_the_period_across_windows() {
    let h = harness();
    let events = vec![
        window(21, 5, 200, 0.4, "early"),
        window(22, 3, 200, 6.5, "slowest"),
        window(23, 1, 200, 0.9, "late"),
        window(24, 4, 500, 0.2, "first-failure"),
        window(25, 2, 500, 3.0, "later-failure"),
    ];
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    openlens_server::flush_now(&h.state).unwrap();

    let r = get(&h, "/api/projects/default/routes?days=1", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let route = v["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["route"] == "/api/export")
        .unwrap();
    let slowest = &route["slowest"];
    assert_eq!(slowest["session_id"], "slowest", "{route}");
    assert!(slowest["ms"].as_f64().unwrap() >= route["p95_ms"].as_f64().unwrap());
    assert_eq!(slowest["trace_id"], "5b8efff798038103d269b633813fc60c");
    let first = &route["first_5xx"];
    assert_eq!(first["session_id"], "first-failure", "{route}");
    assert!(first["status"].as_i64().unwrap() >= 500);
    assert!(first["time"].as_str().unwrap() < slowest["time"].as_str().unwrap());
    assert!(
        slowest.get("user").is_none() && first.get("user").is_none(),
        "an exemplar keeps the session, never the user"
    );
    assert!(!route.to_string().contains("\"u1\""));
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

/// #726, D-100: health checks are not route stats. The fixed health routes
/// and the project's own uptime health path are dropped at ingestion; a
/// real route is kept, and a server error on a health route too.
#[tokio::test]
async fn health_check_route_stats_are_not_stored_and_real_routes_are() {
    let h = harness();
    h.state
        .meta
        .set_project_setting("default", "alerts", r#"{"health_path":"/internal/up"}"#)
        .unwrap();
    let point = |uuid: u32, method: &str, route: &str| {
        serde_json::json!({
            "event": "$request_stats",
            "distinct_id": "$server",
            "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
            "properties": {
                "$lib": "node", "$environment": "production",
                "$metric": "http.server.request.duration", "$unit": "s",
                "http.request.method": method, "http.route": route,
                "http.response.status_code": 200,
                "count": 60, "sum": 0.06, "min": 0.001, "max": 0.002,
                "bucket_counts": [60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "$window_s": 60,
            },
        })
    };
    let events = vec![
        point(31, "GET", "/health"),
        point(32, "HEAD", "/api/health/"),
        point(33, "GET", "/internal/up"),
        point(34, "GET", "/api/items"),
        serde_json::json!({
            "event": "$error", "distinct_id": "$server",
            "uuid": "0191f0c0-0000-7000-8000-000000000035",
            "properties": { "$lib": "node", "$environment": "production", "$kind": "server",
                "http.route": "/health", "$message": "Error: health db down",
                "$source": "/app/dist/health.js:1:2" },
        }),
    ];
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    openlens_server::flush_now(&h.state).unwrap();

    let rows = h
        .state
        .query
        .sql(
            "default",
            "SELECT event, properties FROM raw_events WHERE event IN ('$request_stats', '$error')",
        )
        .await
        .unwrap()
        .rows;
    let stored = |event: &str, needle: &str| {
        rows.iter().any(|r| {
            r["event"] == event && r["properties"].as_str().is_some_and(|p| p.contains(needle))
        })
    };
    assert!(
        stored("$request_stats", "/api/items"),
        "a real route is kept"
    );
    for health in ["\"/health\"", "/api/health", "/internal/up"] {
        assert!(
            !stored("$request_stats", health),
            "{health} is a health check"
        );
    }
    assert!(
        stored("$error", "health db down"),
        "a failing health check is still an error"
    );
}

//! Server route stats (#593, #594): what the Node SDK sends is read back
//! per route by `GET /api/projects/{id}/routes`, shown on Quality › Routes
//! (#622) and summed up on the Errors page, and never counted as user
//! activity.

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
    assert!(empty.contains("@openlens/node") && !empty.contains("/api/auth/sign-in"));

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
    assert!(routes.contains("/api/auth/sign-in") && !routes.contains("@openlens/node"));
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

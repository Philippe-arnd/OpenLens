//! The API and the project pages. Moved out of main.rs as is (#307).

use super::*;

#[tokio::test]
async fn api_needs_the_bearer_token_and_runs_sql() {
    let h = harness();
    assert_eq!(
        get(&h, "/api/projects", None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/api/projects", Some(&cookie), false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "a browser session is not an API credential"
    );
    assert_eq!(
        get(&h, "/api/projects", None, true).await.status(),
        StatusCode::OK
    );

    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let resp = router(h.state.clone())
        .oneshot(
            Request::post("/api/projects/default/sql")
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"sql":"SELECT count(*) AS n FROM events"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["rows"][0]["n"], 2);
}

#[tokio::test]
async fn board_renders_numbers_once_data_exists() {
    let h = harness();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default", Some(&cookie), false).await;
    let body = axum::body::to_bytes(page.into_body(), 1 << 20)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains(r#"id="out-h""#));
    assert!(html.contains("owner"), "the role shows up");
    // The page CSP forbids inline styles, so the markup must not rely on them.
    assert!(
        !html.contains(" style="),
        "inline style attribute in the board"
    );
    let events = get(&h, "/p/default/events", Some(&cookie), false).await;
    assert_eq!(events.status(), StatusCode::OK);
}

#[tokio::test]
async fn feature_map_put_refuses_errors_and_stores_valid_maps() {
    let h = harness();
    let put = |body: &'static str| {
        let st = h.state.clone();
        async move {
            let resp = router(st)
                .oneshot(
                    Request::put("/api/projects/default/feature-map")
                        .header("authorization", "Bearer adm_test")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = resp.status();
            let v: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), 1 << 20)
                    .await
                    .unwrap(),
            )
            .unwrap();
            (status, v)
        }
    };
    let (status, v) = put("features:\n  - key: Bad\n    name: x\n").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["stored"], false);
    let issues = v["issues"].as_array().unwrap();
    assert!(
        issues.iter().any(
            |i| i["level"] == "error" && i["path"].as_str().unwrap().starts_with("features[0]")
        )
    );

    let (status, v) = put("roles:\n  - key: admin\n    match: [owner]\nfeatures:\n  - key: search\n    name: Search\n    entry: { routes: ['/search'] }\n").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["version"], 1);

    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default", Some(&cookie), false).await;
    let html = String::from_utf8_lossy(
        &axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .to_string();
    assert!(html.contains("Feature map v1"));
    assert!(
        html.contains("Not used in"),
        "search is declared but unused"
    );
    assert!(html.contains(">admin<"), "owner resolves to admin");
}

#[tokio::test]
async fn backfill_imports_persons_and_past_events_once() {
    let h = harness();
    let body = concat!(
        r#"{"type":"person","id":"old1","role":"owner","created_at":"2026-09-01T10:00:00Z","last_seen_at":"2026-11-10T10:00:00Z"}"#,
        "\n",
        r#"{"event":"entry_saved","distinct_id":"old1","timestamp":"2026-10-02T09:00:00Z"}"#,
        "\n",
        "not json\n",
    );
    let send = || {
        let st = h.state.clone();
        async move {
            router(st)
                .oneshot(
                    Request::post("/api/projects/default/import")
                        .header("authorization", "Bearer adm_test")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };
    let first = send().await;
    assert_eq!(first.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(first.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["persons"], 1);
    assert_eq!(v["accepted_events"], 3, "identify + seen + entry_saved");
    assert_eq!(v["rejected"].as_array().unwrap().len(), 1);
    assert_eq!(
        send().await.status(),
        StatusCode::CONFLICT,
        "same chunk twice is refused"
    );

    let person = h.state.meta.person("default", "old1").unwrap().unwrap();
    assert_eq!(person.role.as_deref(), Some("owner"));
    let r = h
        .state
        .query
        .sql(
            "default",
            "SELECT min(date) AS d, count(*) AS n FROM events",
        )
        .await
        .unwrap();
    assert_eq!(
        r.rows[0]["d"], "2026-09-01",
        "history lands in its own past partitions"
    );
    assert_eq!(r.rows[0]["n"], 3);
}

#[tokio::test]
async fn export_returns_one_day_as_jsonl_and_validates_the_date() {
    let h = harness();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let resp = get(
        &h,
        &format!("/api/projects/default/export?date={today}"),
        None,
        true,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-openlens-truncated"], "false");
    let body = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(body.lines().count(), 2);
    assert!(
        body.lines()
            .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
    );
    for bad in ["2026-1-01", "2026-01-01' OR '1'='1", "yesterday"] {
        let resp = get(
            &h,
            &format!(
                "/api/projects/default/export?date={}",
                bad.replace(' ', "%20").replace('\'', "%27")
            ),
            None,
            true,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(
        get(
            &h,
            &format!("/api/projects/default/export?date={today}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn flags_target_canonical_roles_and_need_the_token_to_change() {
    let h = harness();
    let map_put = router(h.state.clone())
        .oneshot(
            Request::put("/api/projects/default/feature-map")
                .header("authorization", "Bearer adm_test")
                .body(Body::from("roles:\n  - key: admin\n    match: [owner]\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(map_put.status(), StatusCode::OK);
    let put = |auth: bool| {
        let st = h.state.clone();
        async move {
            let mut req = Request::put("/api/projects/default/flags/new-editor")
                .header("content-type", "application/json");
            if auth {
                req = req.header("authorization", "Bearer adm_test");
            }
            router(st)
                .oneshot(
                    req.body(Body::from(
                        r#"{"key":"x","enabled":true,"rollout":100,"roles":["admin"]}"#,
                    ))
                    .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(put(false).await, StatusCode::UNAUTHORIZED);
    assert_eq!(put(true).await, StatusCode::OK);

    let flags_for = |role: &'static str| {
        let (st, key) = (h.state.clone(), h.key.clone());
        async move {
            let resp = router(st)
                .oneshot(
                    Request::get(format!("/flags?key={key}&distinct_id=u1&role={role}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let v: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), 1 << 20)
                    .await
                    .unwrap(),
            )
            .unwrap();
            v["flags"].clone()
        }
    };
    assert_eq!(
        flags_for("owner").await,
        serde_json::json!(["new-editor"]),
        "owner resolves to admin"
    );
    assert_eq!(flags_for("guest").await, serde_json::json!([]));

    let cookie = login_cookie(&h).await;
    let cross = router(h.state.clone())
        .oneshot(
            Request::post("/p/default/flags/new-editor")
                .header("cookie", &cookie)
                .header("origin", "https://evil.example")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("rollout=0"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        cross.status(),
        StatusCode::FORBIDDEN,
        "cross-site form posts are refused"
    );
}

#[tokio::test]
async fn theme_cookie_sets_the_page_theme_and_redirects_back_safely() {
    let h = harness();
    let resp = router(h.state.clone())
        .oneshot(
            Request::get("/theme/dark")
                .header("host", "lens.test")
                .header("referer", "http://lens.test/p/default/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.headers()["location"], "/p/default/events");
    assert!(
        resp.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .starts_with("ol_theme=dark")
    );
    let evil = router(h.state.clone())
        .oneshot(
            Request::get("/theme/light")
                .header("host", "lens.test")
                .header("referer", "https://evil.example/x")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(evil.headers()["location"], "/", "never redirect off-site");

    let page = get(&h, "/login", Some("ol_theme=dark"), false).await;
    let html = String::from_utf8_lossy(
        &axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .to_string();
    assert!(html.contains(r#"data-theme="dark""#));
    let page = get(&h, "/login", Some("ol_theme=<script>"), false).await;
    let html = String::from_utf8_lossy(
        &axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .to_string();
    assert!(
        !html.contains("data-theme"),
        "only known themes are applied"
    );
}

#[tokio::test]
async fn errors_page_and_api_show_groups() {
    let h = harness();
    let body = serde_json::json!({"key": h.key, "batch": [
        {"event": "$error", "distinct_id": "u1", "session_id": "s1", "properties": {"$message": "boom 42", "$source": "https://x.test/a.js:1:2", "$role": "owner"}},
        {"event": "$error", "distinct_id": "u2", "session_id": "s2", "properties": {"$message": "boom 7", "$source": "https://x.test/a.js:9:9"}}
    ]}).to_string().into_bytes();
    post(&h, "/capture", body, false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let resp = get(&h, "/api/projects/default/errors", None, true).await;
    let v: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["groups"].as_array().unwrap().len(), 1);
    assert_eq!(v["groups"][0]["users"], 2);
    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default/errors", Some(&cookie), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8_lossy(
        &axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .to_string();
    assert!(html.contains("boom"));
}

#[tokio::test]
async fn replay_chunks_are_stored_and_read_back_in_order() {
    let h = harness();
    let chunk = |seq: u32, events: &str| {
        format!(
            r#"{{"key":"{}","session_id":"s-1","distinct_id":"u1","seq":{seq},"events":{events}}}"#,
            h.key
        )
        .into_bytes()
    };
    assert_eq!(
        post(&h, "/r", chunk(1, r#"[{"t":2}]"#), false).await.0,
        StatusCode::OK
    );
    assert_eq!(
        post(&h, "/r", chunk(0, r#"[{"t":1}]"#), false).await.0,
        StatusCode::OK
    );
    // Hostile input: path traversal, wrong shape, wrong key.
    let bad = format!(
        r#"{{"key":"{}","session_id":"../x","distinct_id":"u","seq":0,"events":[]}}"#,
        h.key
    );
    assert_eq!(
        post(&h, "/r", bad.into_bytes(), false).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(&h, "/r", chunk(2, "{}"), false).await.0,
        StatusCode::BAD_REQUEST
    );
    let nokey = br#"{"key":"nope","session_id":"s","distinct_id":"u","seq":0,"events":[]}"#;
    assert_eq!(
        post(&h, "/r", nokey.to_vec(), false).await.0,
        StatusCode::UNAUTHORIZED
    );

    let project = h.state.meta.project_by_key(&h.key).unwrap().unwrap().id;
    let list = format!("/api/projects/{project}/replays");
    assert_eq!(
        get(&h, &list, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let resp = get(&h, &list, None, true).await;
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["replays"][0]["session_id"], "s-1");
    let resp = get(&h, &format!("{list}/s-1"), None, true).await;
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["distinct_id"], "u1");
    assert_eq!(v["events"], serde_json::json!([{"t":1},{"t":2}]));
    assert_eq!(
        get(&h, &format!("{list}/missing"), None, true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn replay_pages_list_play_and_lock_down() {
    let h = harness();
    let body = format!(
        r#"{{"key":"{}","session_id":"sess-9","distinct_id":"ana","seq":0,"events":[{{"type":4,"timestamp":1}},{{"type":2,"timestamp":2}}]}}"#,
        h.key
    );
    assert_eq!(
        post(&h, "/r", body.into_bytes(), false).await.0,
        StatusCode::OK
    );
    let project = h.state.meta.project_by_key(&h.key).unwrap().unwrap().id;
    let base = format!("/p/{project}/replays");
    assert_eq!(
        get(&h, &base, None, false).await.status(),
        StatusCode::SEE_OTHER
    );
    let cookie = login_cookie(&h).await;
    let text = |r: axum::response::Response| async move {
        String::from_utf8(
            axum::body::to_bytes(r.into_body(), 1 << 22)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    };
    let list = text(get(&h, &base, Some(&cookie), false).await).await;
    assert!(list.contains("sess-9") && list.contains("ana"));
    let page = get(
        &h,
        &format!("{base}/sess-9?at=2026-01-01%2010:00:00"),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let csp = page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'none'"));
    let html = text(page).await;
    assert!(html.contains("/assets/player.js") && html.contains("events.json"));
    // Other pages keep the no-script policy.
    let ev = get(&h, &format!("/p/{project}/events"), Some(&cookie), false).await;
    assert!(
        !ev.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("script-src")
    );
    let json = text(
        get(
            &h,
            &format!("{base}/sess-9/events.json"),
            Some(&cookie),
            false,
        )
        .await,
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["events"].as_array().unwrap().len(), 2);
    assert_eq!(
        get(&h, &format!("{base}/sess-9/events.json"), None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&h, &format!("{base}/nope"), Some(&cookie), false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&h, "/assets/player.js", None, false).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn breakdown_splits_by_a_fixed_dimension_only() {
    let h = harness();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let body = |r: axum::response::Response| async move {
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()
    };
    let v = body(get(&h, "/api/projects/default/breakdown?by=role", None, true).await).await;
    assert!(
        v["role_rule"]
            .as_str()
            .unwrap()
            .starts_with("role at event time")
    );
    assert!(
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"] == "owner")
    );
    let v = body(get(&h, "/api/projects/default/breakdown?by=page", None, true).await).await;
    assert!(
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"] == "/")
    );
    // Anything else is refused, so `by` can never inject SQL.
    let bad = get(
        &h,
        "/api/projects/default/breakdown?by=distinct_id;DROP",
        None,
        true,
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let noauth = get(&h, "/api/projects/default/breakdown?by=role", None, false).await;
    assert_eq!(noauth.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn source_maps_resolve_error_stacks_per_release() {
    let h = harness();
    let put = |uri: String, body: &'static str, bearer: bool| {
        let st = h.state.clone();
        async move {
            let mut req = Request::put(uri);
            if bearer {
                req = req.header("authorization", "Bearer adm_test");
            }
            router(st)
                .oneshot(req.body(Body::from(body)).unwrap())
                .await
                .unwrap()
                .status()
        }
    };
    const MAP: &str = r#"{"version":3,"sources":["src/billing/Invoice.tsx"],"names":["pay"],"mappings":"AAAA,UAEIA"}"#;
    let base = "/api/projects/default/sourcemaps";
    assert_eq!(
        put(format!("{base}/v7/app.3f9a.js"), MAP, false).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        put(format!("{base}/v7/app.3f9a.js"), "{}", true).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        put(format!("{base}/v7/..%2Fx"), MAP, true).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        put(format!("{base}/v7/app.3f9a.js"), MAP, true).await,
        StatusCode::OK
    );

    let stack =
        "TypeError: x is undefined\\n    at pay (https://app.example.com/assets/app.3f9a.js:1:14)";
    let batch = format!(
        r#"{{"key":"{}","batch":[{{"event":"$error","distinct_id":"u1","properties":{{"$message":"x is undefined","$stack":"{stack}","$source":"https://app.example.com/assets/app.3f9a.js:1:14","$release":"v7"}}}}]}}"#,
        h.key
    );
    assert_eq!(
        post(&h, "/capture", batch.into_bytes(), false).await.0,
        StatusCode::OK
    );
    openlens_server::flush_now(&h.state).unwrap();
    let resp = get(&h, "/api/projects/default/errors", None, true).await;
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let o = &v["groups"][0]["original"];
    assert_eq!(o["source"], "src/billing/Invoice.tsx", "{v}");
    assert_eq!(o["line"], 3);
    assert_eq!(o["name"], "pay");
    // The dashboard shows the original position too.
    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default/errors", Some(&cookie), false).await;
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("src/billing/Invoice.tsx:3"));
}

#[tokio::test]
async fn user_timeline_tells_one_persons_story() {
    let h = harness();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let resp = get(&h, "/api/projects/default/people/u1", None, true).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["person"]["role"], "owner");
    assert_eq!(v["role_history"][0]["role"], "owner");
    assert!(v["events"].as_array().unwrap().len() >= 2);
    assert_eq!(
        get(&h, "/api/projects/default/people/nobody", None, true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&h, "/api/projects/default/people/u1", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn several_projects_open_on_the_project_switcher() {
    let h = harness();
    let cookie = login_cookie(&h).await;
    // One project: straight to its board.
    let r = get(&h, "/", Some(&cookie), false).await;
    assert_eq!(r.headers()["location"], "/p/default");
    h.state.meta.create_project("Journal", None).unwrap();
    let r = get(&h, "/", Some(&cookie), false).await;
    assert_eq!(r.headers()["location"], "/projects");
    let page = get(&h, "/projects", Some(&cookie), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("/p/journal") && html.contains("/p/default"));
    assert!(!html.contains(" style="));
    assert_eq!(
        get(&h, "/projects", None, false).await.status(),
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn click_heatmap_draws_positions_without_inline_styles() {
    let h = harness();
    let mut batch = vec![
        serde_json::json!({"event":"$pageview","distinct_id":"u1","properties":{"$pathname":"/pricing"}}),
    ];
    for i in 0..12 {
        batch.push(
            serde_json::json!({"event":"$click","distinct_id":"u1","properties":{
                "$pathname":"/pricing","$x": 0.5,"$y": 300 + i,"$vw": 1280, "$rage": i == 0
            }}),
        );
    }
    let body = serde_json::json!({"key": h.key, "batch": batch})
        .to_string()
        .into_bytes();
    assert_eq!(post(&h, "/capture", body, false).await.0, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let cookie = login_cookie(&h).await;
    let page = get(
        &h,
        "/p/default/heatmap?path=%2Fpricing",
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        html.contains("<circle") && html.contains("12 clicks with a position"),
        "{html}"
    );
    assert!(html.contains("dot rage"));
    assert!(!html.contains(" style="));
    // The Heatmap tab names no page: it opens the most viewed one (#592).
    let tab = get(&h, "/p/default/heatmap", Some(&cookie), false).await;
    assert_eq!(tab.status(), StatusCode::OK);
    let tab = String::from_utf8(
        axum::body::to_bytes(tab.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(tab.contains("12 clicks with a position"), "{tab}");
    // Before any page view, the tab shows its empty state, not an error.
    let other = harness();
    let other_cookie = login_cookie(&other).await;
    let empty = get(&other, "/p/default/heatmap", Some(&other_cookie), false).await;
    assert_eq!(empty.status(), StatusCode::OK);
    let empty = String::from_utf8(
        axum::body::to_bytes(empty.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(empty.contains("heat-empty"), "{empty}");
    let json = get(
        &h,
        "/p/default/heatmap.json?path=%2Fpricing",
        Some(&cookie),
        false,
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(json.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["map"]["cells"][0][0], 25, "x = 0.5 → column 25");
    assert_eq!(v["map"]["cells"][0][1], 15, "y = 300 → row 15");
    assert_eq!(v["map"]["clicks"], 12);
    assert_eq!(
        get(&h, "/p/default/heatmap.json?path=%2F", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn weekly_digest_compares_this_week_with_last() {
    let h = harness();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let resp = get(&h, "/api/projects/default/digest", None, true).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["digest"]["users"]["now"], 1);
    assert_eq!(v["digest"]["users"]["before"], 0);
    let md = v["markdown"].as_str().unwrap();
    assert!(md.contains("**Active users:** 1 new this week"), "{md}");
    assert_eq!(
        get(&h, "/api/projects/default/digest", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default/digest", Some(&cookie), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("This week vs last") && !html.contains(" style="));
}

/// API errors carry a stable code from a closed list, with the message,
/// and `error` for older clients (#312).
#[tokio::test]
async fn api_errors_carry_a_stable_code() {
    let h = harness();
    let r = get(&h, "/api/projects/nope/board", None, true).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let v: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
            .unwrap();
    assert_eq!(v["code"], "not_found", "{v}");
    assert_eq!(v["message"], v["error"]);
    assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));
}

/// A date range that reads more than 30 days of events shows its cost
/// before it runs (#514, D-061): the page shows the estimate and a "Run
/// it" button (a background job, #515), the API answers 409 with it, and both run with `confirm=1`;
/// a short range runs at once.
#[tokio::test]
async fn a_long_range_shows_its_cost_and_runs_once_confirmed() {
    let h = harness();
    let now = chrono::Utc::now();
    // One event a day for the last 40 days, none rolled up.
    {
        let evs: Vec<_> = (1..=40)
            .map(|d| openlens_event::Event {
                uuid: format!("cost-{d}"),
                project: "default".into(),
                event: "$pageview".into(),
                distinct_id: "u1".into(),
                session_id: Some(format!("s{d}")),
                timestamp: now - chrono::Duration::days(d),
                received_at: now,
                url: None,
                path: Some("/".into()),
                feature: None,
                role: None,
                account: None,
                plan: None,
                environment: "production".into(),
                release: None,
                flags: None,
                properties: "{}".into(),
            })
            .collect();
        let mut w = h.state.writer.lock().unwrap();
        w.append(&evs).unwrap();
        w.flush().unwrap();
    }
    let from = |days: i64| (now - chrono::Duration::days(days)).format("%Y-%m-%d");
    // 21 days against the 21 before: 40 stored days, none rolled up.
    let long = format!("from={}", from(20));
    let api = format!("/api/projects/default/board?{long}");
    let r = get(&h, &api, None, true).await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert_eq!(v["code"], "conflict");
    assert!(v["estimate"]["raw_days"].as_u64().unwrap() > 30, "{v}");
    assert_eq!(v["estimate"]["confirm"], true);
    let cookie = login_cookie(&h).await;
    // The page asks too, with a form that runs it.
    let page = get(&h, &format!("/p/default?{long}"), Some(&cookie), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(html.contains("Run it"), "{html}");
    assert!(
        html.contains("/board/jobs"),
        "a form that starts a job: {html}"
    );
    assert!(!html.contains(r#"id="out-h""#), "the board did not run");
    // Confirmed, it runs, and says what it read.
    let r = get(&h, &format!("{api}&confirm=1"), None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let b: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert!(b["window"]["estimate"]["raw_days"].as_u64().unwrap() > 30);
    let page = get(
        &h,
        &format!("/p/default?{long}&confirm=1"),
        Some(&cookie),
        false,
    )
    .await;
    assert!(body_text(page).await.contains(r#"id="out-h""#));
    // A short range runs at once, and its estimate says it needs nothing.
    let short = format!("from={}", from(5));
    let r = get(
        &h,
        &format!("/api/projects/default/board/estimate?{short}"),
        None,
        true,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let e: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert_eq!(e["confirm"], false, "{e}");
    let r = get(
        &h,
        &format!("/api/projects/default/board?{short}"),
        None,
        true,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
}

/// Pages link the stylesheet at its content-hashed URL, cached for a year;
/// only the current hash answers, and the old URL still works (#579).
#[tokio::test]
async fn the_stylesheet_has_an_immutable_hashed_url() {
    let h = harness();
    let login = body_text(get(&h, "/login", None, false).await).await;
    let href = openlens_ui::css_href();
    assert!(login.contains(&format!("href=\"{href}\"")), "{login}");
    assert!(!login.contains("href=\"/assets/app.css\""));
    let r = get(&h, href, None, false).await;
    assert_eq!(r.status(), StatusCode::OK);
    let cache = r.headers()["cache-control"].to_str().unwrap().to_owned();
    assert!(cache.contains("immutable"), "{cache}");
    assert_eq!(body_text(r).await, openlens_ui::CSS);
    for stale in ["/assets/app/0000000000000000.css", "/assets/app/x"] {
        assert_eq!(
            get(&h, stale, None, false).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let old = get(&h, "/assets/app.css", None, false).await;
    assert_eq!(old.status(), StatusCode::OK);
    assert!(
        !old.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
}

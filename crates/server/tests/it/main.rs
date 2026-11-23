//! Integration tests for `openlens-server` (one binary per crate).

// Test helpers may unwrap: a failure here is a test failure (clippy.toml
// already allows it inside #[test] functions; this extends it to helpers).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use openlens_meta::Meta;
use openlens_server::{AppState, router};
use openlens_storage::{FlushPolicy, Writer, count_rows};
use tower::ServiceExt;

struct Harness {
    _dir: tempfile::TempDir,
    state: AppState,
    key: String,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let meta = Meta::in_memory().unwrap();
    let key = meta.ensure_default_project(None).unwrap().api_key;
    let (writer, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let state = AppState {
        meta: Arc::new(meta),
        writer: Arc::new(Mutex::new(writer)),
        query: openlens_query::QueryEngine::new(dir.path(), openlens_query::Limits::default()),
        data_dir: dir.path().to_path_buf(),
        admin_token: "adm_test".into(),
    };
    Harness {
        _dir: dir,
        state,
        key,
    }
}

async fn post(
    h: &Harness,
    uri: &str,
    body: Vec<u8>,
    gzip: bool,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::post(uri).header("content-type", "application/json");
    if gzip {
        req = req.header("content-encoding", "gzip");
    }
    let resp = router(h.state.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

fn batch(key: &str) -> Vec<u8> {
    serde_json::json!({
        "key": key,
        "batch": [
            {"event": "$identify", "distinct_id": "u1", "properties": {"$role": "owner"}},
            {"event": "$pageview", "distinct_id": "u1", "properties": {"$pathname": "/"}},
            {"event": "", "distinct_id": "u1"}
        ]
    })
    .to_string()
    .into_bytes()
}

#[tokio::test]
async fn healthz_answers_ok_and_unknown_routes_do_not() {
    let h = harness();
    let ok = router(h.state.clone())
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let missing = router(h.state.clone())
        .oneshot(Request::get("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(missing.status().is_client_error());
}

#[tokio::test]
async fn captured_events_are_stored_and_identify_updates_the_person() {
    let h = harness();
    let (status, body) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["accepted"], 2);
    assert_eq!(body["rejected"].as_array().unwrap().len(), 1);

    openlens_server::flush_now(&h.state).unwrap();
    assert_eq!(count_rows(&h.state.data_dir, "default").unwrap(), 2);
    let person = h.state.meta.person("default", "u1").unwrap().unwrap();
    assert_eq!(person.role.as_deref(), Some("owner"));
}

#[tokio::test]
async fn gzip_bodies_are_accepted_and_bombs_are_refused() {
    let h = harness();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&batch(&h.key)).unwrap();
    let (status, _) = post(&h, "/e", enc.finish().unwrap(), true).await;
    assert_eq!(status, StatusCode::OK);

    // 20 MB of zeros compresses to a few kB: must be refused, not inflated.
    let mut bomb = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    bomb.write_all(&vec![b' '; 20 * 1024 * 1024]).unwrap();
    let (status, _) = post(&h, "/capture", bomb.finish().unwrap(), true).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn wrong_or_missing_keys_are_unauthorized() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch("pk_wrong"), false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post(&h, "/capture", br#"{"batch":[]}"#.to_vec(), false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn hostile_bodies_get_client_errors_not_crashes() {
    let h = harness();
    for body in openlens_testkit::hostile_capture_bodies() {
        let (status, _) = post(&h, &format!("/capture?key={}", h.key), body, false).await;
        assert!(
            status.is_client_error() || status == StatusCode::OK,
            "unexpected {status}"
        );
    }
}

#[tokio::test]
async fn sdk_is_served_as_cacheable_javascript() {
    let h = harness();
    let resp = router(h.state.clone())
        .oneshot(Request::get("/c.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers()["content-type"].to_str().unwrap().to_owned();
    assert!(ct.starts_with("application/javascript"));
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(body.len() > 1000);
    let rec = get(&h, "/r.js", None, false).await;
    assert_eq!(rec.status(), StatusCode::OK);
    let body = axum::body::to_bytes(rec.into_body(), 1 << 22)
        .await
        .unwrap();
    assert!(body.len() > 10_000);
}

async fn get(
    h: &Harness,
    uri: &str,
    cookie: Option<&str>,
    bearer: bool,
) -> axum::response::Response {
    let mut req = Request::get(uri);
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    if bearer {
        req = req.header("authorization", "Bearer adm_test");
    }
    router(h.state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn login_cookie(h: &Harness) -> String {
    let resp = router(h.state.clone())
        .oneshot(
            Request::post("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("token=adm_test"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    let set = resp.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax"));
    set.split(';').next().unwrap().to_owned()
}

#[tokio::test]
async fn dashboard_requires_a_session_and_pages_are_locked_down() {
    let h = harness();
    let anon = get(&h, "/p/default", None, false).await;
    assert!(anon.status().is_redirection());
    let forged = get(
        &h,
        "/p/default",
        Some("ol_session=9999999999.deadbeef"),
        false,
    )
    .await;
    assert!(
        forged.status().is_redirection(),
        "forged cookie must not pass"
    );

    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default", Some(&cookie), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csp = page.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"));
    let body = axum::body::to_bytes(page.into_body(), 1 << 20)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("warming up"),
        "empty project shows the onboarding state"
    );
    assert!(html.contains(&h.key), "and the capture key to wire the app");
}

#[tokio::test]
async fn wrong_token_login_fails() {
    let h = harness();
    let resp = router(h.state.clone())
        .oneshot(
            Request::post("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("token=nope"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().get("set-cookie").is_none());
}

#[tokio::test]
async fn login_links_open_the_board_exactly_once() {
    let h = harness();
    let resp = router(h.state.clone())
        .oneshot(
            Request::post("/api/login-links")
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
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
    let url = body["url"].as_str().unwrap();
    let path = &url[url.find("/l/").unwrap()..];
    let first = get(&h, path, None, false).await;
    assert!(first.status().is_redirection());
    assert!(first.headers().get("set-cookie").is_some());
    let second = get(&h, path, None, false).await;
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
}

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
    assert!(html.contains("Daily active users"));
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
    assert!(html.contains("Never used"), "search is declared but unused");
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

async fn invite(h: &Harness, body: serde_json::Value) -> String {
    let resp = router(h.state.clone())
        .oneshot(
            Request::post("/api/login-links")
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    let url = v["url"].as_str().unwrap();
    let r = get(h, &url[url.find("/l/").unwrap()..], None, false).await;
    r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn post_form(h: &Harness, uri: &str, cookie: &str, body: &str) -> StatusCode {
    router(h.state.clone())
        .oneshot(
            Request::post(uri)
                .header("cookie", cookie)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn viewers_see_their_projects_only_and_change_nothing() {
    let h = harness();
    h.state.meta.create_project("Other", None).unwrap();
    let viewer = invite(
        &h,
        serde_json::json!({"email": "Ana@Example.com", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    assert_eq!(
        get(&h, "/p/default", Some(&viewer), false).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&h, "/p/other", Some(&viewer), false).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&h, "/p/other/events", Some(&viewer), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags/beta",
            &viewer,
            "enabled=on&rollout=50"
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&h, "/users", Some(&viewer), false).await.status(),
        StatusCode::FORBIDDEN
    );
    // One visible project: home goes straight to it.
    assert_eq!(
        get(&h, "/", Some(&viewer), false).await.headers()["location"],
        "/p/default"
    );

    let admin = invite(
        &h,
        serde_json::json!({"email": "boss@example.com", "role": "admin"}),
    )
    .await;
    let page = get(&h, "/users", Some(&admin), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("ana@example.com") && html.contains("boss@example.com"));
    assert!(!html.contains(" style="));
    assert_eq!(
        get(&h, "/p/other", Some(&admin), false).await.status(),
        StatusCode::OK
    );

    // Removing a user ends their session at once.
    let id = h
        .state
        .meta
        .users()
        .unwrap()
        .into_iter()
        .find(|u| u.email == "ana@example.com")
        .unwrap()
        .id;
    let del = router(h.state.clone())
        .oneshot(
            Request::delete(format!("/api/users/{id}"))
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    assert_eq!(
        get(&h, "/p/default", Some(&viewer), false).await.status(),
        StatusCode::SEE_OTHER
    );
    // Bad input is refused.
    let bad = router(h.state.clone())
        .oneshot(
            Request::post("/api/login-links")
                .header("authorization", "Bearer adm_test")
                .body(Body::from(r#"{"email":"x@y.z","role":"god"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn micro_surveys_are_targeted_served_with_flags_and_summarised() {
    let h = harness();
    let put = |body: serde_json::Value| {
        let st = h.state.clone();
        async move {
            router(st)
                .oneshot(
                    Request::put("/api/projects/default/surveys/useful")
                        .header("authorization", "Bearer adm_test")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };
    let good = serde_json::json!({"key":"useful","question":"How useful was that?","kind":"rating","trigger":"entry_saved","enabled":true,"sample":100});
    assert_eq!(put(serde_json::json!({"key":"useful","question":"?","kind":"essay","trigger":"x","enabled":true,"sample":100})).await, StatusCode::BAD_REQUEST);
    assert_eq!(put(good.clone()).await, StatusCode::OK);
    let flags = |id: &'static str| {
        let (st, key) = (h.state.clone(), h.key.clone());
        async move {
            let r = router(st)
                .oneshot(
                    Request::get(format!("/flags?key={key}&distinct_id={id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            serde_json::from_slice::<serde_json::Value>(
                &axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap(),
            )
            .unwrap()
        }
    };
    let v = flags("u1").await;
    assert_eq!(v["surveys"][0]["key"], "useful");
    assert_eq!(v["surveys"][0]["trigger"], "entry_saved");
    assert!(
        v["surveys"][0].get("sample").is_none(),
        "targeting stays server-side"
    );
    let mut off = good.clone();
    off["enabled"] = false.into();
    assert_eq!(put(off).await, StatusCode::OK);
    assert_eq!(flags("u1").await["surveys"].as_array().unwrap().len(), 0);

    let batch = serde_json::json!({"key": h.key, "batch": [
        {"event":"$identify","distinct_id":"a","properties":{"$role":"owner"}},
        {"event":"$survey","distinct_id":"a","properties":{"$survey":"useful","$answer":5}},
        {"event":"$survey","distinct_id":"b","properties":{"$survey":"useful","$answer":3}},
        {"event":"$survey","distinct_id":"c","properties":{"$survey":"useful","$dismissed":true}},
        {"event":"$survey","distinct_id":"d","properties":{"$survey":"why","$answer":"too slow on mobile"}}
    ]});
    assert_eq!(
        post(&h, "/capture", batch.to_string().into_bytes(), false)
            .await
            .0,
        StatusCode::OK
    );
    openlens_server::flush_now(&h.state).unwrap();
    let r = get(&h, "/api/projects/default/surveys", None, true).await;
    let v: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
            .unwrap();
    let res = v["results"].as_array().unwrap();
    let useful = res.iter().find(|r| r["key"] == "useful").unwrap();
    assert_eq!(useful["mean"], 4.0);
    assert_eq!(useful["dismissed"], 1);
    let why = res.iter().find(|r| r["key"] == "why").unwrap();
    assert_eq!(why["latest_text"][0], "too slow on mobile");
    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default/surveys", Some(&cookie), false).await;
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("How useful was that?") && !html.contains(" style="));
    let js = get(&h, "/s.js", None, false).await;
    assert_eq!(js.status(), StatusCode::OK);
}

/// First setup: the owner lands on the welcome page, creates their admin
/// account and continues as that user; once someone has an account, the
/// token goes straight to the board.
#[tokio::test]
async fn first_owner_login_creates_the_admin_account() {
    let h = harness();
    let login = || async {
        router(h.state.clone())
            .oneshot(
                Request::post("/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("token=adm_test"))
                    .unwrap(),
            )
            .await
            .unwrap()
    };
    let first = login().await;
    assert_eq!(first.headers()["location"], "/welcome");
    let owner = first.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        get(&h, "/welcome", Some(&owner), false).await.status(),
        StatusCode::OK
    );
    assert!(
        get(&h, "/welcome", None, false)
            .await
            .status()
            .is_redirection()
    );

    let created = router(h.state.clone())
        .oneshot(
            Request::post("/welcome")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=Phil%40Example.com"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.headers()["location"], "/");
    let user = created.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_ne!(user, owner, "the session now belongs to the new account");
    let users = h.state.meta.users().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(
        (users[0].email.as_str(), users[0].role.as_str()),
        ("phil@example.com", "admin")
    );
    assert_eq!(
        get(&h, "/users", Some(&user), false).await.status(),
        StatusCode::OK
    );
    assert!(
        get(&h, "/welcome", Some(&user), false)
            .await
            .status()
            .is_redirection(),
        "a user already has an account"
    );

    assert_eq!(
        login().await.headers()["location"],
        "/",
        "set up: straight to the board"
    );
}

/// Without SMTP and a public URL there is no email sign-in, and asking for a
/// link says the same thing whatever the address.
#[tokio::test]
async fn email_login_is_off_without_smtp_and_never_tells_who_exists() {
    let h = harness();
    let page = get(&h, "/login", None, false).await;
    let body = axum::body::to_bytes(page.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("Email me a login link"));
    for email in ["nobody@example.com", "x"] {
        let r = router(h.state.clone())
            .oneshot(
                Request::post("/login/email")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("email={email}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Check your inbox"));
    }
}

/// Through a proxy, one client address gets a burst, then 429 with
/// Retry-After (which the SDK honours by retrying later).
#[tokio::test]
async fn capture_is_rate_limited_per_client_behind_a_proxy() {
    let h = harness();
    let mut limited = None;
    for i in 0..5000 {
        let r = router(h.state.clone())
            .oneshot(
                Request::post("/capture")
                    .header("content-type", "text/plain")
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::from(batch(&h.key)))
                    .unwrap(),
            )
            .await
            .unwrap();
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(r.headers()["retry-after"], "5");
            limited = Some(i);
            break;
        }
    }
    let i = limited.expect("a client sending batches non-stop is limited");
    assert!(
        i >= 400,
        "the burst lets a normal client through first ({i})"
    );
    // Another client is not affected.
    let other = router(h.state.clone())
        .oneshot(
            Request::post("/capture")
                .header("content-type", "text/plain")
                .header("x-forwarded-for", "198.51.100.7")
                .body(Body::from(batch(&h.key)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::OK);
}

/// Deleting a project needs its id twice, then removes its metadata and
/// files; its key stops working.
#[tokio::test]
async fn deleting_a_project_needs_confirmation_and_removes_everything() {
    let h = harness();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let del = |q: &'static str| {
        router(h.state.clone()).oneshot(
            Request::delete(format!("/api/projects/default{q}"))
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
                .unwrap(),
        )
    };
    assert_eq!(del("").await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        del("?confirm=other").await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert!(h.state.data_dir.join("events/project=default").exists());
    assert_eq!(
        del("?confirm=default").await.unwrap().status(),
        StatusCode::OK
    );
    assert!(h.state.meta.projects().unwrap().is_empty());
    assert!(!h.state.data_dir.join("events/project=default").exists());
    let after = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(after.0, StatusCode::UNAUTHORIZED, "the key is gone");
    assert_eq!(
        del("?confirm=default").await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

/// A project's time zone (D-017): set at creation or later, IANA names only.
#[tokio::test]
async fn project_time_zone_is_set_and_checked() {
    let h = harness();
    let send = |method: &'static str, path: &'static str, body: &'static str| {
        router(h.state.clone()).oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
    };
    let created = send(
        "POST",
        "/api/projects",
        r#"{"name":"Paris app","timezone":"Europe/Paris"}"#,
    )
    .await
    .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let p = h.state.meta.project("paris-app").unwrap().unwrap();
    assert_eq!(p.timezone, "Europe/Paris");
    assert_eq!(
        h.state.meta.project("default").unwrap().unwrap().timezone,
        "UTC"
    );
    let bad = send(
        "POST",
        "/api/projects",
        r#"{"name":"Bad","timezone":"Mars/Olympus"}"#,
    )
    .await
    .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let ok = send(
        "PATCH",
        "/api/projects/default",
        r#"{"timezone":"America/New_York"}"#,
    )
    .await
    .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(
        h.state.meta.project("default").unwrap().unwrap().timezone,
        "America/New_York"
    );
    let bad = send("PATCH", "/api/projects/default", r#"{"timezone":"x'; --"}"#)
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let missing = send("PATCH", "/api/projects/nope", r#"{"timezone":"UTC"}"#)
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    // The board still loads with a zone set.
    let board = router(h.state.clone())
        .oneshot(
            Request::get("/api/projects/default/board")
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(board.status(), StatusCode::OK);
}

#[tokio::test]
async fn project_settings_do_what_the_cli_does() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let page = get(&h, "/p/default/settings", Some(&owner), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let key = h.state.meta.project("default").unwrap().unwrap().api_key;
    assert!(html.contains(&key) && html.contains("Wiring check"));
    assert!(!html.contains(" style="));

    assert_eq!(
        post_form(
            &h,
            "/p/default/settings",
            &owner,
            "name=Journal&app_url=https%3A%2F%2Fjournal.example%2Fapp&timezone=Europe%2FParis"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let p = h.state.meta.project("default").unwrap().unwrap();
    assert_eq!(
        (p.name.as_str(), p.timezone.as_str()),
        ("Journal", "Europe/Paris")
    );
    assert_eq!(
        h.state
            .meta
            .project_setting("default", "app_url")
            .unwrap()
            .as_deref(),
        Some("https://journal.example")
    );
    assert_eq!(
        post_form(&h, "/p/default/settings", &owner, "name=J&timezone=Nowhere").await,
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &owner, "").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &owner, "confirm=yes").await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.project_by_key(&key).unwrap().is_none());

    let icon = get(&h, "/p/default/icon", Some(&owner), false).await;
    assert_eq!(icon.headers()["content-type"], "image/svg+xml");

    assert_eq!(
        post_form(&h, "/projects", &owner, "name=Kanban&timezone=").await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.project("kanban").unwrap().is_some());
    assert_eq!(
        post_form(
            &h,
            "/p/kanban/settings/delete",
            &owner,
            "confirm=kanban+app"
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(&h, "/p/kanban/settings/delete", &owner, "confirm=Kanban").await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.project("kanban").unwrap().is_none());

    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &viewer, "confirm=yes").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(&h, "/projects", &viewer, "name=Nope").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &h,
            "/p/default/export?date=2026-01-01",
            Some(&viewer),
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn team_crud_and_the_viewer_time_zone() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let ana = invite(
        &h,
        serde_json::json!({"email": "ana@example.com", "role": "admin"}),
    )
    .await;
    let id = h
        .state
        .meta
        .user_by_email("ana@example.com")
        .unwrap()
        .unwrap()
        .id;
    // Ana is the only admin user: she can't be removed or disabled.
    assert_eq!(
        post_form(&h, &format!("/users/{id}/disable"), &owner, "").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            &format!("/users/{id}/delete"),
            &owner,
            "confirm=ana%40example.com"
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            &format!("/users/{id}"),
            &owner,
            "name=Ana&email=ana%40example.com&role=admin&projects="
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        h.state.meta.user(&id).unwrap().unwrap().name.as_deref(),
        Some("Ana")
    );
    let page = get(&h, "/users?q=ana", Some(&owner), false).await;
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("ana@example.com") && html.contains("active"));

    // Her own time zone drives what she reads.
    assert_eq!(
        post_form(&h, "/me", &ana, "name=Ana&timezone=Asia%2FTokyo").await,
        StatusCode::SEE_OTHER
    );
    let ev = get(&h, "/p/default/events", Some(&ana), false).await;
    let html = String::from_utf8(
        axum::body::to_bytes(ev.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Times in Asia/Tokyo"), "viewer zone shown");
    assert_eq!(
        post_form(&h, "/me", &ana, "timezone=Mars").await,
        StatusCode::BAD_REQUEST
    );

    // A second admin lets the first be disabled: her session ends at once.
    invite(
        &h,
        serde_json::json!({"email": "bo@example.com", "role": "admin"}),
    )
    .await;
    assert_eq!(
        post_form(&h, &format!("/users/{id}/disable"), &owner, "").await,
        StatusCode::SEE_OTHER
    );
    assert_ne!(
        get(&h, "/p/default", Some(&ana), false).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        post_form(&h, &format!("/users/{id}/delete"), &owner, "confirm=nope").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            &format!("/users/{id}/delete"),
            &owner,
            "confirm=ana%40example.com"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.user(&id).unwrap().is_none());
}

#[tokio::test]
async fn viewers_comment_on_replays_and_delete_only_their_own() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let ana = invite(
        &h,
        serde_json::json!({"email": "ana@example.com", "role": "viewer"}),
    )
    .await;
    let bo = invite(
        &h,
        serde_json::json!({"email": "bo@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(
            &h,
            "/p/default/replays/s1/comments",
            &ana,
            "at=12400&body=Form+error+here"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let c = h.state.meta.replay_comments("default", "s1").unwrap();
    assert_eq!(
        (c.len(), c[0].at_ms, c[0].author.as_str()),
        (1, 12_400, "ana@example.com")
    );
    let path = format!("/p/default/replays/s1/comments/{}/delete", c[0].id);
    assert_eq!(post_form(&h, &path, &bo, "").await, StatusCode::FORBIDDEN);
    // Viewers still can't change anything else under the project.
    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &ana, "confirm=yes").await,
        StatusCode::FORBIDDEN
    );
    let api = get(&h, "/api/projects/default/replays/s1/comments", None, true).await;
    assert_eq!(api.status(), StatusCode::OK);
    assert_eq!(
        post_form(&h, &path, &owner, "").await,
        StatusCode::SEE_OTHER
    );
    assert!(
        h.state
            .meta
            .replay_comments("default", "s1")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn flags_and_surveys_are_managed_from_the_dashboard() {
    let h = harness();
    let owner = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/p/default/flags-edit", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "key=new-editor&description=Editor&rollout=20&roles=staff&enabled=on"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "key=new-editor&rollout=5"
        )
        .await,
        StatusCode::BAD_REQUEST,
        "duplicate key"
    );
    assert_eq!(
        post_form(&h, "/p/default/flags-edit", &owner, "key=Bad+Key").await,
        StatusCode::BAD_REQUEST
    );
    // Rename keeps one flag.
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "original=new-editor&key=editor&rollout=30"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let keys: Vec<String> = h.state.meta.flags("default").unwrap();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].contains("\"editor\"") && keys[0].contains("30"));
    assert_eq!(
        post_form(&h, "/p/default/flags/editor/delete", &owner, "confirm=nope").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags/editor/delete",
            &owner,
            "confirm=editor"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.flags("default").unwrap().is_empty());

    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys-edit",
            &owner,
            "key=useful&question=How+useful%3F&kind=choice&choices=a&trigger=saved&sample=10"
        )
        .await,
        StatusCode::BAD_REQUEST,
        "a choice needs 2 options"
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys-edit",
            &owner,
            "key=useful&question=How+useful%3F&kind=rating&trigger=saved&sample=10&enabled=on"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(h.state.meta.surveys("default").unwrap().len(), 1);
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys/useful/delete",
            &viewer,
            "confirm=useful"
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys/useful/delete",
            &owner,
            "confirm=useful"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.surveys("default").unwrap().is_empty());
}

async fn body_text(r: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(r.into_body(), 1 << 22)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn people_breakdown_and_sql_pages() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let page = get(&h, "/p/default/people/u1", Some(&owner), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(html.contains("$pageview") && html.contains("owner"));
    assert!(!html.contains(" style="));
    assert_eq!(
        get(&h, "/p/default/people/nobody", Some(&owner), false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for by in ["role", "environment", "release", "page"] {
        let r = get(
            &h,
            &format!("/p/default/breakdown?by={by}"),
            Some(&owner),
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK, "{by}");
    }
    assert_eq!(
        get(&h, "/p/default/breakdown?by=nope", Some(&owner), false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&h, "/p/default/sql", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    let run = router(h.state.clone())
        .oneshot(
            Request::post("/p/default/sql")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "sql=SELECT+event%2C+count(*)+AS+n+FROM+events+GROUP+BY+event&format=csv",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run.status(), StatusCode::OK);
    let csv = body_text(run).await;
    assert!(
        csv.starts_with("event,n\n") && csv.contains("$pageview,1"),
        "{csv}"
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        get(&h, "/p/default/sql", Some(&viewer), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(&h, "/p/default/sql", &viewer, "sql=SELECT+1").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn the_feature_map_is_read_checked_and_saved_from_the_dashboard() {
    let h = harness();
    let owner = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/p/default/features", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/features",
            &owner,
            "action=save&yaml=features%3A+%5B%7Bkey%3A+%22Bad+Key%22%7D%5D"
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a map with errors is not saved"
    );
    assert!(h.state.meta.feature_map("default").unwrap().is_none());
    let yaml = "features:\n  - key: search\n    name: Search\n    success: search_done\n    entry: { routes: ['/search'] }\n";
    let form = |action: &str| format!("action={action}&yaml={}", openlens_ui::url_enc(yaml));
    let check = router(h.state.clone())
        .oneshot(
            Request::post("/p/default/features")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form("check")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(check.status(), StatusCode::OK);
    let html = body_text(check).await;
    assert!(
        html.contains("Changes against the stored map") && html.contains("<ins>+ features:"),
        "diff shown"
    );
    assert_eq!(
        post_form(&h, "/p/default/features", &owner, &form("save")).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(h.state.meta.feature_map("default").unwrap().unwrap().0, 1);
    let page = body_text(get(&h, "/p/default/features", Some(&owner), false).await).await;
    assert!(page.contains("not seen") && page.contains("Version 1"));
    let json = body_text(get(&h, "/p/default/features/map.json", Some(&owner), false).await).await;
    assert!(json.contains("\"search_done\""), "{json}");
    assert_eq!(
        get(&h, "/p/default/features/map.yaml?v=1", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(&h, "/p/default/features", &viewer, &form("save")).await,
        StatusCode::FORBIDDEN
    );
    let ro = body_text(get(&h, "/p/default/features", Some(&viewer), false).await).await;
    assert!(!ro.contains("<textarea"), "read-only for viewers");
}

#[tokio::test]
async fn storage_and_backups_from_the_dashboard() {
    let h = harness();
    let owner = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/settings/storage", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        get(&h, "/settings/storage", Some(&viewer), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(&h, "/settings/storage/backups/run", &viewer, "").await,
        StatusCode::FORBIDDEN
    );

    // S3: the secret is sealed, never shown, kept when left empty.
    assert_eq!(
        post_form(
            &h,
            "/settings/storage/s3",
            &owner,
            "action=save&bucket=my-bucket&region=eu-west-1&endpoint=http%3A%2F%2F127.0.0.1%3A1&access_key=AKID&secret=TopSecret123"
        )
        .await,
        StatusCode::OK
    );
    let sealed = h.state.meta.setting("s3:secret").unwrap().unwrap();
    assert!(!sealed.contains("TopSecret123"));
    let page = body_text(get(&h, "/settings/storage", Some(&owner), false).await).await;
    assert!(page.contains("my-bucket") && !page.contains("TopSecret123"));
    post_form(
        &h,
        "/settings/storage/s3",
        &owner,
        "action=save&bucket=my-bucket&access_key=AKID2&secret=",
    )
    .await;
    assert_eq!(h.state.meta.setting("s3:secret").unwrap().unwrap(), sealed);
    let test = router(h.state.clone())
        .oneshot(
            Request::post("/settings/storage/s3")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("action=test"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(test).await.contains("Test failed"), "no S3 there");
    post_form(&h, "/settings/storage/s3", &owner, "action=remove").await;
    assert!(
        h.state
            .meta
            .setting("s3:bucket")
            .unwrap()
            .is_none_or(|v| v.is_empty())
    );

    // Plan validation, then a manual backup that lands in the history.
    let bad = body_text(
        router(h.state.clone())
            .oneshot(
                Request::post("/settings/storage/backups")
                    .header("cookie", &owner)
                    .header("origin", "http://localhost:8686")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("schedule=daily&hour=3&keep=0"))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(bad.contains("keep 1 to 60"));
    post_form(
        &h,
        "/settings/storage/backups",
        &owner,
        "schedule=daily&hour=3&weekday=1&keep=2",
    )
    .await;
    assert_eq!(
        post_form(&h, "/settings/storage/backups/run", &owner, "").await,
        StatusCode::SEE_OTHER
    );
    for _ in 0..100 {
        if h.state.meta.setting("backup:history").unwrap().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let hist = h.state.meta.setting("backup:history").unwrap().unwrap();
    assert!(hist.contains("\"ok\":true"), "{hist}");
    let dl = get(
        &h,
        "/settings/storage/backups/latest.tar.gz",
        Some(&owner),
        false,
    )
    .await;
    assert_eq!(dl.status(), StatusCode::OK);
    assert_eq!(dl.headers()["content-type"], "application/gzip");
}

#[tokio::test]
async fn page_views_get_device_browser_and_os_but_never_the_address() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$pageview", "distinct_id": "u1", "properties": {"$pathname": "/", "$country": "ZZ", "$lang": "fr-FR", "$sw": 390}},
            {"event": "signup", "distinct_id": "u1"}
        ]
    })
    .to_string();
    let resp = router(h.state.clone())
        .oneshot(
            Request::post("/capture")
                .header("content-type", "application/json")
                .header("x-forwarded-for", "203.0.113.9")
                .header("user-agent", "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 Version/17.5 Mobile/15E148 Safari/604.1")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql(
            "default",
            "SELECT event, properties FROM raw_events ORDER BY event",
        )
        .await
        .unwrap();
    let all = serde_json::to_string(&r.rows).unwrap();
    assert!(!all.contains("203.0.113.9"), "no address stored");
    assert!(!all.contains("ZZ"), "client-sent country replaced");
    let pv = r.rows.iter().find(|x| x["event"] == "$pageview").unwrap()["properties"]
        .as_str()
        .unwrap();
    assert!(
        pv.contains(r#""$device":"phone""#)
            && pv.contains(r#""$browser":"Safari""#)
            && pv.contains(r#""$os":"iOS""#),
        "{pv}"
    );
    let other = r.rows.iter().find(|x| x["event"] == "signup").unwrap()["properties"]
        .as_str()
        .unwrap();
    assert!(!other.contains("$device"), "only page views are enriched");
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/audience", Some(&owner), false).await).await;
    assert!(page.contains("phone") && page.contains("Safari") && !page.contains(" style="));
}

#[tokio::test]
async fn the_feature_flow_page_and_api() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$pageview", "distinct_id": "u1", "session_id": "s1", "properties": {"$pathname": "/", "$feature": "search"}},
            {"event": "click", "distinct_id": "u1", "session_id": "s1", "properties": {"$feature": "cart"}}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    for u in [
        "/p/default/flows",
        "/p/default/flows?to=success",
        "/p/default/flows?from=cart",
    ] {
        let r = get(&h, u, Some(&owner), false).await;
        assert_eq!(r.status(), StatusCode::OK, "{u}");
        assert!(!body_text(r).await.contains(" style="));
    }
    let api = body_text(get(&h, "/api/projects/default/flows", None, true).await).await;
    let v: serde_json::Value = serde_json::from_str(&api).unwrap();
    assert!(v["sessions"].as_i64().unwrap() <= 1, "{api}");
}

#[tokio::test]
async fn the_live_view_shows_who_is_here_and_refreshes() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let r = get(
        &h,
        "/p/default/live?env=production&x=evil",
        Some(&owner),
        false,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers()["refresh"],
        "10;url=/p/default/live?env=production"
    );
    let html = body_text(r).await;
    assert!(html.contains("Active now") && html.contains("$pageview") && !html.contains(" style="));
}

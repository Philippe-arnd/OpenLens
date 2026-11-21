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

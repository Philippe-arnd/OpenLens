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

mod access;
mod agents;
mod allowlist;
mod auth;
mod board_jobs;
mod environments;
mod features;
mod hooks;
mod ingest;
mod manage;
mod pages;
mod privacy;
mod releases;
mod team;
mod totp;

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
    let meta = Arc::new(meta);
    let state = AppState {
        query: openlens_query::QueryEngine::new(dir.path(), openlens_query::Limits::default())
            .with_identity(openlens_server::identity::source(meta.clone())),
        meta,
        writer: Arc::new(Mutex::new(writer)),
        data_dir: dir.path().to_path_buf(),
        admin_token_hash: openlens_meta::sha256_hex("adm_test").into(),
        session_key: "session-key-for-tests".into(),
        boards: Default::default(),
        flushing: Default::default(),
        metrics: Default::default(),
        disk_space: openlens_server::disk_space,
        session_gen: Default::default(),
        caches: Default::default(),
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

/// The "Continue" button of a login link page.
async fn use_link(h: &Harness, path: &str) -> axum::response::Response {
    router(h.state.clone())
        .oneshot(
            Request::post(path)
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
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
    let r = use_link(h, &url[url.find("/l/").unwrap()..]).await;
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

async fn post_form_body(h: &Harness, uri: &str, cookie: &str, body: &str) -> (StatusCode, String) {
    let r = router(h.state.clone())
        .oneshot(
            Request::post(uri)
                .header("cookie", cookie)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = r.status();
    (s, body_text(r).await)
}

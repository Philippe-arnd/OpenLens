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

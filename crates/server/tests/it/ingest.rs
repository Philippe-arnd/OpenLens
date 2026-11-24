//! Capture, health and the SDK. Moved out of main.rs as is (#307).

use super::*;

#[tokio::test]
async fn healthz_answers_ok_and_unknown_routes_do_not() {
    let h = harness();
    let ok = router(h.state.clone())
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let v = router(h.state.clone())
        .oneshot(Request::get("/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(v.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(v.into_body(), 1 << 16).await.unwrap())
            .unwrap();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
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

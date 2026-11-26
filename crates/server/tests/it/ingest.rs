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

/// People counted through `events`, by the server's engine.
async fn users(q: &openlens_query::QueryEngine) -> i64 {
    q.sql(
        "default",
        "SELECT count(DISTINCT distinct_id) AS n FROM events",
    )
    .await
    .unwrap()
    .rows[0]["n"]
        .as_i64()
        .unwrap()
}

/// #450: a `$identify` received after anonymous events still gives them to
/// the user, through the identity recorded at capture.
#[tokio::test]
async fn an_identify_after_anonymous_events_counts_the_person_once() {
    let h = harness();
    let send = |batch: serde_json::Value| {
        let body = serde_json::json!({ "key": h.key, "batch": batch }).to_string();
        post(&h, "/capture", body.into_bytes(), false)
    };
    let (s, _) = send(serde_json::json!([
        {"event": "$pageview", "distinct_id": "anon-1", "properties": {"$pathname": "/"}},
        {"event": "$pageview", "distinct_id": "anon-1", "properties": {"$pathname": "/pricing"}}
    ]))
    .await;
    assert_eq!(s, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let (s, _) = send(serde_json::json!([
        {"event": "$identify", "distinct_id": "alice", "properties": {"$anon_distinct_id": "anon-1"}},
        {"event": "note_saved", "distinct_id": "alice"}
    ]))
    .await;
    assert_eq!(s, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    openlens_server::identity::backfill(&h.state, "default")
        .await
        .unwrap();

    let kept = h.state.meta.identities("default").unwrap().unwrap();
    assert_eq!(kept.aliases, vec![("anon-1".into(), "alice".into())]);
    assert_eq!(kept.persons.len(), 1);
    assert_eq!(users(&h.state.query).await, 1);
}

/// #450: data stored before identity was kept at ingestion (no tables) is
/// backfilled once from the files, with the same results.
#[tokio::test]
async fn identity_is_backfilled_from_existing_events_with_the_same_results() {
    let h = harness();
    let at = chrono::Utc::now();
    let ev = |n: usize, event: &str, who: &str, props: &str| openlens_event::Event {
        uuid: format!("old-{n}"),
        project: "default".into(),
        event: event.into(),
        distinct_id: who.into(),
        session_id: None,
        timestamp: at - chrono::Duration::minutes(10 - n as i64),
        received_at: at,
        url: None,
        path: Some("/".into()),
        feature: None,
        role: None,
        account: None,
        plan: None,
        environment: "production".into(),
        release: None,
        flags: None,
        properties: props.into(),
    };
    // Written straight to storage, as an older release left them.
    {
        let mut w = h.state.writer.lock().unwrap();
        w.append(&[
            ev(0, "$pageview", "anon-1", "{}"),
            ev(1, "$identify", "alice", r#"{"$anon_distinct_id":"anon-1"}"#),
            ev(2, "$pageview", "anon-2", "{}"),
            ev(
                3,
                "$identify",
                "bob",
                r#"{"$anon_distinct_id":"anon-2","$internal":true}"#,
            ),
            ev(4, "$pageview", "anon-3", "{}"),
        ])
        .unwrap();
        w.flush().unwrap();
    }
    assert!(h.state.meta.identities("default").unwrap().is_none());
    let files = openlens_query::QueryEngine::new(&h.state.data_dir, Default::default());
    let persons = "SELECT person, first_seen, internal FROM persons ORDER BY person";
    let before = files.sql("default", persons).await.unwrap().rows;

    openlens_server::identity::backfill(&h.state, "default")
        .await
        .unwrap();
    let kept = h.state.meta.identities("default").unwrap().unwrap();
    assert_eq!(kept.aliases.len(), 2);
    assert_eq!(kept.persons.len(), before.len());
    let ready = openlens_query::QueryEngine::new(&h.state.data_dir, Default::default())
        .with_identity(openlens_server::identity::source(h.state.meta.clone()));
    assert_eq!(users(&ready).await, users(&files).await);
    assert_eq!(ready.sql("default", persons).await.unwrap().rows, before);
    assert_eq!(ready.identity_scans(), 0, "the files are not scanned again");
    // Running it again changes nothing.
    openlens_server::identity::backfill(&h.state, "default")
        .await
        .unwrap();
    assert_eq!(h.state.meta.identities("default").unwrap().unwrap(), kept);
}

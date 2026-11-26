//! The public status page (#561): off is a 404 with no body, on it shows
//! incidents and 90 bars without anything from inside the project, the
//! badge and the JSON agree with the page, and a flood from one address
//! is limited without touching ingestion.

use super::*;

async fn put(h: &Harness, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let r = router(h.state.clone())
        .oneshot(
            Request::put("/api/projects/default/status-page")
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or_default())
}

async fn anonymous(h: &Harness, uri: &str, from: Option<&str>) -> axum::response::Response {
    let mut req = Request::get(uri);
    if let Some(ip) = from {
        req = req.header("x-forwarded-for", ip);
    }
    router(h.state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// The default project, renamed, with an address, a health path, failed
/// checks with error texts and a closed incident with its cause.
fn seed(h: &Harness) {
    let m = &h.state.meta;
    m.rename_project("default", "Acme Shop").unwrap();
    m.set_project_setting("default", "app_url", "https://app.internal.example")
        .unwrap();
    m.set_project_setting("default", "alerts", r#"{"health_path":"/secret-health"}"#)
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    for k in 0..20 {
        let ok = k % 7 != 2;
        let err = (!ok).then_some("connect to 10.1.2.3:8080 refused");
        m.record_check(
            "default",
            now - 60 * k,
            ok,
            ok.then_some(200),
            Some(90 + k),
            err,
        )
        .unwrap();
    }
    let inc = m
        .start_incident(
            "default",
            "down",
            now - 5400,
            "HTTP 502 from https://app.internal.example/secret-health",
        )
        .unwrap();
    m.end_incident(inc, now - 4300).unwrap();
}

fn turn_on() -> serde_json::Value {
    serde_json::json!({
        "slug": "acme",
        "enabled": true,
        "components": [{"environment": "production", "name": "Web app", "response_time": true}]
    })
}

#[tokio::test]
async fn a_page_that_is_off_or_unknown_is_a_404_with_no_body() {
    let h = harness();
    let r = anonymous(&h, "/status/acme", None).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert!(body_text(r).await.is_empty());
    let (s, _) = put(&h, serde_json::json!({"slug": "acme", "enabled": false})).await;
    assert_eq!(s, StatusCode::OK);
    for uri in [
        "/status/acme",
        "/status/acme.json",
        "/status/acme/badge.svg",
        "/status/acme/feed.atom",
        "/status/acme/icon",
    ] {
        let r = anonymous(&h, uri, None).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{uri}");
        assert!(body_text(r).await.is_empty(), "{uri}");
    }
    for slug in openlens_testkit::hostile_slugs()
        .into_iter()
        .filter(|s| !s.is_empty())
    {
        let r = anonymous(&h, &format!("/status/{}", url_encode(&slug)), None).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{slug:?}");
        assert!(body_text(r).await.is_empty(), "{slug:?}");
    }
    // Saving a hostile slug is refused, never stored.
    let (s, _) = put(&h, serde_json::json!({"slug": "../admin", "enabled": true})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn the_page_shows_the_incident_and_the_bars_and_nothing_from_inside() {
    let h = harness();
    seed(&h);
    let (s, a) = call_api(
        &h,
        "/api/projects/default/announcements",
        serde_json::json!({"title": "Slower exports", "level": "degraded",
            "body": "<script>alert(1)</script> we are on it"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let (s, v) = put(&h, turn_on()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["path"], "/status/acme");

    let r = anonymous(&h, "/status/acme", None).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().get("set-cookie").is_none(), "no cookie");
    let html = body_text(r).await;
    assert!(html.contains("Acme Shop") && html.contains("Web app"));
    assert!(html.contains("http-equiv=\"refresh\""));
    assert_eq!(html.matches("class=\"uptime-day ").count(), 90);
    assert!(html.contains("Detected incidents"), "the seeded incident");
    assert!(html.contains("Slower exports"));
    assert!(!html.contains("<script"), "no script, posted or ours");
    assert!(!html.contains("/p/"), "no link into the dashboard");

    let r = anonymous(&h, "/status/acme.json", None).await;
    assert_eq!(r.status(), StatusCode::OK);
    let json = body_text(r).await;
    for page in [&html, &json] {
        for inside in [
            "internal.example",
            "secret-health",
            "10.1.2.3",
            "refused",
            "HTTP 502",
        ] {
            assert!(!page.contains(inside), "{inside:?} leaked");
        }
    }
    assert!(!json.contains("default"), "the project id leaked: {json}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    // The badge and the JSON agree with the page.
    let a = v["components"][0]["availability"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["window"] == "24h")
        .and_then(|f| f["availability"].as_f64())
        .unwrap();
    let shown = format!("{:.2}%", a * 100.0);
    assert!(html.contains(&shown), "{shown} on the page");
    let r = anonymous(&h, "/status/acme/badge.svg?window=24h", None).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(body_text(r).await.contains(&shown));
    let r = anonymous(&h, "/status/acme/badge.svg", None).await;
    assert!(body_text(r).await.contains(v["state"].as_str().unwrap()));
    let r = anonymous(&h, "/status/acme/badge.svg?window=1y", None).await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    let r = anonymous(&h, "/status/acme/feed.atom", None).await;
    assert_eq!(r.status(), StatusCode::OK);
    let feed = body_text(r).await;
    assert!(feed.contains("Slower exports"));
    assert!(feed.contains("&lt;script&gt;") && !feed.contains("<script"));

    // Turned off, it is gone at once (the cache is cleared).
    put(&h, serde_json::json!({"enabled": false})).await;
    let r = anonymous(&h, "/status/acme", None).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

async fn call_api(
    h: &Harness,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let r = router(h.state.clone())
        .oneshot(
            Request::post(uri)
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or_default())
}

#[tokio::test]
async fn a_flood_from_one_address_is_limited_and_ingestion_still_answers() {
    let h = harness();
    seed(&h);
    put(&h, turn_on()).await;
    let app = router(h.state.clone());
    let (mut ok, mut limited) = (0u32, 0u32);
    for _ in 0..1_000 {
        let r = app
            .clone()
            .oneshot(
                Request::get("/status/acme")
                    .header("x-forwarded-for", "203.0.113.7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        match r.status() {
            StatusCode::OK => ok += 1,
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            s => panic!("unexpected {s}"),
        }
    }
    assert!(limited > ok, "{ok} served, {limited} limited");
    // Another address still gets the page.
    let r = anonymous(&h, "/status/acme", Some("198.51.100.4")).await;
    assert_eq!(r.status(), StatusCode::OK);
    // The same address still sends events: the buckets are apart.
    let r = router(h.state.clone())
        .oneshot(
            Request::post("/capture")
                .header("content-type", "application/json")
                .header("x-forwarded-for", "203.0.113.7")
                .body(Body::from(batch(&h.key)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

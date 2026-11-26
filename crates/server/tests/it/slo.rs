//! Availability and the SLO (#560): the page, the status route and the
//! report route show the figures of one engine command; a target needs
//! write access and is audited.

use super::*;

async fn call(
    h: &Harness,
    method: &str,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let r = router(h.state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or_default())
}

#[tokio::test]
async fn the_page_the_status_and_the_report_agree_and_a_target_needs_write() {
    let h = harness();
    let m = &h.state.meta;
    let now = chrono::Utc::now().timestamp() / 60 * 60;
    // Ninety minutes of checks ending an hour and a half ago, ten down.
    let (from, to) = (now - 3 * 3600, now - 90 * 60);
    let (s, e) = (from + 1800, from + 2400);
    let id = m.start_incident("default", "down", s, "HTTP 503").unwrap();
    m.end_incident(id, e).unwrap();
    let mut t = from;
    while t < to {
        let ok = !(s..e).contains(&t);
        let p = openlens_meta::Probe {
            ok,
            status: Some(if ok { 200 } else { 503 }),
            ms: Some(90),
            error: None,
        };
        m.record_check_in("default", "production", t, &p).unwrap();
        t += 60;
    }

    let (st, tok) = call(
        &h,
        "POST",
        "/api/tokens",
        "adm_test",
        serde_json::json!({"scope": "read", "projects": ["default"]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let read = tok["token"].as_str().unwrap().to_owned();
    let target = "/api/projects/default/uptime/target";
    let body = serde_json::json!({"target": 0.999});
    let (st, _) = call(&h, "PUT", target, &read, body.clone()).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a read token cannot set a target"
    );
    let (st, v) = call(&h, "PUT", target, "adm_test", body).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, _) = call(
        &h,
        "PUT",
        target,
        "adm_test",
        serde_json::json!({"target": 99.9}),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a percentage is refused"
    );
    let audit = m.audit_log(50).unwrap();
    assert!(
        audit.iter().any(|a| a.3 == "uptime.set_target"),
        "the change is audited"
    );

    let (st, report) = call(
        &h,
        "GET",
        "/api/projects/default/uptime/report",
        &read,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{report}");
    let (_, status) = call(
        &h,
        "GET",
        "/api/projects/default/status",
        "adm_test",
        serde_json::Value::Null,
    )
    .await;
    let windows = report["windows"].as_array().unwrap();
    let day = &windows[0];
    let a = day["availability"].as_f64().unwrap();
    assert!(a < 1.0 && a > 0.8, "{day}");
    for (i, w) in windows.iter().enumerate() {
        let s = &status["availability"]["windows"][i];
        assert_eq!(s["label"], w["label"]);
        assert_eq!(s["availability"], w["availability"]);
        assert_eq!(s["incidents"], w["incidents"]);
    }
    assert_eq!(status["availability"]["slo"]["target"], 0.999);
    assert_eq!(
        status["availability"]["slo"]["spent_seconds"],
        report["slo"]["spent_seconds"]
    );

    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/uptime", Some(&owner), false).await).await;
    assert!(
        page.contains(&format!("{:.3}", a * 100.0)),
        "the page shows the same figure"
    );
    assert!(page.contains("Error budget"));
}

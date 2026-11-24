//! Dashboard sign-in and login links. Moved out of main.rs as is (#307).

use super::*;

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
    // Link previews GET the link: that must not use it (#315).
    for _ in 0..2 {
        let page = get(&h, path, None, false).await;
        assert_eq!(page.status(), StatusCode::OK);
        assert!(page.headers().get("set-cookie").is_none());
    }
    let first = use_link(&h, path).await;
    assert!(first.status().is_redirection());
    assert!(first.headers().get("set-cookie").is_some());
    let second = use_link(&h, path).await;
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
}

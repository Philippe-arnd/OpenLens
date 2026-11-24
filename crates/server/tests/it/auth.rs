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

/// SEC-06: a public URL derived from request headers trusted a header any
/// client could send. `X-Forwarded-Host`/`-Proto` must only count from a
/// trusted proxy, and the public scheme must drive HSTS; `/api` responses
/// carry `nosniff` and `no-store`, since they are session-authenticated
/// JSON.
#[tokio::test]
async fn login_links_ignore_an_untrusted_forwarded_host() {
    let h = harness();

    async fn login_link(
        peer: &str,
        forwarded_host: &str,
        forwarded_proto: &str,
    ) -> (String, bool, bool, bool) {
        let h = harness();
        let mut req = Request::post("/api/login-links")
            .header("authorization", "Bearer adm_test")
            .header("host", "legit.example")
            .header("x-forwarded-host", forwarded_host)
            .header("x-forwarded-proto", forwarded_proto)
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(
                peer.parse().unwrap(),
            ));
        let resp = router(h.state.clone()).oneshot(req).await.unwrap();
        let hsts = resp.headers().get("strict-transport-security").is_some();
        let nosniff = resp
            .headers()
            .get("x-content-type-options")
            .is_some_and(|v| v == "nosniff");
        let no_store = resp
            .headers()
            .get("cache-control")
            .is_some_and(|v| v == "no-store");
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        (
            body["url"].as_str().unwrap().to_owned(),
            hsts,
            nosniff,
            no_store,
        )
    }

    // A direct, public client cannot pick the host its own links use, and
    // its (forged) claim of HTTPS does not turn on HSTS.
    let (url, hsts, nosniff, no_store) =
        login_link("203.0.113.5:1234", "evil.example", "https").await;
    assert!(
        url.contains("legit.example") && !url.contains("evil.example"),
        "{url}"
    );
    assert!(!hsts, "an untrusted claim of HTTPS must not set HSTS");
    assert!(nosniff && no_store, "{url}");

    // Behind a trusted proxy (loopback/private, as the rest of the server
    // already treats X-Forwarded-For), the headers it sets are honoured.
    let (url, hsts, ..) = login_link("127.0.0.1:1234", "proxy.example", "https").await;
    assert!(url.contains("proxy.example"), "{url}");
    assert!(hsts, "a trusted proxy's https must set HSTS");

    // The session cookie itself follows the same public scheme (SEC-06):
    // `__Host-` over HTTPS, plain otherwise.
    let secure = router(h.state.clone())
        .oneshot({
            let mut req = Request::post("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-forwarded-proto", "https")
                .body(Body::from("token=adm_test"))
                .unwrap();
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(
                    "127.0.0.1:1234".parse().unwrap(),
                ));
            req
        })
        .await
        .unwrap();
    let cookie = secure.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cookie.starts_with("__Host-ol_session=") && cookie.contains("; Secure"),
        "{cookie}"
    );
}

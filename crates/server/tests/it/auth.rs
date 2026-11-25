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

/// A form POST that also returns the session cookie it sets, if any.
async fn post_cookie(
    h: &Harness,
    uri: &str,
    cookie: &str,
    body: &str,
) -> (StatusCode, Option<String>) {
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
    let set = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ol_session="))
        .map(|v| v.split(';').next().unwrap().to_owned());
    (r.status(), set)
}

#[tokio::test]
async fn the_token_holder_creates_an_account_then_signs_in_with_a_password() {
    let h = harness();
    let token = login_cookie(&h).await;
    // Weak or mistyped: refused.
    let (s, page) = post_form_body(
        &h,
        "/me/password",
        &token,
        "email=ann%40example.com&password=short&again=short",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(page.contains("at least 12"), "{page}");
    // The owner becomes a real admin account.
    let (s, first) = post_cookie(
        &h,
        "/me/password",
        &token,
        "email=ann%40example.com&password=violet+tram+seven&again=violet+tram+seven",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let first = first.expect("signed in as the account");
    assert!(first.contains(".ann-at-example-com."), "{first}");
    // The login page now leads with the password, the token folded below.
    let page = body_text(get(&h, "/login", None, false).await).await;
    assert!(page.contains("action=\"/login/password\"") && page.contains("Use the admin token"));
    // Wrong password and unknown email: the same answer.
    let (s1, _) = post_cookie(
        &h,
        "/login/password",
        "",
        "email=ann%40example.com&password=nope+nope+nope",
    )
    .await;
    let (s2, _) = post_cookie(
        &h,
        "/login/password",
        "",
        "email=bob%40example.com&password=nope+nope+nope",
    )
    .await;
    assert_eq!(
        (s1, s2),
        (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED)
    );
    // The right one.
    let (s, session) = post_cookie(
        &h,
        "/login/password",
        "",
        "email=ann%40example.com&password=violet+tram+seven",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let session = session.unwrap();
    assert_eq!(
        get(&h, "/me", Some(&session), false).await.status(),
        StatusCode::OK
    );
    // Changing it needs the current one, and signs out the older sessions.
    let (s, _) = post_form_body(
        &h,
        "/me/password",
        &session,
        "current=wrong&password=amber+kite+river&again=amber+kite+river",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (s, fresh) = post_cookie(
        &h,
        "/me/password",
        &session,
        "current=violet+tram+seven&password=amber+kite+river&again=amber+kite+river",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let fresh = fresh.unwrap();
    assert_ne!(
        get(&h, "/me", Some(&first), false).await.status(),
        StatusCode::OK,
        "older session out"
    );
    assert_eq!(
        get(&h, "/me", Some(&fresh), false).await.status(),
        StatusCode::OK
    );
    let (s, _) = post_cookie(
        &h,
        "/login/password",
        "",
        "email=ann%40example.com&password=violet+tram+seven",
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "old password");
    // The token always works: the way back in.
    assert!(login_cookie(&h).await.starts_with("ol_session="));
}

#[tokio::test]
async fn an_admin_resets_a_password_with_a_one_time_link() {
    let h = harness();
    let token = login_cookie(&h).await;
    // An invited viewer signs in with their link and sets a password.
    let vic = h
        .state
        .meta
        .upsert_user("vic@example.com", "viewer", None)
        .unwrap();
    let t = h
        .state
        .meta
        .create_login_link_for(Some(&vic.id), 3600)
        .unwrap();
    let (_, link_session) = post_cookie(&h, &format!("/l/{t}"), "", "").await;
    let link_session = link_session.expect("signed in by the link");
    let (s, _) = post_cookie(
        &h,
        "/me/password",
        &link_session,
        "password=violet+tram+seven&again=violet+tram+seven",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER, "a viewer sets a password");
    let (s, session) = post_cookie(
        &h,
        "/login/password",
        "",
        "email=vic%40example.com&password=violet+tram+seven",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let session = session.unwrap();
    // The token holder can't take over that email as their own account.
    let (s, page) = post_form_body(
        &h,
        "/me/password",
        &token,
        "email=vic%40example.com&password=amber+kite+river&again=amber+kite+river",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(page.contains("already belongs"), "{page}");
    assert_eq!(h.state.meta.user(&vic.id).unwrap().unwrap().role, "viewer");
    // The admin resets it: a one-time link, the old password and sessions out.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (s, page) =
        post_form_body(&h, &format!("/users/{}/reset-password", vic.id), &token, "").await;
    assert_eq!(s, StatusCode::OK, "{page}");
    let link = page
        .split("/l/")
        .nth(1)
        .and_then(|r| r.split('<').next())
        .expect("a link on the page")
        .to_owned();
    assert!(!page.contains("violet tram"), "no password shown");
    assert_ne!(
        get(&h, "/me", Some(&session), false).await.status(),
        StatusCode::OK,
        "sessions end"
    );
    let (s, _) = post_cookie(
        &h,
        "/login/password",
        "",
        "email=vic%40example.com&password=violet+tram+seven",
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "old password gone");
    let (_, fresh) = post_cookie(&h, &format!("/l/{link}"), "", "").await;
    let fresh = fresh.expect("the reset link signs in once");
    let (s, _) = post_cookie(
        &h,
        "/me/password",
        &fresh,
        "password=amber+kite+river&again=amber+kite+river",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER, "a new one, no current needed");
    let (s, _) = post_cookie(&h, &format!("/l/{link}"), "", "").await;
    assert_ne!(s, StatusCode::SEE_OTHER, "the link works once");
}

#[tokio::test]
async fn wrong_current_passwords_are_slowed_down_and_audited() {
    let h = harness();
    let u = h
        .state
        .meta
        .upsert_user("cur@example.com", "viewer", None)
        .unwrap();
    let t = h
        .state
        .meta
        .create_login_link_for(Some(&u.id), 3600)
        .unwrap();
    let (_, s0) = post_cookie(&h, &format!("/l/{t}"), "", "").await;
    let s0 = s0.unwrap();
    let (s, _) = post_cookie(
        &h,
        "/me/password",
        &s0,
        "password=violet+tram+seven&again=violet+tram+seven",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let (_, s1) = post_cookie(
        &h,
        "/login/password",
        "",
        "email=cur%40example.com&password=violet+tram+seven",
    )
    .await;
    let s1 = s1.unwrap();
    let start = std::time::Instant::now();
    for _ in 0..4 {
        let (s, _) = post_form_body(
            &h,
            "/me/password",
            &s1,
            "current=nope&password=amber+kite+river&again=amber+kite+river",
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
    assert!(
        start.elapsed() >= std::time::Duration::from_secs(1),
        "the 4th try waited"
    );
    let audit = h.state.meta.audit_log(50).unwrap();
    let n = audit
        .iter()
        .filter(|e| e.3 == "auth.password_failed")
        .count();
    assert!(n >= 4, "{n} failures audited");
}

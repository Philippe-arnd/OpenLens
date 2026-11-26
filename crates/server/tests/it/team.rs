//! Viewers, owners, tokens, operations and project settings. Moved out of main.rs as is (#307).

use super::*;

#[tokio::test]
async fn viewers_see_their_projects_only_and_change_nothing() {
    let h = harness();
    h.state.meta.create_project("Other", None).unwrap();
    let viewer = invite(
        &h,
        serde_json::json!({"email": "Ana@Example.com", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    assert_eq!(
        get(&h, "/p/default", Some(&viewer), false).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&h, "/p/other", Some(&viewer), false).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&h, "/p/other/events", Some(&viewer), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags/beta",
            &viewer,
            "enabled=on&rollout=50"
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&h, "/users", Some(&viewer), false).await.status(),
        StatusCode::FORBIDDEN
    );
    // One visible project: home goes straight to it.
    assert_eq!(
        get(&h, "/", Some(&viewer), false).await.headers()["location"],
        "/p/default"
    );

    let admin = invite(
        &h,
        serde_json::json!({"email": "boss@example.com", "role": "admin"}),
    )
    .await;
    let page = get(&h, "/users", Some(&admin), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("ana@example.com") && html.contains("boss@example.com"));
    assert!(!html.contains(" style="));
    assert_eq!(
        get(&h, "/p/other", Some(&admin), false).await.status(),
        StatusCode::OK
    );

    // Removing a user ends their session at once.
    let id = h
        .state
        .meta
        .users()
        .unwrap()
        .into_iter()
        .find(|u| u.email == "ana@example.com")
        .unwrap()
        .id;
    let del = router(h.state.clone())
        .oneshot(
            Request::delete(format!("/api/users/{id}"))
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    assert_eq!(
        get(&h, "/p/default", Some(&viewer), false).await.status(),
        StatusCode::SEE_OTHER
    );
    // Bad input is refused.
    let bad = router(h.state.clone())
        .oneshot(
            Request::post("/api/login-links")
                .header("authorization", "Bearer adm_test")
                .body(Body::from(r#"{"email":"x@y.z","role":"god"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn micro_surveys_are_targeted_served_with_flags_and_summarised() {
    let h = harness();
    let put = |body: serde_json::Value| {
        let st = h.state.clone();
        async move {
            router(st)
                .oneshot(
                    Request::put("/api/projects/default/surveys/useful")
                        .header("authorization", "Bearer adm_test")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };
    let good = serde_json::json!({"key":"useful","question":"How useful was that?","kind":"rating","trigger":"entry_saved","enabled":true,"sample":100});
    assert_eq!(put(serde_json::json!({"key":"useful","question":"?","kind":"essay","trigger":"x","enabled":true,"sample":100})).await, StatusCode::BAD_REQUEST);
    assert_eq!(put(good.clone()).await, StatusCode::OK);
    let flags = |id: &'static str| {
        let (st, key) = (h.state.clone(), h.key.clone());
        async move {
            let r = router(st)
                .oneshot(
                    Request::get(format!("/flags?key={key}&distinct_id={id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            serde_json::from_slice::<serde_json::Value>(
                &axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap(),
            )
            .unwrap()
        }
    };
    let v = flags("u1").await;
    assert_eq!(v["surveys"][0]["key"], "useful");
    assert_eq!(v["surveys"][0]["trigger"], "entry_saved");
    assert!(
        v["surveys"][0].get("sample").is_none(),
        "targeting stays server-side"
    );
    let mut off = good.clone();
    off["enabled"] = false.into();
    assert_eq!(put(off).await, StatusCode::OK);
    assert_eq!(flags("u1").await["surveys"].as_array().unwrap().len(), 0);

    let batch = serde_json::json!({"key": h.key, "batch": [
        {"event":"$identify","distinct_id":"a","properties":{"$role":"owner"}},
        {"event":"$survey","distinct_id":"a","properties":{"$survey":"useful","$answer":5}},
        {"event":"$survey","distinct_id":"b","properties":{"$survey":"useful","$answer":3}},
        {"event":"$survey","distinct_id":"c","properties":{"$survey":"useful","$dismissed":true}},
        {"event":"$survey","distinct_id":"d","properties":{"$survey":"why","$answer":"too slow on mobile"}}
    ]});
    assert_eq!(
        post(&h, "/capture", batch.to_string().into_bytes(), false)
            .await
            .0,
        StatusCode::OK
    );
    openlens_server::flush_now(&h.state).unwrap();
    let r = get(&h, "/api/projects/default/surveys", None, true).await;
    let v: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
            .unwrap();
    let res = v["results"].as_array().unwrap();
    let useful = res.iter().find(|r| r["key"] == "useful").unwrap();
    assert_eq!(useful["mean"], 4.0);
    assert_eq!(useful["dismissed"], 1);
    let why = res.iter().find(|r| r["key"] == "why").unwrap();
    assert_eq!(why["latest_text"][0], "too slow on mobile");
    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default/surveys", Some(&cookie), false).await;
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("How useful was that?") && !html.contains(" style="));
    let js = get(&h, "/s.js", None, false).await;
    assert_eq!(js.status(), StatusCode::OK);
}

/// First setup: the owner lands on the welcome page, creates their admin
/// account and continues as that user; once someone has an account, the
/// token goes straight to the board.
#[tokio::test]
async fn first_owner_login_creates_the_admin_account() {
    let h = harness();
    let login = || async {
        router(h.state.clone())
            .oneshot(
                Request::post("/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("token=adm_test"))
                    .unwrap(),
            )
            .await
            .unwrap()
    };
    let first = login().await;
    assert_eq!(first.headers()["location"], "/welcome");
    let owner = first.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        get(&h, "/welcome", Some(&owner), false).await.status(),
        StatusCode::OK
    );
    assert!(
        get(&h, "/welcome", None, false)
            .await
            .status()
            .is_redirection()
    );

    let created = router(h.state.clone())
        .oneshot(
            Request::post("/welcome")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=Phil%40Example.com"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.headers()["location"], "/");
    let user = created.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_ne!(user, owner, "the session now belongs to the new account");
    let users = h.state.meta.users().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(
        (users[0].email.as_str(), users[0].role.as_str()),
        ("phil@example.com", "admin")
    );
    assert_eq!(
        get(&h, "/users", Some(&user), false).await.status(),
        StatusCode::OK
    );
    assert!(
        get(&h, "/welcome", Some(&user), false)
            .await
            .status()
            .is_redirection(),
        "a user already has an account"
    );

    assert_eq!(
        login().await.headers()["location"],
        "/",
        "set up: straight to the board"
    );
}

/// Without SMTP and a public URL there is no email sign-in, and asking for a
/// link says the same thing whatever the address.
#[tokio::test]
async fn email_login_is_off_without_smtp_and_never_tells_who_exists() {
    let h = harness();
    let page = get(&h, "/login", None, false).await;
    let body = axum::body::to_bytes(page.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("Email me a login link"));
    for email in ["nobody@example.com", "x"] {
        let r = router(h.state.clone())
            .oneshot(
                Request::post("/login/email")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("email={email}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Check your inbox"));
    }
}

async fn api_as(
    h: &Harness,
    method: &str,
    uri: &str,
    token: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let r = router(h.state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or_default())
}

#[tokio::test]
async fn scoped_tokens_reach_only_their_scope_and_projects() {
    let h = harness();
    let (s, v) = api_as(
        &h,
        "POST",
        "/api/tokens",
        "adm_test",
        r#"{"scope":"read","label":"mcp"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let read = v["token"].as_str().unwrap().to_owned();
    let (_, v) = api_as(
        &h,
        "POST",
        "/api/tokens",
        "adm_test",
        r#"{"scope":"write","projects":["default"]}"#,
    )
    .await;
    let write = v["token"].as_str().unwrap().to_owned();
    let (_, v) = api_as(
        &h,
        "POST",
        "/api/tokens",
        "adm_test",
        r#"{"scope":"write","projects":["elsewhere"]}"#,
    )
    .await;
    let other = v["token"].as_str().unwrap().to_owned();
    let flag = r#"{"key":"beta","enabled":false,"rollout":0}"#;

    assert_eq!(
        api_as(&h, "GET", "/api/projects/default/flags", &read, "")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        api_as(&h, "PUT", "/api/projects/default/flags/beta", &read, flag)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api_as(&h, "GET", "/api/users", &read, "").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api_as(&h, "GET", "/api/tokens", &write, "").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api_as(&h, "PUT", "/api/projects/default/flags/beta", &write, flag)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        api_as(&h, "GET", "/api/projects/default/flags", &other, "")
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Not a key to the dashboard.
    let page = router(h.state.clone())
        .oneshot(
            Request::get("/p/default")
                .header("authorization", format!("Bearer {read}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(page.status().is_redirection());
    // The internal marker cannot be sent from outside.
    let forged = router(h.state.clone())
        .oneshot(
            Request::get("/api/projects/default/flags")
                .header("x-openlens-scope-checked", "anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
    // Listed without their values, then revoked.
    let (_, list) = api_as(&h, "GET", "/api/tokens", "adm_test", "").await;
    assert!(!list.to_string().contains(&read));
    let id = list["tokens"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        api_as(&h, "DELETE", &format!("/api/tokens/{id}"), "adm_test", "")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        api_as(&h, "GET", "/api/projects/default/flags", &read, "")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn operators_get_metrics_readiness_and_request_ids() {
    let h = harness();
    let ready = get(&h, "/readyz", None, false).await;
    assert_eq!(ready.status(), StatusCode::OK);
    assert!(ready.headers().contains_key("x-request-id"));
    assert_eq!(
        get(&h, "/metrics", None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let count = |text: &str| -> u64 {
        text.lines()
            .find_map(|l| l.strip_prefix("openlens_events_accepted_total "))
            .unwrap()
            .parse()
            .unwrap()
    };
    let before = count(&body_text(get(&h, "/metrics", None, true).await).await);
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    let after = body_text(get(&h, "/metrics", None, true).await).await;
    assert!(count(&after) > before, "{after}");
    assert!(
        after.contains("openlens_buffered_events") && after.contains("openlens_disk_free_bytes")
    );
    // An id sent by the proxy comes back as is.
    let r = router(h.state.clone())
        .oneshot(
            Request::get("/healthz")
                .header("x-request-id", "req-42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.headers()["x-request-id"], "req-42");
}

/// Through a proxy, one client address gets a burst, then 429 with
/// Retry-After (which the SDK honours by retrying later).
#[tokio::test]
async fn capture_is_rate_limited_per_client_behind_a_proxy() {
    let h = harness();
    let mut limited = None;
    for i in 0..5000 {
        let r = router(h.state.clone())
            .oneshot(
                Request::post("/capture")
                    .header("content-type", "text/plain")
                    .header("x-forwarded-for", "203.0.113.9")
                    // The limit applies before the body is read: a body the
                    // server rejects at once keeps each request fast, so the
                    // bucket (50/s) cannot refill faster than the test sends
                    // on a slow disk.
                    .body(Body::from("not a batch"))
                    .unwrap(),
            )
            .await
            .unwrap();
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(r.headers()["retry-after"], "5");
            limited = Some(i);
            break;
        }
    }
    let i = limited.expect("a client sending batches non-stop is limited");
    assert!(
        i >= 400,
        "the burst lets a normal client through first ({i})"
    );
    // Another client is not affected.
    let other = router(h.state.clone())
        .oneshot(
            Request::post("/capture")
                .header("content-type", "text/plain")
                .header("x-forwarded-for", "198.51.100.7")
                .body(Body::from(batch(&h.key)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::OK);
}

/// Deleting a project needs its id twice, then removes its metadata and
/// files; its key stops working.
#[tokio::test]
async fn deleting_a_project_needs_confirmation_and_removes_everything() {
    let h = harness();
    post(&h, "/capture", batch(&h.key), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    let del = |q: &'static str| {
        router(h.state.clone()).oneshot(
            Request::delete(format!("/api/projects/default{q}"))
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
                .unwrap(),
        )
    };
    assert_eq!(del("").await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        del("?confirm=other").await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert!(h.state.data_dir.join("events/project=default").exists());
    assert_eq!(
        del("?confirm=default").await.unwrap().status(),
        StatusCode::OK
    );
    assert!(h.state.meta.projects().unwrap().is_empty());
    assert!(!h.state.data_dir.join("events/project=default").exists());
    let after = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(after.0, StatusCode::UNAUTHORIZED, "the key is gone");
    assert_eq!(
        del("?confirm=default").await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

/// A project's time zone (D-017): set at creation or later, IANA names only.
#[tokio::test]
async fn project_time_zone_is_set_and_checked() {
    let h = harness();
    let send = |method: &'static str, path: &'static str, body: &'static str| {
        router(h.state.clone()).oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
    };
    let created = send(
        "POST",
        "/api/projects",
        r#"{"name":"Paris app","timezone":"Europe/Paris"}"#,
    )
    .await
    .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let p = h.state.meta.project("paris-app").unwrap().unwrap();
    assert_eq!(p.timezone, "Europe/Paris");
    assert_eq!(
        h.state.meta.project("default").unwrap().unwrap().timezone,
        "UTC"
    );
    let bad = send(
        "POST",
        "/api/projects",
        r#"{"name":"Bad","timezone":"Mars/Olympus"}"#,
    )
    .await
    .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let ok = send(
        "PATCH",
        "/api/projects/default",
        r#"{"timezone":"America/New_York"}"#,
    )
    .await
    .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(
        h.state.meta.project("default").unwrap().unwrap().timezone,
        "America/New_York"
    );
    let bad = send("PATCH", "/api/projects/default", r#"{"timezone":"x'; --"}"#)
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let missing = send("PATCH", "/api/projects/nope", r#"{"timezone":"UTC"}"#)
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    // The board still loads with a zone set.
    let board = router(h.state.clone())
        .oneshot(
            Request::get("/api/projects/default/board")
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(board.status(), StatusCode::OK);
}

#[tokio::test]
async fn project_settings_do_what_the_cli_does() {
    let h = harness();
    let owner = login_cookie(&h).await;
    // The key and the wiring check live on the Install tab (#438).
    let page = get(&h, "/p/default/settings/install", Some(&owner), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let key = h.state.meta.project("default").unwrap().unwrap().api_key;
    assert!(html.contains(&key) && html.contains("Wiring check"));
    assert!(!html.contains(" style="));

    assert_eq!(
        post_form(
            &h,
            "/p/default/settings",
            &owner,
            "name=Journal&app_url=https%3A%2F%2Fjournal.example%2Fapp&timezone=Europe%2FParis"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let p = h.state.meta.project("default").unwrap().unwrap();
    assert_eq!(
        (p.name.as_str(), p.timezone.as_str()),
        ("Journal", "Europe/Paris")
    );
    assert_eq!(
        h.state
            .meta
            .project_setting("default", "app_url")
            .unwrap()
            .as_deref(),
        Some("https://journal.example")
    );
    assert_eq!(
        post_form(&h, "/p/default/settings", &owner, "name=J&timezone=Nowhere").await,
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &owner, "").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &owner, "confirm=yes").await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.project_by_key(&key).unwrap().is_none());

    let icon = get(&h, "/p/default/icon", Some(&owner), false).await;
    assert_eq!(icon.headers()["content-type"], "image/svg+xml");

    assert_eq!(
        post_form(&h, "/projects", &owner, "name=Kanban&timezone=").await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.project("kanban").unwrap().is_some());
    assert_eq!(
        post_form(
            &h,
            "/p/kanban/settings/delete",
            &owner,
            "confirm=kanban+app"
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(&h, "/p/kanban/settings/delete", &owner, "confirm=Kanban").await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.project("kanban").unwrap().is_none());

    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &viewer, "confirm=yes").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(&h, "/projects", &viewer, "name=Nope").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &h,
            "/p/default/export?date=2026-01-01",
            Some(&viewer),
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn team_crud_and_the_viewer_time_zone() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let ana = invite(
        &h,
        serde_json::json!({"email": "ana@example.com", "role": "admin"}),
    )
    .await;
    let id = h
        .state
        .meta
        .user_by_email("ana@example.com")
        .unwrap()
        .unwrap()
        .id;
    // Ana is the only admin user: she can't be removed or disabled.
    assert_eq!(
        post_form(&h, &format!("/users/{id}/disable"), &owner, "").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            &format!("/users/{id}/delete"),
            &owner,
            "confirm=ana%40example.com"
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            &format!("/users/{id}"),
            &owner,
            "name=Ana&email=ana%40example.com&role=admin&projects="
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        h.state.meta.user(&id).unwrap().unwrap().name.as_deref(),
        Some("Ana")
    );
    let page = get(&h, "/users?q=ana", Some(&owner), false).await;
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("ana@example.com") && html.contains("active"));

    // Her own time zone drives what she reads.
    assert_eq!(
        post_form(&h, "/me", &ana, "name=Ana&timezone=Asia%2FTokyo").await,
        StatusCode::SEE_OTHER
    );
    let ev = get(&h, "/p/default/events", Some(&ana), false).await;
    let html = String::from_utf8(
        axum::body::to_bytes(ev.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Times in Asia/Tokyo"), "viewer zone shown");
    assert_eq!(
        post_form(&h, "/me", &ana, "timezone=Mars").await,
        StatusCode::BAD_REQUEST
    );

    // A second admin lets the first be disabled: her session ends at once.
    invite(
        &h,
        serde_json::json!({"email": "bo@example.com", "role": "admin"}),
    )
    .await;
    assert_eq!(
        post_form(&h, &format!("/users/{id}/disable"), &owner, "").await,
        StatusCode::SEE_OTHER
    );
    assert_ne!(
        get(&h, "/p/default", Some(&ana), false).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        post_form(&h, &format!("/users/{id}/delete"), &owner, "confirm=nope").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            &format!("/users/{id}/delete"),
            &owner,
            "confirm=ana%40example.com"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.user(&id).unwrap().is_none());
}

#[tokio::test]
async fn viewers_comment_on_replays_and_delete_only_their_own() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let ana = invite(
        &h,
        serde_json::json!({"email": "ana@example.com", "role": "viewer"}),
    )
    .await;
    let bo = invite(
        &h,
        serde_json::json!({"email": "bo@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(
            &h,
            "/p/default/replays/s1/comments",
            &ana,
            "at=12400&body=Form+error+here"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let c = h.state.meta.replay_comments("default", "s1").unwrap();
    assert_eq!(
        (c.len(), c[0].at_ms, c[0].author.as_str()),
        (1, 12_400, "ana@example.com")
    );
    let path = format!("/p/default/replays/s1/comments/{}/delete", c[0].id);
    assert_eq!(post_form(&h, &path, &bo, "").await, StatusCode::FORBIDDEN);
    // Viewers still can't change anything else under the project.
    assert_eq!(
        post_form(&h, "/p/default/settings/rotate-key", &ana, "confirm=yes").await,
        StatusCode::FORBIDDEN
    );
    let api = get(&h, "/api/projects/default/replays/s1/comments", None, true).await;
    assert_eq!(api.status(), StatusCode::OK);
    assert_eq!(
        post_form(&h, &path, &owner, "").await,
        StatusCode::SEE_OTHER
    );
    assert!(
        h.state
            .meta
            .replay_comments("default", "s1")
            .unwrap()
            .is_empty()
    );
}

/// A token or an admin limited to some projects cannot reach the rest of
/// the install (review of v0.3.0): no other project's key, no install-wide
/// call, no install-wide page.
#[tokio::test]
async fn project_limited_tokens_and_admins_stay_in_their_projects() {
    let h = harness();
    let (_, v) = api_as(
        &h,
        "POST",
        "/api/projects",
        "adm_test",
        r#"{"name":"Other"}"#,
    )
    .await;
    let other = v["id"].as_str().unwrap().to_owned();
    let (_, v) = api_as(
        &h,
        "POST",
        "/api/tokens",
        "adm_test",
        r#"{"scope":"admin","projects":["default"]}"#,
    )
    .await;
    let limited = v["token"].as_str().unwrap().to_owned();
    let (s, v) = api_as(&h, "GET", "/api/projects", &limited, "").await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(ids, ["default"], "{v}");
    for (m, p, body) in [
        ("GET", "/api/users", ""),
        ("POST", "/api/tokens", r#"{"scope":"admin"}"#),
        ("POST", "/api/projects", r#"{"name":"x"}"#),
    ] {
        assert_eq!(
            api_as(&h, m, p, &limited, body).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p}"
        );
    }
    assert_eq!(
        api_as(
            &h,
            "GET",
            &format!("/api/projects/{other}/board"),
            &limited,
            ""
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // An admin of one project gets 403 on install-wide pages.
    let admin_a = invite(
        &h,
        serde_json::json!({"email": "a@example.com", "role": "admin", "projects": ["default"]}),
    )
    .await;
    for uri in ["/settings/tokens", "/settings/storage", "/users"] {
        assert_eq!(
            get(&h, uri, Some(&admin_a), false).await.status(),
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
}

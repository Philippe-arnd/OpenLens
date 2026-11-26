//! Status announcements (#562): who may post them, the order of their
//! updates in the API, and posted markup shown as text on the Uptime page.

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

async fn token(h: &Harness, scope: &str) -> String {
    let (s, v) = call(
        h,
        "POST",
        "/api/tokens",
        "adm_test",
        serde_json::json!({"scope": scope, "projects": ["default"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    v["token"].as_str().unwrap().to_owned()
}

const LIST: &str = "/api/projects/default/announcements";

#[tokio::test]
async fn a_write_token_posts_updates_in_order_and_readers_only_read() {
    let h = harness();
    let read = token(&h, "read").await;
    let write = token(&h, "write").await;
    let post = serde_json::json!({"title": "Exports are slow", "level": "degraded",
        "body": "<script>alert(1)</script> details: https://example.com/status."});
    assert_eq!(
        call(&h, "POST", LIST, &read, post.clone()).await.0,
        StatusCode::FORBIDDEN,
        "a read token cannot post"
    );
    let (s, a) = call(&h, "POST", LIST, &write, post).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let id = a["id"].as_i64().unwrap();
    for state in ["investigating", "identified", "resolved"] {
        let uri = format!("{LIST}/{id}/updates");
        let body = serde_json::json!({"state": state, "body": format!("now {state}")});
        assert_eq!(call(&h, "POST", &uri, &write, body).await.0, StatusCode::OK);
    }
    let (s, l) = call(&h, "GET", LIST, &read, serde_json::Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    let got = l["announcements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id)
        .unwrap();
    let states: Vec<&str> = got["updates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["investigating", "identified", "resolved"]);
    assert_eq!(got["level"], "resolved", "a resolved update resolves it");
    assert!(got["ends_at"].is_i64());
    // An unknown announcement is a 404, not a 500.
    let (s, _) = call(
        &h,
        "POST",
        &format!("{LIST}/999999/updates"),
        &write,
        serde_json::json!({"state": "identified", "body": "x"}),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let audited = h.state.meta.audit_log(50).unwrap();
    assert!(
        audited
            .iter()
            .filter(|e| e.3.starts_with("announcement."))
            .count()
            >= 4
    );

    // A viewer reads them on Uptime, as text, and cannot post.
    let viewer = invite(
        &h,
        serde_json::json!({"email": "[EMAIL_REDACTED]", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    let page = body_text(get(&h, "/p/default/uptime", Some(&viewer), false).await).await;
    assert!(page.contains("Exports are slow"));
    assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!page.contains("<script>alert(1)"));
    assert!(page.contains("href=\"https://example.com/status\""));
    assert_eq!(
        post_form(
            &h,
            "/p/default/uptime/announcements",
            &viewer,
            "title=Mine&level=information"
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn posted_markup_never_reaches_the_page_as_markup() {
    let h = harness();
    let mut accepted = 0;
    for t in openlens_testkit::hostile_texts() {
        let title: String = t.chars().take(100).collect();
        let body = serde_json::json!({"title": format!("T {title}"), "body": t});
        let (s, _) = call(&h, "POST", LIST, "adm_test", body).await;
        assert!(
            s == StatusCode::CREATED || s == StatusCode::UNPROCESSABLE_ENTITY,
            "{t:?}: {s}"
        );
        accepted += usize::from(s == StatusCode::CREATED);
    }
    assert!(accepted > 3, "plain markup is accepted as text");
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/uptime", Some(&owner), false).await).await;
    for raw in [
        "<script>alert",
        "<img src=x",
        "<svg onload",
        "href=\"javascript:",
    ] {
        assert!(!page.contains(raw), "{raw} reached the page");
    }
    // The owner writes from the page: the form posts the command.
    let s = post_form(
        &h,
        "/p/default/uptime/announcements",
        &owner,
        "title=Deploying+v2.3&level=information&components=production&pinned=on",
    )
    .await;
    assert!(s.is_redirection(), "{s}");
    let (_, l) = call(&h, "GET", LIST, "adm_test", serde_json::Value::Null).await;
    assert!(l["pinned"].is_i64(), "the pinned notice is listed: {l}");
}

#[tokio::test]
async fn an_open_incident_is_offered_an_update_and_a_closed_one_a_resolution() {
    let h = harness();
    let now = chrono::Utc::now().timestamp();
    let inc = h
        .state
        .meta
        .start_incident("default", "down", now - 120, "HTTP 502")
        .unwrap();
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/uptime", Some(&owner), false).await).await;
    assert!(page.contains(&format!("/uptime/announcements/new?incident={inc}")));
    let (s, a) = call(
        &h,
        "POST",
        LIST,
        "adm_test",
        serde_json::json!({"title": "Down", "level": "outage", "incident": inc, "state": "investigating"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    h.state.meta.end_incident(inc, now).unwrap();
    let page = body_text(get(&h, "/p/default/uptime", Some(&owner), false).await).await;
    let close = format!("/uptime/announcements/{}/close", a["id"]);
    assert!(page.contains(&close), "Mark resolved is proposed");
    // The probe closing the incident did not close the announcement.
    let (_, l) = call(&h, "GET", LIST, "adm_test", serde_json::Value::Null).await;
    assert_eq!(l["announcements"][0]["level"], "outage");
    assert_eq!(l["offers"][0]["kind"], "mark_resolved");
}

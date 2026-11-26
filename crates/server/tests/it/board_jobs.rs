//! Long boards as background jobs (#515, D-063): start, progress, done,
//! cancel, privacy and limits, on a tiny dataset.

use super::*;

async fn send(
    h: &Harness,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: &str,
) -> axum::response::Response {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/x-www-form-urlencoded");
    match cookie {
        Some(c) => req = req.header("cookie", c),
        None => req = req.header("authorization", "Bearer adm_test"),
    }
    router(h.state.clone())
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

fn location(r: &axum::response::Response) -> String {
    r.headers()["location"].to_str().unwrap().to_owned()
}

/// Load a job page until it stops refreshing itself (20 s at most).
async fn settled(h: &Harness, page: &str, cookie: &str) -> String {
    for _ in 0..400 {
        let r = get(h, page, Some(cookie), false).await;
        assert_eq!(r.status(), StatusCode::OK);
        if !r.headers().contains_key("refresh") {
            return body_text(r).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the job never finished");
}

async fn captured(h: &Harness) {
    let (status, _) = post(h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
}

#[tokio::test]
async fn a_board_job_runs_in_the_background_and_shows_its_board() {
    let h = harness();
    captured(&h).await;
    let from = (chrono::Utc::now() - chrono::Duration::days(2)).format("%Y-%m-%d");
    // The API: 202, then the job until it is done, with the board.
    let r = send(
        &h,
        "POST",
        &format!("/api/projects/default/board/jobs?from={from}"),
        None,
        "",
    )
    .await;
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let url = v["url"].as_str().unwrap().to_owned();
    let mut job = serde_json::Value::Null;
    for _ in 0..400 {
        let r = get(&h, &url, None, true).await;
        assert_eq!(r.status(), StatusCode::OK);
        job = serde_json::from_str(&body_text(r).await).unwrap();
        if !matches!(job["state"].as_str(), Some("queued" | "running")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(job["state"], "done", "{job}");
    assert!(job["board"]["total_events"].as_i64().unwrap() > 0, "{job}");
    // The dashboard: "Run it" goes to the job's page, which ends showing
    // the board.
    let owner = login_cookie(&h).await;
    let r = send(
        &h,
        "POST",
        "/p/default/board/jobs",
        Some(&owner),
        &format!("from={from}"),
    )
    .await;
    assert!(r.status().is_redirection(), "{}", r.status());
    let page = location(&r);
    assert!(page.starts_with("/p/default/board/jobs/"), "{page}");
    let html = settled(&h, &page, &owner).await;
    assert!(html.contains("Daily active users"), "{html}");
}

#[tokio::test]
async fn a_board_job_is_its_starters_alone_limited_and_cancellable() {
    let h = harness();
    captured(&h).await;
    let owner = login_cookie(&h).await;
    let viewer = |email: &str| {
        serde_json::json!({"email": email, "role": "viewer", "projects": ["default"]})
    };
    let ana = invite(&h, viewer("[EMAIL_REDACTED]")).await;
    let bob = invite(&h, viewer("[EMAIL_REDACTED]")).await;
    let before = h.state.boards.computed();
    // While a rollup pass holds the query pool, boards wait: the jobs stay
    // running (two at once) or queued.
    let quiet = h.state.boards.exclusive().await;
    let start = |cookie: String| {
        let h = &h;
        async move {
            let r = send(h, "POST", "/p/default/board/jobs", Some(&cookie), "").await;
            assert!(r.status().is_redirection(), "{}", r.status());
            location(&r)
        }
    };
    let mine = start(owner.clone()).await;
    assert_eq!(
        start(owner.clone()).await,
        mine,
        "one job per person: the second goes to the first"
    );
    let anas = start(ana.clone()).await;
    let bobs = start(bob.clone()).await;
    tokio::task::yield_now().await;
    let html = body_text(get(&h, &bobs, Some(&bob), false).await).await;
    assert!(html.contains("Waiting for its turn"), "{html}");

    // Nobody else reads or cancels a job, on the page or the API.
    let theirs = get(&h, &mine, Some(&ana), false).await;
    assert_eq!(theirs.status(), StatusCode::NOT_FOUND);
    let r = send(&h, "POST", &format!("{mine}/cancel"), Some(&ana), "").await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let anas_id = anas.rsplit('/').next().unwrap();
    let api = format!("/api/projects/default/board/jobs/{anas_id}");
    assert_eq!(
        get(&h, &api, None, true).await.status(),
        StatusCode::NOT_FOUND
    );
    // The owner's admin token is the owner: their job is running, so the
    // API refuses a second one and names it.
    let r = send(&h, "POST", "/api/projects/default/board/jobs", None, "").await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert!(mine.ends_with(v["job"].as_str().unwrap()), "{v}");

    // Cancelled, it stays cancelled and computes nothing.
    let r = send(&h, "POST", &format!("{mine}/cancel"), Some(&owner), "").await;
    assert!(r.status().is_redirection());
    drop(quiet);
    let html = settled(&h, &bobs, &bob).await;
    assert!(html.contains("Daily active users"), "{html}");
    let html = settled(&h, &anas, &ana).await;
    assert!(html.contains("Daily active users"), "{html}");
    let html = settled(&h, &mine, &owner).await;
    assert!(html.contains("Cancelled"), "{html}");
    let mine_id = mine.rsplit('/').next().unwrap();
    let r = get(
        &h,
        &format!("/api/projects/default/board/jobs/{mine_id}"),
        None,
        true,
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert_eq!(v["state"], "cancelled", "{v}");
    assert_eq!(
        h.state.boards.computed(),
        before + 1,
        "the two other jobs shared one computation; the cancelled one ran none"
    );
}

//! The deploy hook (#504, D-053): one fixture per platform format, a
//! wrong secret answers like no route, rotation, bounds, and deploys on
//! the Releases page and in `releases.list`; a release recorded by the
//! deploy step with its commit range (#521, D-066).

use super::*;

/// A fixture of `tests/fixtures/deploys`.
fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/deploys")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A new hook URL for the default project, as its path.
async fn make_hook(h: &Harness) -> String {
    let r = router(h.state.clone())
        .oneshot(
            Request::post("/api/projects/default/deploy-hook")
                .header("authorization", "Bearer adm_test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let url = v["url"].as_str().unwrap();
    url[url.find("/hooks/deploy/").unwrap()..].to_owned()
}

async fn send(
    h: &Harness,
    path: &str,
    body: String,
    headers: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::post(path).header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let r = router(h.state.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let s = r.status();
    (
        s,
        serde_json::from_str(&body_text(r).await).unwrap_or_default(),
    )
}

/// A deploy fixture: (file, headers, query, status, environment, commit or release).
type Case<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
);

#[tokio::test]
async fn every_platform_format_is_recorded_with_its_status_environment_and_build() {
    let h = harness();
    let hook = make_hook(&h).await;
    let gh = [("x-github-event", "deployment_status")];
    let flux = [("gotk-component", "kustomize-controller")];
    // (fixture, headers, query, status, environment, commit or release)
    let cases: &[Case] = &[
        (
            "coolify_success.json",
            &[],
            "",
            "succeeded",
            "production",
            None,
        ),
        (
            "coolify_failed_preview.json",
            &[],
            "",
            "failed",
            "preview",
            None,
        ),
        (
            "vercel_succeeded.json",
            &[],
            "",
            "succeeded",
            "production",
            Some("9f8e7d6c5b4a39281706f5e4d3c2b1a098765432"),
        ),
        (
            "vercel_error_preview.json",
            &[],
            "",
            "failed",
            "preview",
            Some("1234567890abcdef1234567890abcdef12345678"),
        ),
        (
            "netlify_ready.json",
            &[],
            "",
            "succeeded",
            "production",
            Some("abcdef0123456789abcdef0123456789abcdef01"),
        ),
        (
            "netlify_error_preview.json",
            &[],
            "",
            "failed",
            "preview",
            Some("fedcba9876543210fedcba9876543210fedcba98"),
        ),
        (
            "render_deploy_ended_failed.json",
            &[],
            "?env=staging",
            "failed",
            "staging",
            None,
        ),
        (
            "fly_machine_env.json",
            &[],
            "",
            "succeeded",
            "production",
            Some("deployment-01JDEXAMPLE0000000000000000"),
        ),
        (
            "github_deployment_status.json",
            &gh[..],
            "",
            "succeeded",
            "staging",
            Some("c0ffee0123456789c0ffee0123456789c0ffee01"),
        ),
        (
            "argocd_template.json",
            &[],
            "",
            "succeeded",
            "production",
            Some("5e4d3c2b1a0918273645abcdef0123456789abcd"),
        ),
        (
            "argocd_app_json.json",
            &[],
            "",
            "failed",
            "staging",
            Some("deadbeef0123456789deadbeef0123456789dead"),
        ),
        (
            "flux_validation_failed.json",
            &flux[..],
            "",
            "failed",
            "production",
            Some("731f7eaddfb6af01cb2173e18f0f75b0ba780ef1"),
        ),
        (
            "flux_reconciled.json",
            &[],
            "",
            "succeeded",
            "staging",
            Some("v2.1.0"),
        ),
        (
            "plain_rolled_back.json",
            &[],
            "",
            "rolled_back",
            "production",
            Some("v1.3.9"),
        ),
    ];
    let mut wrong = Vec::new();
    for (file, headers, query, status, env, build) in cases {
        let (s, v) = send(&h, &format!("{hook}{query}"), fixture(file), headers).await;
        let got_build = v["release"].as_str().or_else(|| v["commit"].as_str());
        if s != StatusCode::OK
            || v["status"] != *status
            || v["environment"] != *env
            || got_build != *build
        {
            wrong.push(format!("{file}: {s} {v}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    let stored = h.state.meta.deploys("default", 0).unwrap();
    assert_eq!(stored.len(), cases.len(), "each one recorded");
    // Notifications that are not deploys are accepted and not recorded.
    for (file, headers) in [
        ("github_ping.json", &[("x-github-event", "ping")][..]),
        ("render_build_started.json", &[][..]),
    ] {
        let (s, _) = send(&h, &hook, fixture(file), headers).await;
        assert_eq!(s, StatusCode::ACCEPTED, "{file}");
    }
    assert_eq!(
        h.state.meta.deploys("default", 0).unwrap().len(),
        cases.len()
    );
    // An unknown shape is refused, and says what is expected.
    let (s, v) = send(&h, &hook, r#"{"hello": "world"}"#.into(), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v.to_string().contains("status"), "{v}");
}

#[tokio::test]
async fn a_wrong_secret_answers_like_no_route_and_rotation_retires_the_old_url() {
    let h = harness();
    let body = || fixture("plain_rolled_back.json");
    let none = send(&h, "/no/such/route", body(), &[]).await.0;
    assert_eq!(none, StatusCode::NOT_FOUND);
    // Before any hook, and with a well-formed secret that is not one.
    let fake = format!("/hooks/deploy/dh_{}", "a".repeat(40));
    assert_eq!(send(&h, &fake, body(), &[]).await.0, none);
    let old = make_hook(&h).await;
    assert_eq!(send(&h, &old, body(), &[]).await.0, StatusCode::OK);
    assert_eq!(send(&h, &fake, body(), &[]).await.0, none);
    assert_eq!(send(&h, "/hooks/deploy/short", body(), &[]).await.0, none);

    // Rotation from the Releases page: the new URL is shown once.
    let cookie = login_cookie(&h).await;
    let (s, _) = post_form_body(&h, "/p/default/releases/hook", &cookie, "").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "rotating needs the box ticked");
    let (s, page) = post_form_body(&h, "/p/default/releases/hook", &cookie, "confirm=yes").await;
    assert_eq!(s, StatusCode::OK);
    let at = page.find("/hooks/deploy/dh_").unwrap();
    let new: String = page[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || "/_-".contains(*c))
        .collect();
    assert_ne!(new, old);
    assert_eq!(
        send(&h, &old, body(), &[]).await.0,
        none,
        "the old URL is gone"
    );
    assert_eq!(send(&h, &new, body(), &[]).await.0, StatusCode::OK);
    let later = body_text(get(&h, "/p/default/releases", Some(&cookie), false).await).await;
    assert!(!later.contains(&new), "shown once");
    assert!(later.contains("Rotate the URL"));
    let log = h.state.meta.audit_log(10).unwrap();
    assert!(log.iter().any(|e| e.3 == "deploys.rotate_hook"));
    // The stored value is not the secret.
    let secret = new.rsplit('/').next().unwrap();
    assert_eq!(
        h.state.meta.deploy_hook_project(secret).unwrap().as_deref(),
        Some("default")
    );
}

#[tokio::test]
async fn the_hook_bounds_its_body_and_its_rate() {
    let h = harness();
    let hook = make_hook(&h).await;
    let big = format!(
        r#"{{"status": "succeeded", "pad": "{}"}}"#,
        "x".repeat(300 * 1024)
    );
    assert_eq!(
        send(&h, &hook, big, &[]).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        send(&h, &hook, "not json".into(), &[]).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut limited = 0;
    for _ in 0..40 {
        let s = send(&h, &hook, r#"{"status": "started"}"#.into(), &[])
            .await
            .0;
        if s == StatusCode::TOO_MANY_REQUESTS {
            limited += 1;
        }
    }
    assert!(limited > 0, "a burst of deploys is cut");
    let stored = h.state.meta.deploys("default", 0).unwrap().len();
    assert!(stored < 40, "{stored}");
}

#[tokio::test]
async fn deploys_show_on_the_releases_page_and_in_the_list() {
    let h = harness();
    let hook = make_hook(&h).await;
    let ev = serde_json::json!({"key": h.key, "batch": [
        {"event": "$pageview", "distinct_id": "u1", "properties": {"$release": "0123456789ab"}}
    ]});
    let (s, _) = post(&h, "/capture", ev.to_string().into_bytes(), false).await;
    assert_eq!(s, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    // A deploy of the release by its full commit, and a failed one with no
    // traffic.
    let ok = r#"{"status": "succeeded", "commit": "0123456789abcdef0123456789abcdef01234567"}"#;
    assert_eq!(send(&h, &hook, ok.into(), &[]).await.0, StatusCode::OK);
    let bad = r#"{"status": "failed", "release": "v9.9.9"}"#;
    assert_eq!(send(&h, &hook, bad.into(), &[]).await.0, StatusCode::OK);

    let r = get(&h, "/api/projects/default/releases", None, true).await;
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let rows = v["releases"].as_array().unwrap();
    let live = rows
        .iter()
        .find(|x| x["release"] == "0123456789ab")
        .unwrap();
    assert_eq!(live["deploy"], "succeeded");
    assert!(live["deployed_at"].is_string(), "{live}");
    let failed = rows.iter().find(|x| x["release"] == "v9.9.9").unwrap();
    assert_eq!(
        (failed["state"].as_str(), failed["users"].as_i64()),
        (Some("failed"), Some(0))
    );
    assert_eq!(v["deploys"].as_array().unwrap().len(), 2);
    assert!(v["deploy_hook"].is_string());

    let cookie = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/releases", Some(&cookie), false).await).await;
    assert!(page.contains("v9.9.9") && page.contains("failed"));
}

/// `POST /api/projects/{id}/releases` (#521, D-066), as `openlens-cli
/// release` sends it.
async fn record(
    h: &Harness,
    project: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(
        h,
        &format!("/api/projects/{project}/releases"),
        body.to_string(),
        &[("authorization", "Bearer adm_test")],
    )
    .await
}

#[tokio::test]
async fn a_release_from_the_deploy_step_is_a_deploy_with_its_commit_range() {
    let h = harness();
    let (s, first) = record(
        &h,
        "default",
        serde_json::json!({"release": "v1", "commit": "AAAAAAA1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{first}");
    assert!(first["from"].is_null(), "no earlier release: {first}");
    assert_eq!(first["environment"], "production");
    // The next one starts after the previous recorded commit…
    let (s, second) = record(
        &h,
        "default",
        serde_json::json!({"release": "v2", "commit": "bbbbbbb2"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{second}");
    assert_eq!(second["from"], first["commit"]);
    // …unless the deploy step says where it starts.
    let body = serde_json::json!({"release": "v3", "commit": "ccccccc3", "from": "aaaaaaa1"});
    let (_, third) = record(&h, "default", body).await;
    assert_eq!(third["from"], "aaaaaaa1");
    // Another environment has its own range.
    let body = serde_json::json!({"release": "v3", "commit": "ccccccc3", "environment": "staging"});
    let (_, staged) = record(&h, "default", body).await;
    assert!(staged["from"].is_null(), "{staged}");

    for bad in [
        serde_json::json!({"release": "v4", "commit": "not-a-sha"}),
        serde_json::json!({"release": "v4", "commit": "ddddddd4", "from": "nope"}),
        serde_json::json!({"release": "", "commit": "ddddddd4"}),
        serde_json::json!({"commit": "ddddddd4"}),
    ] {
        assert_eq!(
            record(&h, "default", bad.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    let (s, _) = record(&h, "nope", serde_json::json!({"release": "v1"})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let anon = send(
        &h,
        "/api/projects/default/releases",
        r#"{"release":"v9"}"#.into(),
        &[],
    )
    .await;
    assert!(anon.0.is_client_error(), "{}", anon.0);

    // Listed like the deploys the hook reports, with no hook set up.
    let r = get(&h, "/api/projects/default/releases", None, true).await;
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let deploys = v["deploys"].as_array().unwrap();
    assert_eq!(deploys.len(), 4, "{v}");
    assert!(
        deploys
            .iter()
            .all(|d| d["source"] == "cli" && d["status"] == "succeeded")
    );
}

/// Delivery metrics (#523, D-068) from the hook's deploys: a failed
/// deploy is a failed change, and the card is on the Releases page.
#[tokio::test]
async fn deploys_feed_the_delivery_metrics() {
    let h = harness();
    let hook = make_hook(&h).await;
    for body in [
        r#"{"status": "succeeded", "release": "v1"}"#,
        r#"{"status": "failed", "release": "v2"}"#,
        r#"{"status": "succeeded", "release": "v3"}"#,
    ] {
        assert_eq!(send(&h, &hook, body.into(), &[]).await.0, StatusCode::OK);
    }
    let r = get(&h, "/api/projects/default/delivery?days=7", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let prod = v["environments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["environment"] == "production")
        .unwrap();
    assert_eq!(prod["source"], "deploys");
    assert!(
        prod["deploys"].as_u64() < prod["changes"].as_u64(),
        "{prod}"
    );
    assert!(prod["failures"].as_u64() > Some(0));
    assert!(prod["lead_time_secs"].is_null() && prod["lead_time_note"].is_string());
    let bad = get(&h, "/api/projects/default/delivery?days=0", None, true).await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let cookie = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/releases", Some(&cookie), false).await).await;
    assert!(page.contains("Deploy frequency") && page.contains("Time to restore"));
}

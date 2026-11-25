//! Environments per project (#410, D-034): host rules at ingestion, the
//! upgrade seed, and the API.

use super::*;

fn page(host: &str, uuid: u32, env: Option<&str>) -> serde_json::Value {
    let mut props = serde_json::json!({ "$current_url": format!("https://{host}/board") });
    if let Some(e) = env {
        props["$environment"] = e.into();
    }
    serde_json::json!({
        "event": "$pageview",
        "distinct_id": "u1",
        "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
        "properties": props,
    })
}

/// Events stored as they were before this change: straight into the
/// writer, no rules applied.
fn stored_before_upgrade(h: &Harness, events: Vec<serde_json::Value>) {
    let batch: openlens_event::CaptureBatch =
        serde_json::from_value(serde_json::json!({ "batch": events })).unwrap();
    let n = openlens_event::normalize_batch("default", batch, chrono::Utc::now()).unwrap();
    h.state.writer.lock().unwrap().write(&n.events).unwrap();
    openlens_server::flush_now(&h.state).unwrap();
}

async fn capture(h: &Harness, events: Vec<serde_json::Value>) -> serde_json::Value {
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v
}

async fn api(
    h: &Harness,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer adm_test");
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = router(h.state.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn seeded(h: &Harness) {
    for _ in 0..100 {
        if h.state
            .meta
            .project_setting("default", "environments_seeded")
            .unwrap()
            .is_some()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the upgrade seed did not run");
}

/// Environments stored per host, from the Parquet files.
async fn stored(h: &Harness) -> std::collections::BTreeMap<String, Vec<String>> {
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql(
            "default",
            "SELECT DISTINCT split_part(url, '/', 3) AS host, environment FROM events ORDER BY 1, 2",
        )
        .await
        .unwrap();
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for row in r.rows {
        out.entry(row["host"].as_str().unwrap_or_default().to_owned())
            .or_default()
            .push(row["environment"].as_str().unwrap_or_default().to_owned());
    }
    out
}

#[tokio::test]
async fn an_upgrade_keeps_known_hosts_in_production_and_new_ones_are_unknown() {
    let h = harness();
    stored_before_upgrade(&h, vec![page("kanban.example.com", 1, None)]);
    // The first batch after the upgrade: nothing moves before the seed ran.
    capture(&h, vec![page("fresh.example.com", 2, None)]).await;
    seeded(&h).await;
    capture(
        &h,
        vec![
            page("kanban.example.com", 3, None),
            page("kanban-test.example.com", 4, None),
            page("staging.example.com", 5, Some("staging")),
            // Set by the app: it wins over everything.
            page("qa-box.example.com", 6, Some("qa")),
        ],
    )
    .await;
    let s = stored(&h).await;
    assert_eq!(s["kanban.example.com"], ["production"], "{s:?}");
    assert_eq!(
        s["fresh.example.com"],
        ["production"],
        "before the seed: {s:?}"
    );
    assert_eq!(s["kanban-test.example.com"], ["unknown"], "{s:?}");
    assert_eq!(s["staging.example.com"], ["staging"], "{s:?}");
    assert_eq!(s["qa-box.example.com"], ["qa"], "{s:?}");

    let (status, v) = api(&h, "GET", "/api/projects/default/environments", None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let unclassified: Vec<&str> = v["unclassified"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    assert!(unclassified.contains(&"kanban-test.example.com"), "{v}");
    assert!(!unclassified.contains(&"kanban.example.com"), "{v}");
}

#[tokio::test]
async fn a_rule_set_through_the_api_applies_to_the_next_events() {
    let h = harness();
    // A fresh install: the seed finds nothing to keep.
    capture(&h, vec![]).await;
    seeded(&h).await;
    for (rule, want) in [
        (
            serde_json::json!({ "environment_rule": { "host": "kanban-test.example.com", "env": "staging" } }),
            StatusCode::OK,
        ),
        (
            serde_json::json!({ "environment_rule": { "host": "*.junk.example.com", "env": "ignore" } }),
            StatusCode::OK,
        ),
        (
            serde_json::json!({ "environment_rule": { "host": "bad host", "env": "staging" } }),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({ "environment_rule": { "host": "x.example.com", "env": "unknown" } }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, v) = api(&h, "PATCH", "/api/projects/default", Some(rule)).await;
        assert_eq!(status, want, "{v}");
    }
    let v = capture(
        &h,
        vec![
            page("kanban-test.example.com", 1, None),
            page("a.junk.example.com", 2, None),
        ],
    )
    .await;
    assert_eq!(v["accepted"], 1, "the ignored host is dropped: {v}");
    let s = stored(&h).await;
    assert_eq!(s["kanban-test.example.com"], ["staging"], "{s:?}");
    assert!(!s.contains_key("a.junk.example.com"), "{s:?}");

    // Removing the rule: the host is unclassified again.
    let (status, _) = api(
        &h,
        "PATCH",
        "/api/projects/default",
        Some(serde_json::json!({ "remove_environment_rule": "kanban-test.example.com" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    capture(&h, vec![page("kanban-test.example.com", 3, None)]).await;
    let s = stored(&h).await;
    assert!(
        s["kanban-test.example.com"].contains(&"unknown".to_owned()),
        "{s:?}"
    );
}

#[tokio::test]
async fn the_settings_tab_sets_and_removes_a_rule_like_the_api() {
    let h = harness();
    capture(&h, vec![page("app.example.com", 1, Some("production"))]).await;
    let owner = login_cookie(&h).await;
    let uri = "/p/default/settings/environments";
    let status = post_form(&h, uri, &owner, "host=Shop.Example.com&env=staging").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, v) = api(&h, "GET", "/api/projects/default/environments", None).await;
    assert!(
        v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["host"] == "shop.example.com" && r["env"] == "staging"),
        "{v}"
    );
    let html = body_text(get(&h, uri, Some(&owner), false).await).await;
    assert!(html.contains("shop.example.com"), "{html}");
    assert!(
        html.contains("app.example.com"),
        "the hosts seen are listed: {html}"
    );

    // A bad host is refused with a message, and the rules do not change.
    let (status, _) = post_form_body(&h, uri, &owner, "host=bad+host&env=staging").await;
    assert_ne!(status, StatusCode::SEE_OTHER);

    let status = post_form(&h, uri, &owner, "host=shop.example.com&remove=yes").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, v) = api(&h, "GET", "/api/projects/default/environments", None).await;
    assert!(
        !v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["host"] == "shop.example.com"),
        "{v}"
    );
}

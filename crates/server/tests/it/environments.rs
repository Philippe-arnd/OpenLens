//! Environments per project (#410, D-034): host rules and aliases at
//! ingestion, the upgrade seed, hidden environments, per-environment
//! checks, and the API.

use super::*;

pub(super) fn page(host: &str, uuid: u32, env: Option<&str>) -> serde_json::Value {
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

pub(super) async fn capture(h: &Harness, events: Vec<serde_json::Value>) -> serde_json::Value {
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v
}

pub(super) async fn api(
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

async fn patch(h: &Harness, body: serde_json::Value) {
    let (status, v) = api(h, "PATCH", "/api/projects/default", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn an_alias_renames_what_the_app_sends_at_ingestion() {
    let h = harness();
    capture(&h, vec![]).await;
    seeded(&h).await;
    patch(
        &h,
        serde_json::json!({ "environment_alias": { "from": "live-eu", "to": "production" } }),
    )
    .await;
    patch(
        &h,
        serde_json::json!({ "environment_alias": { "from": "sandbox", "to": "ignore" } }),
    )
    .await;
    // A chain is refused: `production` is not itself an alias source.
    let (status, _) = api(
        &h,
        "PATCH",
        "/api/projects/default",
        Some(serde_json::json!({ "environment_alias": { "from": "eu", "to": "live-eu" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v = capture(
        &h,
        vec![
            page("eu.example.com", 1, Some("live-eu")),
            page("sbx.example.com", 2, Some("sandbox")),
        ],
    )
    .await;
    assert_eq!(v["accepted"], 1, "the ignored alias is dropped: {v}");
    let s = stored(&h).await;
    assert_eq!(s["eu.example.com"], ["production"], "{s:?}");
    assert!(!s.contains_key("sbx.example.com"), "{s:?}");
}

#[tokio::test]
async fn a_new_sdk_sending_unknown_is_classified_by_the_host_rules() {
    let h = harness();
    capture(&h, vec![]).await;
    seeded(&h).await;
    patch(
        &h,
        serde_json::json!({ "environment_rule": { "host": "app.example.com", "env": "production" } }),
    )
    .await;
    capture(
        &h,
        vec![
            page("app.example.com", 1, Some("unknown")),
            page("other.example.com", 2, Some("unknown")),
        ],
    )
    .await;
    let s = stored(&h).await;
    assert_eq!(s["app.example.com"], ["production"], "{s:?}");
    assert_eq!(s["other.example.com"], ["unknown"], "{s:?}");
}

#[tokio::test]
async fn a_hidden_environment_leaves_the_selector_but_stays_readable() {
    let h = harness();
    capture(
        &h,
        vec![
            page("app.example.com", 1, Some("production")),
            page("qa-box.example.com", 2, Some("qa")),
        ],
    )
    .await;
    openlens_server::flush_now(&h.state).unwrap();
    let envs = |v: &serde_json::Value| -> Vec<String> {
        v["environments"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (_, before) = api(&h, "GET", "/api/projects/default/board", None).await;
    assert!(envs(&before).contains(&"qa".to_owned()), "{before}");

    patch(&h, serde_json::json!({ "hide_environment": "qa" })).await;
    let (status, _) = api(
        &h,
        "PATCH",
        "/api/projects/default",
        Some(serde_json::json!({ "hide_environment": "production" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "production stays shown");

    let (_, after) = api(&h, "GET", "/api/projects/default/board", None).await;
    assert!(!envs(&after).contains(&"qa".to_owned()), "{after}");
    let owner = login_cookie(&h).await;
    let html = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(!html.contains(r#"value="qa""#), "not in the selector");

    // Still stored, and readable by name.
    let (status, qa) = api(&h, "GET", "/api/projects/default/board?env=qa", None).await;
    assert_eq!(status, StatusCode::OK, "{qa}");
    assert!(qa["total_events"].as_i64().unwrap_or(0) > 0, "{qa}");
    assert!(
        envs(&qa).contains(&"qa".to_owned()),
        "the one opened stays listed"
    );
}

#[tokio::test]
async fn a_monitored_environment_is_announced_only_at_the_notify_level() {
    let h = harness();
    for (env, level) in [("staging", "log"), ("qa", "notify")] {
        patch(
            &h,
            serde_json::json!({ "environment_monitor": { "env": env, "url": "http://127.0.0.1:9", "level": level } }),
        )
        .await;
    }
    let (status, _) = api(
        &h,
        "PATCH",
        "/api/projects/default",
        Some(serde_json::json!({ "environment_monitor": { "env": "production", "url": "http://127.0.0.1:9" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "production keeps the app address"
    );
    // Both have been down for ten minutes, longer than the wait.
    let ten_min_ago = chrono::Utc::now().timestamp() - 600;
    for env in ["staging", "qa"] {
        h.state
            .meta
            .start_incident("default", &format!("env:{env}:down"), ten_min_ago, "down")
            .unwrap();
    }
    openlens_server::alert_tick(&h.state, 7).await;
    openlens_server::alert_tick(&h.state, 8).await;
    let incidents = h.state.meta.incidents("default", 0).unwrap();
    let notified = |rule: &str| {
        incidents
            .iter()
            .find(|i| i.0 == rule)
            .map(|i| i.4)
            .unwrap_or_else(|| panic!("no {rule} incident: {incidents:?}"))
    };
    assert!(notified("env:qa:down"), "{incidents:?}");
    assert!(!notified("env:staging:down"), "{incidents:?}");
    // Production's own status is not touched by another environment.
    assert!(!incidents.iter().any(|i| i.0 == "down"), "{incidents:?}");
}

/// Environments stored per page address, from the Parquet files.
async fn stored_by_url(h: &Harness) -> std::collections::BTreeMap<String, String> {
    let r = h
        .state
        .query
        .sql("default", "SELECT url, environment FROM events")
        .await
        .unwrap();
    r.rows
        .iter()
        .map(|row| {
            (
                row["url"].as_str().unwrap_or_default().to_owned(),
                row["environment"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// Re-labelling (#484): the dry run counts the moves and writes nothing;
/// the rewrite moves only that host's events inside the window, is
/// audited, and is refused past 90 days.
#[tokio::test]
async fn relabelling_moves_only_the_hosts_events_in_the_window() {
    let h = harness();
    let at = |host: &str, path: &str, uuid: u32, days_ago: i64| {
        let mut e = page(host, uuid, None);
        e["properties"]["$current_url"] = format!("https://{host}/{path}").into();
        e["timestamp"] = (chrono::Utc::now() - chrono::Duration::days(days_ago))
            .to_rfc3339()
            .into();
        e
    };
    // Stored as production before any rule said otherwise.
    stored_before_upgrade(
        &h,
        vec![
            at("kanban-test.example.com", "a", 1, 2),
            at("kanban-test.example.com", "b", 2, 0),
            at("kanban-test.example.com", "old", 3, 20),
            at("shop.example.com", "c", 4, 2),
        ],
    );
    patch(
        &h,
        serde_json::json!({ "environment_rule": { "host": "kanban-test.example.com", "env": "staging" } }),
    )
    .await;
    let before = stored_by_url(&h).await;
    let uri = "/api/projects/default/environments";

    let body = serde_json::json!({ "host": "kanban-test.example.com", "days": 7, "dry_run": true });
    let (status, dry) = api(&h, "POST", uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{dry}");
    assert_eq!(dry["events"], 2, "{dry}");
    assert_eq!(
        dry["moves"],
        serde_json::json!([{ "from": "production", "to": "staging", "events": 2 }])
    );
    assert_eq!(stored_by_url(&h).await, before, "a dry run writes nothing");

    let body = serde_json::json!({ "host": "kanban-test.example.com", "days": 7 });
    let (status, done) = api(&h, "POST", uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["moves"], dry["moves"], "{done}");
    let after = stored_by_url(&h).await;
    assert_eq!(after["https://kanban-test.example.com/a"], "staging");
    assert_eq!(after["https://kanban-test.example.com/b"], "staging");
    assert_eq!(
        after["https://kanban-test.example.com/old"], "production",
        "outside the window"
    );
    assert_eq!(after["https://shop.example.com/c"], "production");
    assert_eq!(after.len(), before.len());
    let log = h.state.meta.audit_log(20).unwrap();
    assert!(
        log.iter()
            .any(|e| e.3 == "environments.relabel" && e.4.contains("kanban-test")),
        "{log:?}"
    );

    let body = serde_json::json!({ "host": "kanban-test.example.com", "days": 91 });
    let (status, _) = api(&h, "POST", uri, Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The settings tab previews the moves (a GET), then posts the rewrite.
#[tokio::test]
async fn the_settings_tab_previews_then_relabels() {
    let h = harness();
    stored_before_upgrade(&h, vec![page("kanban-test.example.com", 1, None)]);
    patch(
        &h,
        serde_json::json!({ "environment_rule": { "host": "kanban-test.example.com", "env": "qa" } }),
    )
    .await;
    let owner = login_cookie(&h).await;
    let uri = "/p/default/settings/environments";
    let html = body_text(
        get(
            &h,
            &format!("{uri}?relabel=kanban-test.example.com&days=7"),
            Some(&owner),
            false,
        )
        .await,
    )
    .await;
    assert!(html.contains("Re-label 1 event"), "{html}");
    assert_eq!(
        stored_by_url(&h).await["https://kanban-test.example.com/board"],
        "production"
    );
    let status = post_form(
        &h,
        uri,
        &owner,
        "action=relabel&host=kanban-test.example.com&days=7",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        stored_by_url(&h).await["https://kanban-test.example.com/board"],
        "qa"
    );
}

/// `GET /flags` asked from a page of `origin`.
async fn flags_from(h: &Harness, origin: &str) -> serde_json::Value {
    let req = Request::builder()
        .uri(format!("/flags?key={}&distinct_id=u1", h.key))
        .header("origin", origin)
        .body(Body::empty())
        .unwrap();
    let resp = router(h.state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    serde_json::from_str(&body_text(resp).await).unwrap()
}

#[tokio::test]
async fn the_landing_host_is_internal_and_listed_as_a_sister_of_the_app() {
    // #710, D-102: a landing page on www., the app on app., one project.
    let h = harness();
    capture(&h, vec![]).await;
    seeded(&h).await;
    for (host, env) in [
        ("www.example.com", "production"),
        ("app.example.com", "production"),
        ("*.preview.example.com", "production"),
        ("staging.example.com", "staging"),
        ("junk.example.com", "ignore"),
    ] {
        patch(
            &h,
            serde_json::json!({ "environment_rule": { "host": host, "env": env } }),
        )
        .await;
    }
    let landing = |id: &str, referrer: &str| {
        serde_json::json!({
            "event": "$pageview",
            "distinct_id": id,
            "properties": {
                "$current_url": "https://app.example.com/signup",
                "$pathname": "/signup",
                "$lang": "en",
                "$referrer": referrer,
            },
        })
    };
    capture(
        &h,
        vec![
            landing("from-landing", "https://www.example.com/pricing"),
            landing("from-blog", "https://blog.someone.dev/post"),
        ],
    )
    .await;
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql(
            "default",
            "SELECT distinct_id, properties FROM raw_events ORDER BY distinct_id",
        )
        .await
        .unwrap();
    let row = |id: &str| {
        r.rows
            .iter()
            .find(|row| row.get("distinct_id").and_then(|v| v.as_str()) == Some(id))
            .map(|row| serde_json::to_string(row).unwrap())
            .unwrap_or_default()
    };
    assert!(
        row("from-landing").contains(r#"\"$channel\":\"internal\""#),
        "{:?}",
        r.rows
    );
    assert!(
        row("from-blog").contains(r#"\"$channel\":\"referral\""#),
        "a foreign referrer stays a referral: {:?}",
        r.rows
    );

    // `/flags` from the landing lists the app, and nothing else.
    let v = flags_from(&h, "https://www.example.com").await;
    let sisters: Vec<&str> = v["sister_hosts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    assert!(sisters.contains(&"app.example.com"), "{v}");
    assert!(
        !sisters.iter().any(|s| s.starts_with("*.")
            || *s == "www.example.com"
            || *s == "staging.example.com"
            || *s == "junk.example.com"),
        "{v}"
    );
    // A page on a host no rule names: nothing to link.
    let v = flags_from(&h, "https://elsewhere.org").await;
    assert!(
        v["sister_hosts"].as_array().is_some_and(Vec::is_empty),
        "{v}"
    );
}

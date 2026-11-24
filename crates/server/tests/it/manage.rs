//! Managing flags, surveys, maps and storage; the analysis pages. Moved out of main.rs as is (#307).

use super::*;

#[tokio::test]
async fn flags_and_surveys_are_managed_from_the_dashboard() {
    let h = harness();
    let owner = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/p/default/flags-edit", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "key=new-editor&description=Editor&rollout=20&roles=staff&enabled=on"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "key=new-editor&rollout=5"
        )
        .await,
        StatusCode::BAD_REQUEST,
        "duplicate key"
    );
    assert_eq!(
        post_form(&h, "/p/default/flags-edit", &owner, "key=Bad+Key").await,
        StatusCode::BAD_REQUEST
    );
    // Rename keeps one flag.
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "original=new-editor&key=editor&rollout=30"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let keys: Vec<String> = h.state.meta.flags("default").unwrap();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].contains("\"editor\"") && keys[0].contains("30"));
    assert_eq!(
        post_form(&h, "/p/default/flags/editor/delete", &owner, "confirm=nope").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags/editor/delete",
            &owner,
            "confirm=editor"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.flags("default").unwrap().is_empty());

    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys-edit",
            &owner,
            "key=useful&question=How+useful%3F&kind=choice&choices=a&trigger=saved&sample=10"
        )
        .await,
        StatusCode::BAD_REQUEST,
        "a choice needs 2 options"
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys-edit",
            &owner,
            "key=useful&question=How+useful%3F&kind=rating&trigger=saved&sample=10&enabled=on"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(h.state.meta.surveys("default").unwrap().len(), 1);
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys/useful/delete",
            &viewer,
            "confirm=useful"
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/surveys/useful/delete",
            &owner,
            "confirm=useful"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(h.state.meta.surveys("default").unwrap().is_empty());
}

#[tokio::test]
async fn people_breakdown_and_sql_pages() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let page = get(&h, "/p/default/people/u1", Some(&owner), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(html.contains("$pageview") && html.contains("owner"));
    assert!(!html.contains(" style="));
    assert_eq!(
        get(&h, "/p/default/people/nobody", Some(&owner), false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for by in ["role", "environment", "release", "page"] {
        let r = get(
            &h,
            &format!("/p/default/breakdown?by={by}"),
            Some(&owner),
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK, "{by}");
    }
    assert_eq!(
        get(&h, "/p/default/breakdown?by=nope", Some(&owner), false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&h, "/p/default/sql", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    let run = router(h.state.clone())
        .oneshot(
            Request::post("/p/default/sql")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "sql=SELECT+event%2C+count(*)+AS+n+FROM+events+GROUP+BY+event&format=csv",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run.status(), StatusCode::OK);
    let csv = body_text(run).await;
    assert!(
        csv.starts_with("event,n\n") && csv.contains("$pageview,1"),
        "{csv}"
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        get(&h, "/p/default/sql", Some(&viewer), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(&h, "/p/default/sql", &viewer, "sql=SELECT+1").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn the_feature_map_is_read_checked_and_saved_from_the_dashboard() {
    let h = harness();
    let owner = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/p/default/features", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/features",
            &owner,
            "action=save&yaml=features%3A+%5B%7Bkey%3A+%22Bad+Key%22%7D%5D"
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a map with errors is not saved"
    );
    assert!(h.state.meta.feature_map("default").unwrap().is_none());
    let yaml = "features:\n  - key: search\n    name: Search\n    success: search_done\n    entry: { routes: ['/search'] }\n";
    let form = |action: &str| format!("action={action}&yaml={}", openlens_ui::url_enc(yaml));
    let check = router(h.state.clone())
        .oneshot(
            Request::post("/p/default/features")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form("check")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(check.status(), StatusCode::OK);
    let html = body_text(check).await;
    assert!(
        html.contains("Changes against the stored map") && html.contains("<ins>+ features:"),
        "diff shown"
    );
    assert_eq!(
        post_form(&h, "/p/default/features", &owner, &form("save")).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(h.state.meta.feature_map("default").unwrap().unwrap().0, 1);
    let page = body_text(get(&h, "/p/default/features", Some(&owner), false).await).await;
    assert!(page.contains("not seen") && page.contains("Version 1"));
    let json = body_text(get(&h, "/p/default/features/map.json", Some(&owner), false).await).await;
    assert!(json.contains("\"search_done\""), "{json}");
    assert_eq!(
        get(&h, "/p/default/features/map.yaml?v=1", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(&h, "/p/default/features", &viewer, &form("save")).await,
        StatusCode::FORBIDDEN
    );
    let ro = body_text(get(&h, "/p/default/features", Some(&viewer), false).await).await;
    assert!(!ro.contains("<textarea"), "read-only for viewers");
}

#[tokio::test]
async fn storage_and_backups_from_the_dashboard() {
    let h = harness();
    let owner = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/settings/storage", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        get(&h, "/settings/storage", Some(&viewer), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(&h, "/settings/storage/backups/run", &viewer, "").await,
        StatusCode::FORBIDDEN
    );

    // S3: the secret is sealed, never shown, kept when left empty.
    assert_eq!(
        post_form(
            &h,
            "/settings/storage/s3",
            &owner,
            "action=save&bucket=my-bucket&region=eu-west-1&endpoint=http%3A%2F%2F127.0.0.1%3A1&access_key=AKID&secret=TopSecret123"
        )
        .await,
        StatusCode::OK
    );
    let sealed = h.state.meta.setting("s3:secret").unwrap().unwrap();
    assert!(!sealed.contains("TopSecret123"));
    let page = body_text(get(&h, "/settings/storage", Some(&owner), false).await).await;
    assert!(page.contains("my-bucket") && !page.contains("TopSecret123"));
    post_form(
        &h,
        "/settings/storage/s3",
        &owner,
        "action=save&bucket=my-bucket&access_key=AKID2&secret=",
    )
    .await;
    assert_eq!(h.state.meta.setting("s3:secret").unwrap().unwrap(), sealed);
    let test = router(h.state.clone())
        .oneshot(
            Request::post("/settings/storage/s3")
                .header("cookie", &owner)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("action=test"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(test).await.contains("Test failed"), "no S3 there");
    post_form(&h, "/settings/storage/s3", &owner, "action=remove").await;
    assert!(
        h.state
            .meta
            .setting("s3:bucket")
            .unwrap()
            .is_none_or(|v| v.is_empty())
    );

    // Plan validation, then a manual backup that lands in the history.
    let bad = body_text(
        router(h.state.clone())
            .oneshot(
                Request::post("/settings/storage/backups")
                    .header("cookie", &owner)
                    .header("origin", "http://localhost:8686")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("schedule=daily&hour=3&keep=0"))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(bad.contains("keep 1 to 60"));
    post_form(
        &h,
        "/settings/storage/backups",
        &owner,
        "schedule=daily&hour=3&weekday=1&keep=2",
    )
    .await;
    assert_eq!(
        post_form(&h, "/settings/storage/backups/run", &owner, "").await,
        StatusCode::SEE_OTHER
    );
    for _ in 0..100 {
        if h.state.meta.setting("backup:history").unwrap().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let hist = h.state.meta.setting("backup:history").unwrap().unwrap();
    assert!(hist.contains("\"ok\":true"), "{hist}");
    let dl = get(
        &h,
        "/settings/storage/backups/latest.tar.gz",
        Some(&owner),
        false,
    )
    .await;
    assert_eq!(dl.status(), StatusCode::OK);
    assert_eq!(dl.headers()["content-type"], "application/gzip");
}

#[tokio::test]
async fn page_views_get_device_browser_and_os_but_never_the_address() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$pageview", "distinct_id": "u1", "properties": {"$pathname": "/", "$country": "ZZ", "$lang": "fr-FR", "$sw": 390}},
            {"event": "signup", "distinct_id": "u1"}
        ]
    })
    .to_string();
    let resp = router(h.state.clone())
        .oneshot(
            Request::post("/capture")
                .header("content-type", "application/json")
                .header("x-forwarded-for", "198.51.100.77")
                .header("user-agent", "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 Version/17.5 Mobile/15E148 Safari/604.1")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql(
            "default",
            "SELECT event, properties FROM raw_events ORDER BY event",
        )
        .await
        .unwrap();
    let all = serde_json::to_string(&r.rows).unwrap();
    assert!(!all.contains("198.51.100.77"), "no address stored");
    assert!(!all.contains("ZZ"), "client-sent country replaced");
    let pv = r.rows.iter().find(|x| x["event"] == "$pageview").unwrap()["properties"]
        .as_str()
        .unwrap();
    assert!(
        pv.contains(r#""$device":"phone""#)
            && pv.contains(r#""$browser":"Safari""#)
            && pv.contains(r#""$os":"iOS""#),
        "{pv}"
    );
    let other = r.rows.iter().find(|x| x["event"] == "signup").unwrap()["properties"]
        .as_str()
        .unwrap();
    assert!(!other.contains("$device"), "only page views are enriched");
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/audience", Some(&owner), false).await).await;
    assert!(page.contains("phone") && page.contains("Safari") && !page.contains(" style="));
}

#[tokio::test]
async fn the_feature_flow_page_and_api() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$pageview", "distinct_id": "u1", "session_id": "s1", "properties": {"$pathname": "/", "$feature": "search"}},
            {"event": "click", "distinct_id": "u1", "session_id": "s1", "properties": {"$feature": "cart"}}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    for u in [
        "/p/default/flows",
        "/p/default/flows?to=success",
        "/p/default/flows?from=cart",
    ] {
        let r = get(&h, u, Some(&owner), false).await;
        assert_eq!(r.status(), StatusCode::OK, "{u}");
        assert!(!body_text(r).await.contains(" style="));
    }
    let api = body_text(get(&h, "/api/projects/default/flows", None, true).await).await;
    let v: serde_json::Value = serde_json::from_str(&api).unwrap();
    assert!(v["sessions"].as_i64().unwrap() <= 1, "{api}");
}

#[tokio::test]
async fn the_live_view_shows_who_is_here_and_refreshes() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let r = get(
        &h,
        "/p/default/live?env=production&x=evil",
        Some(&owner),
        false,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers()["refresh"],
        "10;url=/p/default/live?env=production"
    );
    let html = body_text(r).await;
    assert!(html.contains("Active now") && html.contains("$pageview") && !html.contains(" style="));
}

#[tokio::test]
async fn a_failing_probe_opens_one_incident_and_alerts_are_configurable() {
    let h = harness();
    let owner = login_cookie(&h).await;
    // Bad settings are refused; good ones saved.
    assert_eq!(
        post_form(
            &h,
            "/p/default/settings/alerts",
            &owner,
            "down=on&down_wait=2&health_path=%2Fhealthz&webhook=http%3A%2F%2Fx"
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/settings/alerts",
            &owner,
            "down=on&down_wait=2&health_path=%2Fhealthz&ntfy=ops-alerts"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    // No email, webhook or reachable ntfy in tests: the test alert reports the failure.
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(&h, "/p/default/settings/alerts", &viewer, "down=on").await,
        StatusCode::FORBIDDEN
    );
    // A private address is never probed: it counts as a failure, like a dead app.
    h.state
        .meta
        .set_project_setting("default", "app_url", "http://127.0.0.1:9")
        .unwrap();
    openlens_server::alert_tick(&h.state, 1).await;
    assert!(
        h.state
            .meta
            .open_incident("default", "down")
            .unwrap()
            .is_none(),
        "one failure is not down yet"
    );
    openlens_server::alert_tick(&h.state, 2).await;
    let open = h.state.meta.open_incident("default", "down").unwrap();
    assert!(
        open.is_some_and(|(_, _, notified)| !notified),
        "down, announced only after the wait"
    );
    openlens_server::alert_tick(&h.state, 3).await;
    assert_eq!(
        h.state.meta.incidents("default", 0).unwrap().len(),
        1,
        "no storm: one incident"
    );
    let api = body_text(get(&h, "/api/projects/default/status", None, true).await).await;
    assert!(api.contains(r#""state":"down""#), "{api}");
    let page = body_text(get(&h, "/p/default/uptime", Some(&owner), false).await).await;
    assert!(page.contains("Down") && page.contains("/healthz") && !page.contains(" style="));
    let s = body_text(get(&h, "/p/default/settings", Some(&owner), false).await).await;
    assert!(s.contains("ops-alerts"));
}

#[tokio::test]
async fn a_new_error_hitting_two_users_is_announced_once() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$error", "distinct_id": "u1", "session_id": "s1", "properties": {"$message": "TypeError: x is undefined", "$pathname": "/"}},
            {"event": "$error", "distinct_id": "u2", "session_id": "s2", "properties": {"$message": "TypeError: x is undefined", "$pathname": "/"}}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    openlens_server::alert_tick(&h.state, 1).await;
    openlens_server::alert_tick(&h.state, 61).await;
    let all = h.state.meta.incidents("default", 0).unwrap();
    let errors: Vec<_> = all.iter().filter(|i| i.0.starts_with("error:")).collect();
    assert_eq!(errors.len(), 1, "{all:?}");
    assert!(
        errors[0].3.contains("TypeError") && errors[0].2.is_some(),
        "announced and closed at once"
    );
}

#[tokio::test]
async fn landings_get_a_channel_and_only_the_referrer_host_is_kept() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$pageview", "distinct_id": "u1", "properties": {"$current_url": "https://app.example.com/", "$pathname": "/", "$lang": "en", "$referrer": "https://www.google.com/search?q=secret+words"}},
            {"event": "$pageview", "distinct_id": "u2", "properties": {"$current_url": "https://app.example.com/?utm_source=news&utm_medium=email", "$pathname": "/", "$lang": "en", "$utm_source": "news", "$utm_medium": "email"}},
            {"event": "$pageview", "distinct_id": "u3", "properties": {"$current_url": "https://app.example.com/", "$pathname": "/", "$lang": "en"}},
            {"event": "$pageview", "distinct_id": "u3", "properties": {"$current_url": "https://app.example.com/b", "$pathname": "/b"}}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql(
            "default",
            "SELECT distinct_id, properties FROM raw_events ORDER BY distinct_id, timestamp",
        )
        .await
        .unwrap();
    let all = serde_json::to_string(&r.rows).unwrap();
    assert!(
        !all.contains("secret"),
        "the referrer's path and query are not kept"
    );
    assert!(
        all.contains(r#"\"$ref_host\":\"google.com\""#)
            && all.contains(r#"\"$channel\":\"search\""#),
        "{all}"
    );
    assert!(
        all.contains(r#"\"$channel\":\"email\""#) && all.contains(r#"\"$channel\":\"direct\""#)
    );
    assert_eq!(
        all.matches("$channel").count(),
        3,
        "only the first view of a load"
    );
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/acquisition", Some(&owner), false).await).await;
    assert!(
        page.contains("search")
            && page.contains("email")
            && page.contains("low sample")
            && !page.contains(" style=")
    );
    assert_eq!(
        get(
            &h,
            "/p/default/acquisition?by=source&touch=last",
            Some(&owner),
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&h, "/p/default/breakdown?by=channel", Some(&owner), false)
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_churn_page_and_api() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/churn", Some(&owner), false).await).await;
    assert!(
        page.contains("Churn rate")
            && page.contains("How long they stay")
            && !page.contains(" style=")
    );
    let api: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/churn", None, true).await).await,
    )
    .unwrap();
    assert_eq!(api["window"], 14);
    assert_eq!(api["users"], 1);
}

#[tokio::test]
async fn revenue_shows_only_when_success_events_carry_amounts() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(
        !board.contains("Revenue, last 30 days"),
        "nothing changes without amounts"
    );

    h.state
        .meta
        .save_feature_map(
            "default",
            "revenue:\n  currency: EUR\n  rates: { USD: 0.9 }\nfeatures:\n  - key: checkout\n    name: Checkout\n    success: order_paid\n    entry: { routes: ['/checkout'] }\n",
        )
        .unwrap();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "order_paid", "distinct_id": "u1", "properties": {"revenue": 10, "currency": "EUR"}},
            {"event": "order_paid", "distinct_id": "u2", "properties": {"value": 20.5, "currency": "usd"}},
            {"event": "note_saved", "distinct_id": "u1", "properties": {"revenue": 999}},
            {"event": "$vital", "distinct_id": "u1", "properties": {"$name": "LCP", "$value": 1234}}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(
        board.contains("Revenue, last 30 days") && board.contains("28.45 EUR"),
        "10 + 20.5 × 0.9, other events ignored"
    );
    let page = body_text(get(&h, "/p/default/revenue", Some(&owner), false).await).await;
    assert!(page.contains("checkout") && page.contains("Converted") && !page.contains(" style="));
}

#[tokio::test]
async fn goals_are_suggested_set_tracked_and_removed() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/goals", Some(&owner), false).await).await;
    assert!(
        page.contains("Set a goal")
            && page.contains("Sessions with errors")
            && !page.contains(" style=")
    );
    let deadline = (chrono::Utc::now() + chrono::Duration::days(60))
        .format("%Y-%m-%d")
        .to_string();
    assert_eq!(
        post_form(
            &h,
            "/p/default/goals",
            &owner,
            &format!("kpi=error_rate&target=150&deadline={deadline}")
        )
        .await,
        StatusCode::BAD_REQUEST,
        "a share above 100%"
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/goals",
            &owner,
            "kpi=error_rate&target=5&deadline=2020-01-01"
        )
        .await,
        StatusCode::BAD_REQUEST,
        "a past deadline"
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/goals",
            &owner,
            &format!("kpi=error_rate&target=5&deadline={deadline}&owner=Ana")
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(
            &h,
            "/p/default/goals",
            &viewer,
            &format!("kpi=error_rate&target=5&deadline={deadline}")
        )
        .await,
        StatusCode::FORBIDDEN
    );
    let api: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/goals", None, true).await).await,
    )
    .unwrap();
    let g = &api["goals"][0];
    assert_eq!(g["goal"]["target"], 0.05);
    assert_eq!(g["goal"]["owner"], "Ana");
    assert!(
        g["status"] == "reached" || g["status"] == "not enough data",
        "{g}"
    );
    let id = g["goal"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        post_form(&h, &format!("/p/default/goals/{id}/delete"), &owner, "").await,
        StatusCode::SEE_OTHER
    );
    let api: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/goals", None, true).await).await,
    )
    .unwrap();
    assert_eq!(api["goals"].as_array().unwrap().len(), 0);
}

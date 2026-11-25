//! Cookieless visitors, experiments, alerts, map health, goals and churn. Moved out of main.rs as is (#307).

use super::*;

#[tokio::test]
async fn cookieless_visitors_get_a_daily_id_and_no_durable_link() {
    let h = harness();
    let send = |batch: serde_json::Value| {
        let state = h.state.clone();
        let body = serde_json::json!({"key": h.key, "batch": batch}).to_string();
        async move {
            router(state)
                .oneshot(
                    Request::post("/capture")
                        .header("content-type", "application/json")
                        .header("x-forwarded-for", "198.51.100.23")
                        .header(
                            "user-agent",
                            "Mozilla/5.0 (X11; Linux x86_64) Firefox/132.0",
                        )
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(
        send(serde_json::json!([{"event": "$pageview", "distinct_id": "$cookieless", "session_id": "$cookieless", "properties": {"$pathname": "/"}}])).await,
        StatusCode::OK
    );
    assert_eq!(
        send(serde_json::json!([
            {"event": "$pageview", "distinct_id": "$cookieless", "session_id": "$cookieless", "properties": {"$pathname": "/b"}},
            {"event": "$identify", "distinct_id": "ana", "session_id": "$cookieless", "properties": {"$anon_distinct_id": "$cookieless"}}
        ]))
        .await,
        StatusCode::OK
    );
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql(
            "default",
            "SELECT distinct_id, session_id, properties FROM raw_events ORDER BY timestamp",
        )
        .await
        .unwrap();
    let all = serde_json::to_string(&r.rows).unwrap();
    assert!(
        !all.contains("$cookieless") && !all.contains("198.51.100.23"),
        "{all}"
    );
    let ids: std::collections::HashSet<&str> = r
        .rows
        .iter()
        .filter(|x| x["distinct_id"] != "ana")
        .filter_map(|x| x["distinct_id"].as_str())
        .collect();
    assert_eq!(ids.len(), 1, "one visitor for the day: {ids:?}");
    let sessions: std::collections::HashSet<&str> = r
        .rows
        .iter()
        .filter_map(|x| x["session_id"].as_str())
        .collect();
    assert_eq!(sessions.len(), 1, "one session while active");
    assert!(
        !all.contains("anon_distinct_id"),
        "the daily id is never linked to the login"
    );
}

#[tokio::test]
async fn experiment_flags_hand_out_variants_and_show_each_arm() {
    let h = harness();
    let owner = login_cookie(&h).await;
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "key=checkout&rollout=100&enabled=on&variants=control%3A50%2C+short%3A40"
        )
        .await,
        StatusCode::BAD_REQUEST,
        "weights must add up to 100"
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "key=checkout&rollout=100&enabled=on&variants=control%3A50%2C+short%3A50"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let r = router(h.state.clone())
        .oneshot(
            Request::get(format!("/flags?key={}&distinct_id=u1", h.key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let arm = v["variants"]["checkout"].as_str().unwrap().to_owned();
    assert!(arm == "control" || arm == "short", "{v}");
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$pageview", "distinct_id": "u1", "session_id": "s1", "properties": {"$pathname": "/", "$flags": "checkout", "$variants": format!("checkout={arm}")}},
            {"event": "$pageview", "distinct_id": "u2", "session_id": "s2", "properties": {"$pathname": "/", "$flags": "checkout", "$variants": "other=x,checkout=short"}}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let page = body_text(
        get(
            &h,
            "/p/default/flags-edit?key=checkout",
            Some(&owner),
            false,
        )
        .await,
    )
    .await;
    assert!(
        page.contains("Variants, last 30 days")
            && page.contains("<code>short</code>")
            && !page.contains(" style="),
        "{page}"
    );
}

#[tokio::test]
async fn quiet_hours_hold_non_urgent_alerts_and_down_shows_on_the_board() {
    use chrono::Timelike;
    let h = harness();
    let owner = login_cookie(&h).await;
    let now = chrono::Utc::now().hour();
    let (a, b) = (now, (now + 1) % 24);
    assert_eq!(
        post_form(
            &h,
            "/p/default/settings/alerts",
            &owner,
            &format!(
                "down=on&down_wait=2&health_path=%2F&regressions=on&quiet_start={a}&quiet_end={b}"
            )
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$error", "distinct_id": "u1", "session_id": "s1", "properties": {"$message": "RangeError: boom", "$pathname": "/"}},
            {"event": "$error", "distinct_id": "u2", "session_id": "s2", "properties": {"$message": "RangeError: boom", "$pathname": "/"}}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    openlens_server::alert_tick(&h.state, 1).await;
    let queue = h
        .state
        .meta
        .project_setting("default", "alert_queue")
        .unwrap()
        .unwrap_or_default();
    assert!(
        queue.contains("RangeError"),
        "held during quiet hours: {queue}"
    );
    // Quiet hours off: the held alert goes out (nowhere to go here) and the queue empties.
    post_form(
        &h,
        "/p/default/settings/alerts",
        &owner,
        "down=on&down_wait=2&health_path=%2F&regressions=on",
    )
    .await;
    openlens_server::alert_tick(&h.state, 3).await;
    assert!(
        h.state
            .meta
            .project_setting("default", "alert_queue")
            .unwrap()
            .is_none_or(|q| q.is_empty())
    );
    // Two failed probes: the board says the app is down.
    h.state
        .meta
        .set_project_setting("default", "app_url", "http://127.0.0.1:9")
        .unwrap();
    openlens_server::alert_tick(&h.state, 4).await;
    openlens_server::alert_tick(&h.state, 6).await;
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(
        board.contains("The app is down since"),
        "banner on the board"
    );
}

#[tokio::test]
async fn map_health_finds_planted_defects_and_versions_keep_author_and_message() {
    let h = harness();
    let put = |uri: &'static str, body: &'static str| {
        let st = h.state.clone();
        async move {
            let resp = router(st)
                .oneshot(
                    Request::put(uri)
                        .header("authorization", "Bearer adm_test")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = resp.status();
            let v: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), 1 << 20)
                    .await
                    .unwrap(),
            )
            .unwrap();
            (status, v)
        }
    };
    let (status, _) = put(
        "/api/projects/default/feature-map",
        "features:\n  - key: docs\n    name: Docs\n    entry: { routes: ['/docs/*'] }\n",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, v) = put(
        "/api/projects/default/feature-map?message=split%20docs%20and%20edit",
        "features:\n  - key: reading\n    name: Reading\n    area: Docs\n    renamed_from: [docs, ghost]\n    success: doc_read\n    entry: { routes: ['/docs/*'] }\n  - key: edit\n    name: Edit\n    area: Docs\n    requires: [reading]\n    success: doc_saved\n    entry: { routes: ['/docs/:id/edit'] }\n  - key: gone\n    name: Gone\n    entry: { routes: ['/gone'] }\n",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["path"] == "features[0].renamed_from[1]"),
        "ghost was never a feature: {v}"
    );
    let mut events = vec![
        serde_json::json!({"event": "$pageview", "distinct_id": "u1", "properties": {"$pathname": "/docs/1/edit"}}),
        // One session that used `reading`, with flags, a release and an error (#411).
        serde_json::json!({"event": "$click", "distinct_id": "u9", "session_id": "s-feat", "properties": {"$pathname": "/docs/3", "$feature": "reading", "$flags": "beta-x,new-nav", "$release": "1.2.0"}}),
        serde_json::json!({"event": "$error", "distinct_id": "u9", "session_id": "s-feat", "properties": {"$pathname": "/docs/3", "$message": "Boom \"quoted\" here", "$release": "1.2.0"}}),
    ];
    for i in 0..5 {
        events.push(serde_json::json!({"event": "$pageview", "distinct_id": format!("p{i}"), "properties": {"$pathname": format!("/billing/{i}")}}));
        events.push(serde_json::json!({"event": "$click", "distinct_id": "u1", "properties": {"$pathname": "/docs/2", "$feature": "docs"}}));
    }
    let (status, _) = post(
        &h,
        "/capture",
        serde_json::json!({ "key": h.key, "batch": events })
            .to_string()
            .into_bytes(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();

    assert_eq!(
        get(&h, "/api/projects/default/feature-map/health", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let health: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/feature-map/health", None, true).await).await,
    )
    .unwrap();
    let kinds: Vec<&str> = health["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["kind"].as_str())
        .collect();
    for k in ["coverage", "overlap", "dead"] {
        assert!(kinds.contains(&k), "{k} missing: {health}");
    }
    assert!(health["score"].as_u64().unwrap() < 100);
    let reading = &health["features"][0];
    assert_eq!(reading["key"], "reading");
    assert!(
        reading["uses"].as_i64().unwrap() >= 5,
        "old `docs` tags count for `reading`: {health}"
    );
    assert!(
        health["areas"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["area"] == "Docs" && a["features"] == 2),
        "{health}"
    );
    assert_eq!(health["ladder"][0]["to"], "edit");

    let ctx: serde_json::Value = serde_json::from_str(
        &body_text(
            get(
                &h,
                "/api/projects/default/feature-map/features/docs",
                None,
                true,
            )
            .await,
        )
        .await,
    )
    .unwrap();
    assert_eq!(
        ctx["definition"]["key"], "reading",
        "a former key finds the feature"
    );
    assert_eq!(ctx["required_by"][0], "edit");
    // #411: per release, errors in its sessions, flags while used.
    assert!(
        ctx["releases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["release"] == "1.2.0"),
        "{ctx}"
    );
    assert_eq!(ctx["errors"][0]["message"], "Boom \"quoted\" here", "{ctx}");
    assert_eq!(ctx["errors"][0]["last_session"], "s-feat");
    let flags: Vec<&str> = ctx["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flag"].as_str())
        .collect();
    assert!(
        flags.contains(&"beta-x") && flags.contains(&"new-nav"),
        "{ctx}"
    );
    assert!(
        ctx["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["session_id"] == "s-feat"),
        "{ctx}"
    );
    assert_eq!(ctx["area_peers"][0], "edit");
    assert_eq!(
        get(
            &h,
            "/api/projects/default/feature-map/features/nope",
            None,
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // The feature's own page (#411): breadcrumb, tabs, a former key redirects.
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/features/reading", Some(&owner), false).await).await;
    assert!(page.contains("aria-label=\"Breadcrumb\""), "{page}");
    assert!(
        page.contains("href=\"/p/default/features\">Feature map"),
        "{page}"
    );
    assert!(page.contains("aria-current=\"page\">Overview"), "{page}");
    assert!(
        page.contains("href=\"/p/default/features/edit\""),
        "a related feature links"
    );
    let hist = body_text(
        get(
            &h,
            "/p/default/features/reading?tab=history",
            Some(&owner),
            false,
        )
        .await,
    )
    .await;
    assert!(hist.contains("aria-current=\"page\">History") && hist.contains("Map versions"));
    // Every tab carries the filters and has its own view (#411).
    assert!(
        page.contains("/p/default/features/reading?tab=errors&amp;env="),
        "{page}"
    );
    for (tab, needle) in [
        ("errors", "Boom"),
        ("replays", "Recorded sessions that used it"),
        ("flags", "beta-x"),
        ("releases", "1.2.0"),
    ] {
        let r = get(
            &h,
            &format!("/p/default/features/reading?tab={tab}"),
            Some(&owner),
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK, "{tab}");
        let body = body_text(r).await;
        assert!(body.contains(needle), "{tab}: {body}");
    }
    let old = get(&h, "/p/default/features/docs", Some(&owner), false).await;
    assert!(old.status().is_redirection());
    let gone = get(&h, "/p/default/features/nope", Some(&owner), false).await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    assert!(body_text(gone).await.contains("Back to the feature map"));
    // The downloads keep their URLs.
    let yaml = get(&h, "/p/default/features/map.yaml", Some(&owner), false).await;
    assert_eq!(yaml.status(), StatusCode::OK);
    // The map lists each feature as a link to its page.
    let map = body_text(get(&h, "/p/default/features", Some(&owner), false).await).await;
    assert!(map.contains("href=\"/p/default/features/reading\""));

    let versions: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/feature-map/versions", None, true).await).await,
    )
    .unwrap();
    assert_eq!(versions["versions"][0]["version"], 2);
    assert_eq!(versions["versions"][0]["message"], "split docs and edit");
    assert_eq!(versions["versions"][0]["author"], "api (admin token)");

    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/features", Some(&owner), false).await).await;
    assert!(
        page.contains("Map health, last 30 days")
            && page.contains("Adoption ladder")
            && page.contains("split docs and edit")
            && !page.contains(" style="),
    );
}

#[tokio::test]
async fn flag_rules_target_plan_and_custom_properties() {
    let h = harness();
    let resp = router(h.state.clone())
        .oneshot(
            Request::put("/api/projects/default/flags/pro-export")
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"key":"pro-export","enabled":true,"rollout":100,"rules":[{"property":"plan","op":"in","values":["pro","team"]},{"property":"seats","op":"not_in","values":["1"]}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let flags = |q: &'static str| {
        let (st, key) = (h.state.clone(), h.key.clone());
        async move {
            let resp = router(st)
                .oneshot(
                    Request::get(format!("/flags?key={key}&distinct_id=u1{q}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let v: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), 1 << 20)
                    .await
                    .unwrap(),
            )
            .unwrap();
            v["flags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "pro-export")
        }
    };
    assert!(flags("&plan=pro&p.seats=12").await);
    assert!(
        flags("&p.plan=Team&p.seats=3").await,
        "p.plan works too, any case"
    );
    assert!(!flags("&plan=free&p.seats=12").await);
    assert!(!flags("&plan=pro&p.seats=1").await);
    assert!(!flags("&plan=pro").await, "seats not sent: out");
    // A bad rule is refused.
    let resp = router(h.state.clone())
        .oneshot(
            Request::put("/api/projects/default/flags/bad")
                .header("authorization", "Bearer adm_test")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"key":"bad","enabled":true,"rollout":100,"rules":[{"property":"Bad Key","op":"in","values":["x"]}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn goals_part_two_guardrails_objectives_and_the_api() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    // A goal stored before #272 (no guardrails, no releases) still loads.
    h.state
        .meta
        .set_project_setting(
            "default",
            "goals",
            r#"[{"id":"old1","kpi":"error_rate","target":0.0,"up":false,"deadline":"2099-01-01","owner":"","baseline":0.0,"created":"2026-01-01"}]"#,
        )
        .unwrap();
    let set = |body: serde_json::Value| {
        let st = h.state.clone();
        async move {
            let resp = router(st)
                .oneshot(
                    Request::post("/api/projects/default/goals")
                        .header("authorization", "Bearer adm_test")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let s = resp.status();
            let v: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), 1 << 20)
                    .await
                    .unwrap(),
            )
            .unwrap_or_default();
            (s, v)
        }
    };
    let deadline = (chrono::Utc::now() + chrono::Duration::days(60))
        .format("%Y-%m-%d")
        .to_string();
    let (s, v) = set(serde_json::json!({
        "kpi": "bounce_rate", "target": 0.1, "deadline": deadline,
        "guardrails": [{"kpi": "avg_secs", "limit": 1000.0}, {"kpi": "error_rate"}]
    }))
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["goal"]["guardrails"].as_array().unwrap().len(), 2);
    let gid = v["goal"]["id"].as_str().unwrap().to_owned();
    let (s, v) =
        set(serde_json::json!({ "kpi": "bounce_rate", "target": 0.1, "releases": 3 })).await;
    assert_eq!(
        s,
        StatusCode::UNPROCESSABLE_ENTITY,
        "no releases seen yet: {v}"
    );
    let (s, _) =
        set(serde_json::json!({ "kpi": "bounce_rate", "target": 3.0, "deadline": deadline })).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "a share above 1");

    let owner = login_cookie(&h).await;
    let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    assert_eq!(
        post_form(
            &h,
            "/p/default/objectives",
            &owner,
            &format!("title=Calmer+sessions&goal1={gid}&end={tomorrow}")
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/objectives",
            &owner,
            &format!("title=x&goal1=nope&end={tomorrow}")
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    let page = body_text(get(&h, "/p/default/goals", Some(&owner), false).await).await;
    assert!(
        page.contains("Calmer sessions")
            && page.contains("guardrail breached")
            && !page.contains(" style="),
        "avg_secs is under its 1000 s floor"
    );
    // The period ends: the hourly pass archives it and alerts on the guardrail.
    let mut objectives: serde_json::Value = serde_json::from_str(
        &h.state
            .meta
            .project_setting("default", "objectives")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    objectives[0]["end"] = serde_json::json!("2020-01-01");
    h.state
        .meta
        .set_project_setting("default", "objectives", &objectives.to_string())
        .unwrap();
    openlens_server::alert_tick(&h.state, 3).await;
    let objectives: serde_json::Value = serde_json::from_str(
        &h.state
            .meta
            .project_setting("default", "objectives")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        objectives[0]["archived"]["results"][0][0],
        gid.as_str(),
        "{objectives}"
    );
    let incidents = h.state.meta.incidents("default", 0).unwrap();
    assert!(
        incidents
            .iter()
            .any(|i| i.0 == format!("guardrail:{gid}:avg_secs")),
        "{incidents:?}"
    );
    let api: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/goals", None, true).await).await,
    )
    .unwrap();
    assert_eq!(api["goals"].as_array().unwrap().len(), 2, "{api}");
    assert!(
        api["goals"][1]["guardrails"][0]["breached"] == true,
        "{api}"
    );
    let page = body_text(get(&h, "/p/default/goals", Some(&owner), false).await).await;
    assert!(page.contains("Past objectives"));
    let digest = body_text(get(&h, "/api/projects/default/digest", None, true).await).await;
    assert!(digest.contains("## Goals"), "{digest}");
}

#[tokio::test]
async fn churn_part_two_cancels_exports_and_win_back_flags() {
    let h = harness();
    h.state
        .meta
        .save_feature_map(
            "default",
            "churn_event: plan_cancelled\nfeatures:\n  - key: notes\n    name: Notes\n    success: note_saved\n    entry: { routes: ['/notes'] }\n",
        )
        .unwrap();
    let ts = |days: i64| (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let mut batch = Vec::new();
    // "=evil": quiet for 10 of 14 days and slowing down → at risk.
    for d in [27, 26, 25, 20, 18, 10] {
        batch.push(serde_json::json!({"event": "$pageview", "distinct_id": "=evil,\"x\"", "timestamp": ts(d), "properties": {"$pathname": "/notes"}}));
    }
    batch.push(serde_json::json!({"event": "$identify", "distinct_id": "=evil,\"x\"", "timestamp": ts(27), "properties": {"$role": "owner"}}));
    // "quitter" cancels explicitly, yesterday.
    batch.push(serde_json::json!({"event": "$identify", "distinct_id": "quitter", "timestamp": ts(3), "properties": {"$role": "owner"}}));
    batch.push(serde_json::json!({"event": "$pageview", "distinct_id": "quitter", "timestamp": ts(2), "properties": {"$pathname": "/notes"}}));
    batch.push(serde_json::json!({"event": "plan_cancelled", "distinct_id": "quitter", "timestamp": ts(1)}));
    let (status, _) = post(
        &h,
        "/capture",
        serde_json::json!({ "key": h.key, "batch": batch })
            .to_string()
            .into_bytes(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let api: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/churn", None, true).await).await,
    )
    .unwrap();
    assert_eq!(api["cancelled"], 1, "{api}");
    assert_eq!(api["at_risk"][0]["distinct_id"], "=evil,\"x\"", "{api}");
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/churn", Some(&owner), false).await).await;
    assert!(
        page.contains("Top reasons")
            && page.contains("Exit survey")
            && page.contains("plan_cancelled")
            && page.contains("Make the flag")
            && !page.contains(" style="),
    );
    let csv = body_text(get(&h, "/p/default/churn/at-risk.csv", Some(&owner), false).await).await;
    assert!(
        csv.starts_with("distinct_id,role,") && csv.contains("\"'=evil,\"\"x\"\"\""),
        "quoted, and no formula: {csv}"
    );
    assert_eq!(
        get(&h, "/p/default/churn/at-risk.csv", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_form(&h, "/p/default/churn/audience", &owner, "holdout=30").await,
        StatusCode::SEE_OTHER
    );
    let flags = openlens_server::flags_for_tests(&h.state, "default").await;
    let f = flags
        .iter()
        .find(|f| f.key.starts_with("win-back-"))
        .unwrap();
    assert_eq!((f.enabled, f.holdout, f.users.len()), (false, 30, 1));
    let page = body_text(get(&h, "/p/default/churn", Some(&owner), false).await).await;
    assert!(page.contains("Win-back flags"));
}

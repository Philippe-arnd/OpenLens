//! Forgetting, share links, forms and the timeline. Moved out of main.rs as is (#307).

use super::*;

#[tokio::test]
async fn a_forgotten_person_is_gone_everywhere_and_stays_gone() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [
            {"event": "$pageview", "distinct_id": "anon-1", "properties": {"$pathname": "/"}},
            {"event": "$identify", "distinct_id": "ana", "properties": {"$anon_distinct_id": "anon-1", "$role": "admin"}},
            {"event": "note_saved", "distinct_id": "ana"},
            {"event": "note_saved", "distinct_id": "bo"}
        ]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let export =
        body_text(get(&h, "/p/default/people/ana/export.json", Some(&owner), false).await).await;
    assert!(
        export.contains("note_saved") && export.contains("anon-1"),
        "{export}"
    );
    assert_eq!(
        post_form(&h, "/p/default/people/ana/forget", &owner, "confirm=bo").await,
        StatusCode::BAD_REQUEST
    );
    // The identity recorded at capture (#450) holds the link until erasure.
    assert_eq!(
        h.state.meta.aliases_of("default", "ana").unwrap(),
        vec!["anon-1".to_owned()]
    );
    assert_eq!(
        post_form(&h, "/p/default/people/ana/forget", &owner, "confirm=ana").await,
        StatusCode::OK
    );
    let n = |sql: &'static str| {
        let q = h.state.query.clone();
        async move {
            q.sql("default", sql).await.unwrap().rows[0]["n"]
                .as_i64()
                .unwrap()
        }
    };
    assert_eq!(
        n("SELECT count(*) AS n FROM raw_events WHERE distinct_id IN ('ana', 'anon-1')").await,
        0
    );
    assert_eq!(
        n("SELECT count(*) AS n FROM raw_events WHERE distinct_id = 'bo'").await,
        1,
        "others untouched"
    );
    assert!(h.state.meta.person("default", "ana").unwrap().is_none());
    assert!(
        h.state
            .meta
            .aliases_of("default", "ana")
            .unwrap()
            .is_empty()
    );
    openlens_server::identity::backfill(&h.state, "default")
        .await
        .unwrap();
    let kept = h.state.meta.identities("default").unwrap().unwrap();
    assert!(
        kept.persons.iter().all(|p| p.person != "ana")
            && kept
                .aliases
                .iter()
                .all(|(a, p)| a != "anon-1" && p != "ana"),
        "{kept:?}"
    );
    // Anything they send later is dropped.
    let again =
        serde_json::json!({"key": h.key, "batch": [{"event": "note_saved", "distinct_id": "ana"}]})
            .to_string();
    post(&h, "/capture", again.into_bytes(), false).await;
    openlens_server::flush_now(&h.state).unwrap();
    assert_eq!(
        n("SELECT count(*) AS n FROM raw_events WHERE distinct_id = 'ana'").await,
        0
    );
    // The audit log has the export and the erasure; viewers can't see it.
    let log = body_text(get(&h, "/audit", Some(&owner), false).await).await;
    assert!(log.contains("person.forget") && log.contains("person.export"));
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        get(&h, "/audit", Some(&viewer), false).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(&h, "/p/default/people/bo/forget", &viewer, "confirm=bo").await,
        StatusCode::FORBIDDEN
    );
    let api = body_text(get(&h, "/api/audit", None, true).await).await;
    assert!(api.contains("user.invite"));
}

#[tokio::test]
async fn share_links_open_without_login_and_stop_when_revoked() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let (s, page) = post_form_body(
        &h,
        "/p/default/settings/share",
        &owner,
        "scope=board&days=30&embed=https%3A%2F%2Fintranet.example.com",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let token = page
        .split("/s/")
        .nth(1)
        .unwrap()
        .split([' ', '<', '&', '"'])
        .next()
        .unwrap()
        .to_owned();
    let view = get(&h, &format!("/s/{token}"), None, false).await;
    assert_eq!(view.status(), StatusCode::OK);
    let csp = view.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        csp.contains("frame-ancestors https://intranet.example.com"),
        "{csp}"
    );
    let html = body_text(view).await;
    assert!(html.contains("Read-only view") && html.contains("Focus board"));
    assert!(
        !html.contains("action=\"/p/") && !html.contains("/watch?") && !html.contains(" style=")
    );
    assert_eq!(
        get(&h, "/s/sh_nope", None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    let (s, _) = post_form_body(
        &h,
        "/p/default/settings/share",
        &owner,
        "scope=board&embed=http%3A%2F%2Fevil.example",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "http origins refused");
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        post_form(&h, "/p/default/settings/share", &viewer, "scope=board").await,
        StatusCode::FORBIDDEN
    );
    let id = h.state.meta.share_links("default").unwrap()[0].id.clone();
    assert_eq!(
        post_form(
            &h,
            &format!("/p/default/settings/share/{id}/revoke"),
            &owner,
            ""
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        get(&h, &format!("/s/{token}"), None, false).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn forms_show_the_field_where_people_give_up() {
    let h = harness();
    let mut batch = Vec::new();
    let t0 = chrono::Utc::now() - chrono::Duration::minutes(30);
    let mut k = 0;
    let mut at = || {
        k += 1;
        (t0 + chrono::Duration::seconds(k)).to_rfc3339()
    };
    for (s, fields, sent) in [
        ("s1", vec!["email", "phone"], false),
        ("s2", vec!["email", "phone"], false),
        ("s3", vec!["email"], true),
    ] {
        for f in fields {
            batch.push(serde_json::json!({"event": "$field", "distinct_id": s, "session_id": s, "timestamp": at(), "properties": {"$pathname": "/signup", "$form": "signup", "$field": f, "$ms": 1500, "$changed": true}}));
        }
        if sent {
            batch.push(serde_json::json!({"event": "$submit", "distinct_id": s, "session_id": s, "timestamp": at(), "properties": {"$pathname": "/signup", "$form": "signup"}}));
        }
    }
    let body = serde_json::json!({"key": h.key, "batch": batch}).to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/forms", Some(&owner), false).await).await;
    assert!(
        page.contains("signup") && page.contains("66.7% abandoned") && !page.contains(" style="),
        "{page}"
    );
    let phone = page.find(">phone<").unwrap();
    let email = page.find(">email<").unwrap();
    assert!(phone < email, "phone, where people leave, comes first");
}

#[tokio::test]
async fn the_timeline_gathers_changes_and_notes() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    // Anyone signed in adds a note; flag changes come from the audit log.
    assert_eq!(
        post_form(
            &h,
            "/p/default/timeline",
            &viewer,
            &format!("day={today}&body=Pricing+page+shipped")
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_form(
            &h,
            "/p/default/flags-edit",
            &owner,
            "key=beta&rollout=10&enabled=on"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let page = body_text(get(&h, "/p/default/timeline", Some(&owner), false).await).await;
    assert!(
        page.contains("Pricing page shipped")
            && page.contains("Flag beta changed")
            && !page.contains(" style=")
    );
    let api = body_text(
        router(h.state.clone())
            .oneshot(
                Request::post("/api/projects/default/timeline")
                    .header("authorization", "Bearer adm_test")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"body":"Deployed v2"}"#))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(api.contains("\"id\""), "{api}");
    let list = body_text(get(&h, "/api/projects/default/timeline", None, true).await).await;
    assert!(list.contains("Deployed v2") && list.contains("Pricing page shipped"));
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(
        board.contains("class=\"marker ") && board.contains("Pricing page shipped"),
        "today's change is marked under the active users chart"
    );
    // A viewer can't delete the owner's note… but can delete their own.
    let notes = h.state.meta.notes("default", "2000-01-01").unwrap();
    let api_note = notes.iter().find(|n| n.4 == "Deployed v2").unwrap().0;
    let own = notes
        .iter()
        .find(|n| n.4 == "Pricing page shipped")
        .unwrap()
        .0;
    assert_eq!(
        post_form(
            &h,
            &format!("/p/default/timeline/{api_note}/delete"),
            &viewer,
            ""
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(
            &h,
            &format!("/p/default/timeline/{own}/delete"),
            &viewer,
            ""
        )
        .await,
        StatusCode::SEE_OTHER
    );
}

/// The confirmation dialogs (#545): `?confirm=<action>` opens one on the
/// page, holding the form that posts what it always did; an unknown action
/// opens none; and the destructive POSTs still refuse a missing
/// confirmation.
#[tokio::test]
async fn confirm_dialogs_post_the_same_forms_and_an_empty_confirmation_is_refused() {
    let h = harness();
    let body = serde_json::json!({
        "key": h.key,
        "batch": [{"event": "note_saved", "distinct_id": "ana"}]
    })
    .to_string();
    let (status, _) = post(&h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    assert_eq!(
        post_form(&h, "/p/default/flags-edit", &owner, "key=editor&rollout=10").await,
        StatusCode::SEE_OTHER
    );
    for (uri, action) in [
        (
            "/p/default/people/ana?confirm=forget",
            "/p/default/people/ana/forget",
        ),
        (
            "/p/default/flags-edit?key=editor&confirm=delete",
            "/p/default/flags/editor/delete",
        ),
        (
            "/p/default/settings/data?confirm=delete",
            "/p/default/settings/delete",
        ),
    ] {
        let html = body_text(get(&h, uri, Some(&owner), false).await).await;
        assert_eq!(html.matches("<dialog").count(), 1, "{uri}");
        let dialog = &html[html.find("<dialog").unwrap()..html.find("</dialog>").unwrap()];
        assert!(
            dialog.contains(&format!("action=\"{action}\"")),
            "{uri}: {dialog}"
        );
        assert!(dialog.contains("name=\"confirm\""), "{uri}");
        assert!(
            !html.contains("style=") && !html.contains("<script"),
            "{uri}"
        );
    }
    for uri in [
        "/p/default/people/ana",
        "/p/default/people/ana?confirm=wipe",
        "/p/default/flags-edit?key=editor&confirm=forget",
        "/p/default/settings/data?confirm=everything",
    ] {
        let html = body_text(get(&h, uri, Some(&owner), false).await).await;
        assert!(!html.contains("<dialog"), "{uri}");
    }
    for (uri, body) in [
        ("/p/default/people/ana/forget", ""),
        ("/p/default/people/ana/forget", "confirm="),
        ("/p/default/flags/editor/delete", ""),
        ("/p/default/settings/delete", "confirm=+"),
    ] {
        assert_eq!(
            post_form(&h, uri, &owner, body).await,
            StatusCode::BAD_REQUEST,
            "{uri} {body:?}"
        );
    }
    let export =
        body_text(get(&h, "/p/default/people/ana/export.json", Some(&owner), false).await).await;
    assert!(export.contains("note_saved"), "nothing was forgotten");
    assert_eq!(h.state.meta.flags("default").unwrap().len(), 1);
    assert!(h.state.meta.project("default").unwrap().is_some());
}

#[tokio::test]
async fn the_audit_log_names_the_scoped_token_that_acted() {
    let h = harness();
    let (token, plain) = h
        .state
        .meta
        .create_api_token("write", None, "ci-agent")
        .unwrap();
    let map = "features:\n  - key: export-pdf\n    name: Export to PDF\n    entry: { routes: ['/export'] }\n";
    for (bearer, forged) in [(plain.as_str(), true), ("adm_test", false)] {
        let mut req = Request::put("/api/projects/default/feature-map")
            .header("authorization", format!("Bearer {bearer}"));
        if forged {
            // A client cannot name another token: the layer drops this.
            req = req.header("x-openlens-scope-token", "tok_someone_else");
        }
        let r = router(h.state.clone())
            .oneshot(req.body(Body::from(map)).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
    let saves: Vec<String> = h
        .state
        .meta
        .audit_log(50)
        .unwrap()
        .into_iter()
        .filter(|e| e.3 == "feature_map.save")
        .map(|e| e.1)
        .collect();
    assert_eq!(saves.len(), 2, "{saves:?}");
    let scoped = saves.iter().find(|a| a.contains(&token.id));
    assert!(
        scoped.is_some_and(|a| a.contains("ci-agent") && !a.contains("admin")),
        "{saves:?}"
    );
    assert!(
        saves
            .iter()
            .any(|a| a.contains("admin token") && !a.contains("ci-agent")),
        "{saves:?}"
    );
    assert!(
        !saves.iter().any(|a| a.contains("tok_someone_else")),
        "{saves:?}"
    );
}

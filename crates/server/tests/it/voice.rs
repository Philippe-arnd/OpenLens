//! What users say (#718): feedback per feature, texts for admins and the
//! automation token only. Account health (#719): an account whose active
//! users drop is at risk, small accounts are grouped.

use super::*;

async fn put_map(h: &Harness, yaml: &'static str) {
    let r = router(h.state.clone())
        .oneshot(
            Request::put("/api/projects/default/feature-map")
                .header("authorization", "Bearer adm_test")
                .body(Body::from(yaml))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

async fn capture(h: &Harness, events: Vec<serde_json::Value>) {
    let (status, _) = post(
        h,
        "/capture",
        serde_json::json!({ "key": h.key, "batch": events })
            .to_string()
            .into_bytes(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
}

#[tokio::test]
async fn viewers_get_feedback_counts_without_the_texts() {
    let h = harness();
    put_map(
        &h,
        "features:\n  - key: export\n    name: Export\n    entry: { routes: ['/export'] }\n",
    )
    .await;
    let said = "The export button hides under the fold";
    capture(
        &h,
        vec![
            serde_json::json!({"event": "$feedback", "distinct_id": "u1", "properties": {"$text": said, "$about": "export", "$rating": 2}}),
            serde_json::json!({"event": "$feedback", "distinct_id": "u2", "properties": {"text": "Love it", "feature": "export", "rating": 5}}),
            serde_json::json!({"event": "$feedback", "distinct_id": "u3", "properties": {"$text": "Elsewhere"}}),
        ],
    )
    .await;

    // The automation token reads the texts.
    let api: serde_json::Value = serde_json::from_str(
        &body_text(
            get(
                &h,
                "/api/projects/default/feedback?feature=export",
                None,
                true,
            )
            .await,
        )
        .await,
    )
    .unwrap();
    assert_eq!(api["texts"], true, "{api}");
    assert_eq!(api["total"], 2, "{api}");
    let texts: Vec<&str> = api["latest"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect();
    assert!(texts.contains(&said) && texts.contains(&"Love it"), "{api}");
    assert!(!texts.contains(&"Elsewhere"), "{api}");

    // An admin's feature page shows them; a viewer's shows the counts only.
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/features/export", Some(&owner), false).await).await;
    assert!(
        page.contains("What users say") && page.contains(said),
        "{page}"
    );
    let viewer = invite(
        &h,
        serde_json::json!({"email": "viewer@example.com", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    let page = body_text(get(&h, "/p/default/features/export", Some(&viewer), false).await).await;
    assert!(page.contains("What users say"), "{page}");
    assert!(!page.contains(said) && !page.contains("Love it"), "{page}");
    assert!(page.contains("Only admins read what users wrote"), "{page}");
}

#[tokio::test]
async fn an_account_whose_users_drop_is_at_risk_and_small_ones_are_grouped() {
    let h = harness();
    // Nothing new while the app sends no account.
    capture(
        &h,
        vec![serde_json::json!({"event": "$pageview", "distinct_id": "solo", "properties": {"$pathname": "/"}})],
    )
    .await;
    let none: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/accounts", None, true).await).await,
    )
    .unwrap();
    assert!(none["accounts"].as_array().unwrap().is_empty(), "{none}");
    assert_eq!(none["small"]["accounts"], 0, "{none}");
    let owner = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/audience", Some(&owner), false).await).await;
    assert!(!page.contains(r#"id="acc-h""#), "{page}");

    let ts = |d: i64| (chrono::Utc::now() - chrono::Duration::days(d)).to_rfc3339();
    let mut events = Vec::new();
    // Acme: four users active 40 days ago, two of them still active.
    for (i, still) in [(0, true), (1, true), (2, false), (3, false)] {
        let u = format!("acme{i}");
        let props = serde_json::json!({"$account": "acme", "$plan": "pro", "$pathname": "/"});
        events.push(serde_json::json!({"event": "$identify", "distinct_id": u, "timestamp": ts(40), "properties": props.clone()}));
        events.push(serde_json::json!({"event": "$pageview", "distinct_id": u, "timestamp": ts(40), "properties": props.clone()}));
        if still {
            events.push(serde_json::json!({"event": "$pageview", "distinct_id": u, "timestamp": ts(2), "properties": props.clone()}));
        }
    }
    // Duo: two users, both active.
    for i in 0..2 {
        let u = format!("duo{i}");
        let props = serde_json::json!({"$account": "duo", "$pathname": "/"});
        events.push(serde_json::json!({"event": "$identify", "distinct_id": u, "timestamp": ts(3), "properties": props.clone()}));
        events.push(serde_json::json!({"event": "$pageview", "distinct_id": u, "timestamp": ts(3), "properties": props.clone()}));
    }
    capture(&h, events).await;

    let v: serde_json::Value = serde_json::from_str(
        &body_text(get(&h, "/api/projects/default/accounts", None, true).await).await,
    )
    .unwrap();
    let list = v["accounts"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{v}");
    let acme = &list[0];
    assert_eq!(acme["account"], "acme", "{v}");
    assert_eq!(acme["state"], "at_risk", "{v}");
    assert!(
        acme["active_share"].as_f64().unwrap() < acme["previous_share"].as_f64().unwrap(),
        "{v}"
    );
    assert_eq!(acme["plan"], "pro", "{v}");
    assert_eq!(v["small"]["accounts"], 1, "{v}");
    assert!(!v.to_string().contains("duo"), "{v}");
    let page = body_text(get(&h, "/p/default/audience", Some(&owner), false).await).await;
    let section = &page[page.find(r#"id="acc-h""#).unwrap()..];
    assert!(
        section.contains("acme") && section.contains("at risk"),
        "{section}"
    );
    assert!(!section.contains("duo"), "{section}");
}

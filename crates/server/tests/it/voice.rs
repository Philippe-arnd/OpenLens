//! What users say (#718): feedback per feature, texts for admins and the
//! automation token only.

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
    assert!(page.contains("What users say") && page.contains(said), "{page}");
    let viewer = invite(
        &h,
        serde_json::json!({"email": "[EMAIL_REDACTED]", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    let page = body_text(get(&h, "/p/default/features/export", Some(&viewer), false).await).await;
    assert!(page.contains("What users say"), "{page}");
    assert!(!page.contains(said) && !page.contains("Love it"), "{page}");
    assert!(page.contains("Only admins read what users wrote"), "{page}");
}

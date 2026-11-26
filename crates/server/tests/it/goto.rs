//! Go to… (#541): `GET /p/{project}/go?q=…`.

use super::*;

fn location(r: &axum::response::Response) -> String {
    r.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn a_section_name_opens_its_page_whatever_its_case() {
    let h = harness();
    let cookie = login_cookie(&h).await;
    let r = get(&h, "/p/default/go?q=%20ERRORS%20", Some(&cookie), false).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/p/default/errors");
    let r = get(&h, "/p/default/go?q=feature+map", Some(&cookie), false).await;
    assert_eq!(location(&r), "/p/default/features");
}

#[tokio::test]
async fn a_feature_name_opens_the_feature() {
    let h = harness();
    let put = router(h.state.clone())
        .oneshot(
            Request::put("/api/projects/default/feature-map")
                .header("authorization", "Bearer adm_test")
                .body(Body::from(
                    "features:\n  - key: export-pdf\n    name: Export to PDF\n    entry: { routes: ['/export'] }\n",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::OK);
    let cookie = login_cookie(&h).await;
    let r = get(
        &h,
        &format!("/p/default/go?q={}", url_encode("export to pdf")),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/p/default/features/export-pdf");
}

#[tokio::test]
async fn an_unknown_name_lists_nothing_and_stays() {
    let h = harness();
    let cookie = login_cookie(&h).await;
    let r = get(&h, "/p/default/go?q=zzqx-nowhere", Some(&cookie), false).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().get("location").is_none());
    let html = body_text(r).await;
    assert!(html.contains("Nothing matches"));
    assert!(
        html.contains("zzqx-nowhere"),
        "the page says what was typed"
    );
    assert!(
        html.contains("id=\"goto-list\""),
        "the page keeps the sidebar and its field"
    );
}

#[tokio::test]
async fn a_name_longer_than_the_bound_is_refused() {
    let h = harness();
    let cookie = login_cookie(&h).await;
    let bound = openlens_engine::commands::goto::MAX_QUERY;
    let at = get(
        &h,
        &format!("/p/default/go?q={}", "z".repeat(bound)),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(at.status(), StatusCode::OK, "the bound itself is accepted");
    let over = get(
        &h,
        &format!("/p/default/go?q={}", "z".repeat(bound + 1)),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(over.status(), StatusCode::BAD_REQUEST);
    assert!(over.headers().get("location").is_none());
}

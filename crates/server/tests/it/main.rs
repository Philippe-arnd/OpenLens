//! Integration tests for `openlens-server` (one binary per crate).

use axum::{body::Body, http::Request, http::StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn healthz_answers_ok_and_unknown_routes_do_not() {
    let ok = openlens_server::router()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    let missing = openlens_server::router()
        .oneshot(Request::get("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(missing.status().is_client_error());
}

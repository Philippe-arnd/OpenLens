//! What the app answers when no route matches or a request runs out of
//! time (#679): a page inside the shell for a person, a JSON error for an
//! `/api` client. Never an empty body.

use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

/// How long pages, the API and Ask (a few model rounds) may run.
pub(crate) const PAGE_DEADLINE: Duration = Duration::from_secs(120);

/// An `/api` path (the `/api/v1` prefix is rewritten to `/api` first).
fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// The router's fallback: no route has this URL.
pub(crate) async fn not_found(req: Request) -> Response {
    if is_api(req.uri().path()) {
        return crate::apierr::json(StatusCode::NOT_FOUND, "No such endpoint.");
    }
    crate::web::html_page(
        StatusCode::NOT_FOUND,
        openlens_ui::message(
            "Not found",
            "There is no page at this address. It may have moved, or the link has a typo.",
        ),
    )
}

/// Stops a request after [`PAGE_DEADLINE`] and says why (504: the server
/// gave up, not the client).
pub(crate) async fn deadline(req: Request, next: Next) -> Response {
    within(PAGE_DEADLINE, req, next).await
}

async fn within(limit: Duration, req: Request, next: Next) -> Response {
    let api = is_api(req.uri().path());
    match tokio::time::timeout(limit, next.run(req)).await {
        Ok(r) => r,
        Err(_) => timed_out(api, limit),
    }
}

fn timed_out(api: bool, limit: Duration) -> Response {
    let secs = limit.as_secs();
    if api {
        return crate::apierr::with_code(
            StatusCode::GATEWAY_TIMEOUT,
            "busy",
            format!(
                "The request took longer than {secs} s and was stopped. Ask for a shorter period, or run the board as a job (POST /api/projects/{{project}}/board/jobs)."
            ),
        );
    }
    crate::web::html_page(
        StatusCode::GATEWAY_TIMEOUT,
        openlens_ui::message(
            "This page took too long",
            &format!(
                "Building it took longer than {secs} seconds, so the server stopped. Pick a shorter period, or open the board for this period and choose Run it: a board job works in the background and keeps the result for an hour."
            ),
        ),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn slow(path: &str) -> (StatusCode, String, String) {
        let app = Router::new()
            .route("/{*rest}", get(std::future::pending::<&'static str>))
            .layer(axum::middleware::from_fn(|req, next| {
                within(Duration::from_millis(20), req, next)
            }));
        let r = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = r.status();
        let ty = r
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, ty, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn a_slow_page_explains_the_timeout_in_html() {
        let (status, ty, body) = slow("/p/default/funnels").await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert!(ty.starts_with("text/html"), "{ty}");
        assert!(body.contains("<h1") && body.contains("shorter period"));
    }

    #[tokio::test]
    async fn a_slow_api_call_answers_json() {
        let (status, ty, body) = slow("/api/projects/default/events").await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert!(ty.starts_with("application/json"), "{ty}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], "busy");
    }
}

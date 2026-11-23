//! One cross-site request check for every form of the dashboard (#294),
//! instead of a check repeated in each handler.
//!
//! A state-changing request (any method but GET, HEAD and OPTIONS) is
//! accepted when the browser says it comes from this site
//! (`Sec-Fetch-Site: same-origin`, or `none` for a typed URL), or, for a
//! browser that does not send Fetch Metadata, when its `Origin` is this
//! server. A request with neither header does not come from a browser
//! form and cannot carry a victim's cookie across sites, so it is let
//! through. Ingestion (`/capture`, `/e`, `/r`) and the bearer API
//! (`/api/…`) are cross-site by design and not checked here.

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Paths that take cross-site requests on purpose.
fn exempt(path: &str) -> bool {
    matches!(path, "/capture" | "/e" | "/r") || path.starts_with("/api/")
}

/// Whether a state-changing request may proceed.
pub(crate) fn allowed(method: &Method, path: &str, headers: &axum::http::HeaderMap) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) || exempt(path) {
        return true;
    }
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return matches!(site, "same-origin" | "none");
    }
    if headers.get("origin").is_some() {
        return crate::flags::same_origin(headers);
    }
    true
}

/// The middleware.
pub(crate) async fn layer(req: Request, next: Next) -> Response {
    if allowed(req.method(), req.uri().path(), req.headers()) {
        return next.run(req).await;
    }
    crate::web::html_page(
        StatusCode::FORBIDDEN,
        openlens_ui::message("Refused", "Cross-site form."),
    )
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, Method};

    fn h(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert("host", HeaderValue::from_static("lens.test"));
        for (k, v) in pairs {
            m.insert(*k, HeaderValue::from_static(v));
        }
        m
    }

    #[test]
    fn only_same_site_forms_change_things() {
        let post = Method::POST;
        assert!(super::allowed(
            &post,
            "/p/a/goals",
            &h(&[("sec-fetch-site", "same-origin")])
        ));
        assert!(!super::allowed(
            &post,
            "/p/a/goals",
            &h(&[("sec-fetch-site", "cross-site")])
        ));
        assert!(!super::allowed(
            &post,
            "/p/a/goals",
            &h(&[("sec-fetch-site", "same-site")])
        ));
        assert!(!super::allowed(
            &post,
            "/logout",
            &h(&[("origin", "https://evil.example")])
        ));
        assert!(super::allowed(
            &post,
            "/capture",
            &h(&[("sec-fetch-site", "cross-site")])
        ));
        assert!(super::allowed(
            &post,
            "/api/projects",
            &h(&[("origin", "https://evil.example")])
        ));
        assert!(super::allowed(
            &Method::GET,
            "/p/a",
            &h(&[("sec-fetch-site", "cross-site")])
        ));
    }
}

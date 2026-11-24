//! Scoped API tokens: `read`, `write` or `admin`, optionally limited to
//! some projects, so the agent that wires an app does not hold the keys of
//! the whole install. The admin token keeps every right.
//!
//! The scope is checked here, once, for every `/api/…` request. A request
//! that passes gets an internal header that [`crate::auth::has_bearer`]
//! accepts, so the handlers' own bearer check stays as it is. The header
//! is removed from every incoming request first, and its value is signed
//! with the session key, so a client cannot set it. Outside `/api/` (and
//! `/metrics`, which a `read` token may scrape) it is never added: scoped
//! tokens do not open dashboard pages.

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use crate::AppState;

/// The internal header that marks a request whose scoped token was checked.
pub(crate) const MARK: &str = "x-openlens-scope-checked";

/// What a request needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scope {
    Read,
    Write,
    Admin,
}

impl Scope {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// The scope an `/api/` request needs.
pub(crate) fn required(method: &Method, path: &str) -> Scope {
    let admin_area = [
        "/api/audit",
        "/api/login-links",
        "/api/users",
        "/api/tokens",
    ]
    .iter()
    .any(|p| path == *p || path.starts_with(&format!("{p}/")));
    let project_itself = path
        .strip_prefix("/api/projects/")
        .is_some_and(|rest| !rest.contains('/'));
    if admin_area
        || (path == "/api/projects" && *method != Method::GET)
        || (project_itself && *method != Method::GET)
    {
        return Scope::Admin;
    }
    let reads = matches!(*method, Method::GET | Method::HEAD)
        || (*method == Method::POST
            && (path.ends_with("/sql") || path == "/api/feature-map/validate"));
    if reads { Scope::Read } else { Scope::Write }
}

/// The project an `/api/projects/{id}/…` path is about.
fn project_of(path: &str) -> Option<&str> {
    path.strip_prefix("/api/projects/")?.split('/').next()
}

/// The marker value for this server.
pub(crate) fn mark(state: &AppState) -> String {
    crate::auth::sign(&state.session_key, "api-scope-checked")
}

fn denied(msg: &str) -> Response {
    crate::capture::error(StatusCode::FORBIDDEN, msg)
}

/// The middleware.
pub(crate) async fn layer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    req.headers_mut().remove(MARK);
    let path = req.uri().path().to_owned();
    if !path.starts_with("/api/") && path != "/metrics" {
        return next.run(req).await;
    }
    let Some(given) = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_owned())
        .filter(|t| t.starts_with("olt_"))
    else {
        return next.run(req).await;
    };
    let meta = state.meta.clone();
    let found = tokio::task::spawn_blocking(move || meta.api_token(&given))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let Some(token) = found else {
        // Unknown or revoked: the handler answers 401.
        return next.run(req).await;
    };
    let Some(has) = Scope::parse(&token.scope) else {
        return denied("this token has no valid scope");
    };
    let needs = required(req.method(), &path);
    if has < needs {
        return denied(&format!("this token is {:?}; this needs {:?}", has, needs));
    }
    if let (Some(list), Some(p)) = (&token.projects, project_of(&path))
        && !list.iter().any(|x| x == p)
    {
        return denied("this token is not for this project");
    }
    if let Ok(v) = HeaderValue::from_str(&mark(&state)) {
        req.headers_mut().insert(MARK, v);
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::{Scope, required};
    use axum::http::Method;

    #[test]
    fn writes_need_write_and_the_install_needs_admin() {
        assert_eq!(required(&Method::GET, "/api/projects/a/board"), Scope::Read);
        assert_eq!(required(&Method::POST, "/api/projects/a/sql"), Scope::Read);
        assert_eq!(
            required(&Method::PUT, "/api/projects/a/feature-map"),
            Scope::Write
        );
        assert_eq!(
            required(&Method::DELETE, "/api/projects/a/flags/x"),
            Scope::Write
        );
        assert_eq!(required(&Method::DELETE, "/api/projects/a"), Scope::Admin);
        assert_eq!(required(&Method::POST, "/api/projects"), Scope::Admin);
        assert_eq!(required(&Method::GET, "/api/users"), Scope::Admin);
        assert_eq!(required(&Method::POST, "/api/login-links"), Scope::Admin);
        assert_eq!(required(&Method::GET, "/api/tokens"), Scope::Admin);
        assert_eq!(required(&Method::GET, "/api/projects"), Scope::Read);
    }
}

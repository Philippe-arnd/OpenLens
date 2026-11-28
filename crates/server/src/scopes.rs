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

/// The internal header carrying a checked token's projects (comma-separated),
/// set only next to [`MARK`], for handlers that list projects.
pub(crate) const PROJECTS: &str = "x-openlens-scope-projects";

/// Calls with no project in their path that a token limited to some
/// projects may still make; every other install-wide call needs a token
/// for every project (review of v0.3.0).
fn open_to_limited(method: &Method, path: &str) -> bool {
    (*method == Method::GET && (path == "/api/projects" || path == "/api/openapi.json"))
        || (*method == Method::POST && path == "/api/feature-map/validate")
}

/// The projects a checked, project-limited token may see; `None` for any
/// other request.
pub(crate) fn limited_to(state: &AppState, headers: &axum::http::HeaderMap) -> Option<Vec<String>> {
    let marked = headers.get(MARK).and_then(|v| v.to_str().ok()) == Some(mark(state).as_str());
    let list = headers.get(PROJECTS)?.to_str().ok()?;
    marked.then(|| {
        list.split(',')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

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
        "/api/capacity",
    ]
    .iter()
    .any(|p| path == *p || path.starts_with(&format!("{p}/")));
    let project_itself = path
        .strip_prefix("/api/projects/")
        .is_some_and(|rest| !rest.contains('/'));
    // Re-labelling rewrites stored events, like changing the rules (#484);
    // the allow-list decides what is stored at all (#485).
    let relabel = *method == Method::POST
        && path
            .strip_prefix("/api/projects/")
            .and_then(|rest| rest.split_once('/'))
            .is_some_and(|(_, tail)| tail == "environments" || tail == "environments/allow");
    if admin_area
        || (path == "/api/projects" && *method != Method::GET)
        || (project_itself && *method != Method::GET)
        || relabel
    {
        return Scope::Admin;
    }
    // A board job only reads, and is its starter's alone (#515): starting
    // or cancelling one needs no more than reading the board.
    let board_job = path
        .strip_prefix("/api/projects/")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(_, tail)| tail == "board/jobs" || tail.starts_with("board/jobs/"));
    // Proposing a next action drafts a ticket and posts nothing outside
    // OpenLens (#715, D-104): an agent with a read token may.
    let propose = *method == Method::POST
        && path
            .strip_prefix("/api/projects/")
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(_, tail)| tail.strip_prefix("next-actions/"))
            .and_then(|tail| tail.strip_suffix("/propose"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'));
    let reads = matches!(*method, Method::GET | Method::HEAD)
        || (*method == Method::POST
            && (path.ends_with("/sql") || path == "/api/feature-map/validate"))
        || board_job
        || propose;
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
    req.headers_mut().remove(PROJECTS);
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
    if let Some(list) = &token.projects {
        match project_of(&path) {
            Some(p) if !list.iter().any(|x| x == p) => {
                return denied("this token is not for this project");
            }
            None if !open_to_limited(req.method(), &path) => {
                return denied(
                    "this token is limited to some projects; this needs a token for all of them",
                );
            }
            _ => {}
        }
        if let Ok(v) = HeaderValue::from_str(&list.join(",")) {
            req.headers_mut().insert(PROJECTS, v);
        }
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
        assert_eq!(
            required(&Method::POST, "/api/projects/a/board/jobs"),
            Scope::Read
        );
        assert_eq!(
            required(&Method::DELETE, "/api/projects/a/board/jobs/j1"),
            Scope::Read
        );
        assert_eq!(
            required(&Method::GET, "/api/projects/a/environments"),
            Scope::Read
        );
        assert_eq!(
            required(&Method::POST, "/api/projects/a/environments"),
            Scope::Admin
        );
        assert_eq!(
            required(&Method::POST, "/api/projects/a/environments/allow"),
            Scope::Admin
        );
        // An agent proposes with a read token; nothing else under the
        // next actions changes with it (#715).
        assert_eq!(
            required(&Method::POST, "/api/projects/a/next-actions/0a1b/propose"),
            Scope::Read
        );
        assert_eq!(
            required(&Method::POST, "/api/projects/a/next-actions/0a1b/x/propose"),
            Scope::Write
        );
    }
}

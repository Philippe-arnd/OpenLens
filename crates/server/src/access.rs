//! Typed access checks for dashboard handlers (#339), instead of path
//! rules in a middleware.
//!
//! - [`ProjectWrite`]: an admin who may see the `{project}` of the route, or
//!   the bearer token. Every handler that changes a project takes it.
//! - [`Admin`]: an admin (Team pages).
//! - [`Signed`]: anyone signed in, as their [`Viewer`] (for a handler
//!   that reads who it is); a page sends the others to the login.
//! - [`Session`]: anyone signed in to the dashboard (a page sends the
//!   others to the login); [`SessionData`], the same for a data file
//!   (JSON, timelines), which answers 401 instead.
//!
//! A handler that only needs to see the project (pages, and the `Shared`
//! posts open to read-only roles: comments, timeline notes, Ask) takes
//! neither: `access_layer` already requires a session that may see the
//! project for every `/p/*` request, whatever its method.

use std::collections::HashMap;

use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::auth::{self, Viewer};

fn page(status: StatusCode, title: &str, text: &str) -> Response {
    crate::web::html_page(status, openlens_ui::message(title, text))
}

/// The signed-in viewer, or the owner for the bearer token.
fn who(parts: &Parts, state: &AppState) -> Option<Viewer> {
    auth::viewer(state, &parts.headers).or_else(|| {
        auth::has_bearer(state, &parts.headers).then(|| Viewer {
            admin: true,
            projects: None,
            id: auth::OWNER.to_owned(),
            tz: None,
        })
    })
}

/// An admin who may see the route's `{project}` (or the bearer token).
pub(crate) struct ProjectWrite;

impl FromRequestParts<AppState> for ProjectWrite {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let Some(v) = who(parts, state) else {
            return Err(crate::web::to_login());
        };
        let Path(params) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let project = params.get("project").map_or("", String::as_str);
        if !v.can_see(project) {
            return Err(page(
                StatusCode::FORBIDDEN,
                "Not for you",
                "You do not have access to this project.",
            ));
        }
        if !v.admin {
            return Err(page(
                StatusCode::FORBIDDEN,
                "Read-only",
                "Your role can look but not change things. Ask an admin.",
            ));
        }
        Ok(Self)
    }
}

/// An admin.
pub(crate) struct Admin;

impl FromRequestParts<AppState> for Admin {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        match auth::viewer(state, &parts.headers) {
            None => Err(crate::web::to_login()),
            Some(v) if !v.admin => Err(page(
                StatusCode::FORBIDDEN,
                "Admins only",
                "Ask an admin to invite people.",
            )),
            Some(_) => Ok(Self),
        }
    }
}

/// Anyone signed in to the dashboard; others go to the login page.
pub(crate) struct Session;

impl FromRequestParts<AppState> for Session {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        if auth::has_session(state, &parts.headers) {
            Ok(Self)
        } else {
            Err(crate::web::to_login())
        }
    }
}

/// Anyone signed in, for a data file: others get 401, not a redirect.
pub(crate) struct SessionData;

impl FromRequestParts<AppState> for SessionData {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        if auth::has_session(state, &parts.headers) {
            Ok(Self)
        } else {
            Err(StatusCode::UNAUTHORIZED.into_response())
        }
    }
}

/// The signed-in viewer; others go to the login page.
pub(crate) struct Signed(pub Viewer);

impl FromRequestParts<AppState> for Signed {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        auth::viewer(state, &parts.headers)
            .map(Self)
            .ok_or_else(crate::web::to_login)
    }
}

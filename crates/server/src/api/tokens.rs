//! Scoped API tokens: list, create (shown once), revoke.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use super::{err, guard};
use crate::AppState;

#[derive(Deserialize)]
pub(super) struct NewToken {
    scope: String,
    #[serde(default)]
    projects: Option<Vec<String>>,
    #[serde(default)]
    label: String,
}

/// `GET /api/tokens`: the scoped API tokens (never their values).
pub(super) async fn list_tokens(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    match state.meta.api_tokens() {
        Ok(t) => Json(json!({ "tokens": t })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// `POST /api/tokens` `{scope, projects?, label}`: the token is shown once.
pub(super) async fn create_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewToken>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let projects = body.projects.filter(|p| !p.is_empty());
    match state
        .meta
        .create_api_token(&body.scope, projects.as_deref(), &body.label)
    {
        Ok((t, plain)) => {
            crate::privacy::audit(
                &state,
                &headers,
                None,
                "token.create",
                &format!("{} {}", t.id, t.scope),
            );
            Json(json!({ "token": plain, "info": t })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// `DELETE /api/tokens/{id}`.
pub(super) async fn revoke_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    match state.meta.revoke_api_token(&id) {
        Ok(true) => {
            crate::privacy::audit(&state, &headers, None, "token.revoke", &id);
            Json(json!({ "revoked": id })).into_response()
        }
        Ok(false) => err(StatusCode::NOT_FOUND, "no such token"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

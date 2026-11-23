//! Comments pinned to a moment of a replay (#213). Anyone who can see the
//! project may comment (team review); authors and admins delete. The body
//! is plain text, escaped when shown.

use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

/// Is this a comment path (`/p/{p}/replays/{s}/comments…`)? Viewers may
/// post there, unlike elsewhere under a project.
pub(crate) fn is_comment_path(path: &str) -> bool {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    matches!(parts.as_slice(), ["p", _, "replays", _, "comments", ..])
}

#[derive(Deserialize)]
pub(crate) struct NewComment {
    /// Playback position, milliseconds.
    at: String,
    body: String,
}

fn author(state: &AppState, v: &auth::Viewer) -> String {
    if v.id == auth::OWNER {
        return "owner".into();
    }
    state
        .meta
        .user(&v.id)
        .ok()
        .flatten()
        .map_or_else(|| v.id.clone(), |u| u.name.unwrap_or(u.email))
}

/// `POST /p/{p}/replays/{s}/comments` (form): back to that moment.
pub(crate) async fn add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, session)): Path<(String, String)>,
    Form(f): Form<NewComment>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    let Some(v) = auth::viewer(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };
    if !openlens_replay::valid_id(&session) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let at = f.at.trim().parse::<f64>().map_or(0, |x| x.max(0.0) as i64);
    let who = author(&state, &v);
    let meta = state.meta.clone();
    let (p, s) = (project.clone(), session.clone());
    let res = tokio::task::spawn_blocking(move || {
        meta.add_replay_comment(&p, &s, at, &v.id, &who, &f.body)
    })
    .await;
    match res {
        Ok(Ok(c)) => Redirect::to(&format!(
            "/p/{project}/replays/{session}?t={}#c{}",
            c.at_ms, c.id
        ))
        .into_response(),
        Ok(Err(e)) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't add that", &e.to_string()),
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /p/{p}/replays/{s}/comments/{id}/delete`: author or admin.
pub(crate) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, session, id)): Path<(String, String, i64)>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    let Some(v) = auth::viewer(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };
    let meta = state.meta.clone();
    let p = project.clone();
    let ok =
        tokio::task::spawn_blocking(move || meta.delete_replay_comment(&p, id, &v.id, v.admin))
            .await;
    match ok {
        Ok(Ok(true)) => Redirect::to(&format!("/p/{project}/replays/{session}")).into_response(),
        _ => html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message(
                "Not yours",
                "Only its author or an admin can delete a comment.",
            ),
        ),
    }
}

/// `GET /api/projects/{p}/replays/{s}/comments` (bearer).
pub(crate) async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, session)): Path<(String, String)>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "admin bearer token required"})),
        )
            .into_response();
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.replay_comments(&project, &session)).await {
        Ok(Ok(c)) => Json(json!({ "comments": c })).into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct ApiComment {
    /// Seconds into the recording.
    at: f64,
    body: String,
}

/// `POST /api/projects/{p}/replays/{s}/comments` with `{at, body}` (bearer;
/// the author is the owner).
pub(crate) async fn api_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, session)): Path<(String, String)>,
    Json(b): Json<ApiComment>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "admin bearer token required"})),
        )
            .into_response();
    }
    if !openlens_replay::valid_id(&session) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid session id"})),
        )
            .into_response();
    }
    let meta = state.meta.clone();
    let at = (b.at.max(0.0) * 1000.0) as i64;
    match tokio::task::spawn_blocking(move || {
        meta.add_replay_comment(&project, &session, at, auth::OWNER, "owner", &b.body)
    })
    .await
    {
        Ok(Ok(c)) => (StatusCode::CREATED, Json(json!({ "comment": c }))).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

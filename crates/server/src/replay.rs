//! Session recordings.
//!
//! - Public (capture key): `POST /r` stores one chunk of a recording.
//! - Admin (bearer): `GET /api/projects/{id}/replays[?distinct_id=]` lists
//!   recordings, `GET /api/projects/{id}/replays/{session}` returns one.

use std::collections::HashMap;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;

use crate::AppState;
use crate::auth;
use crate::capture::{error, inflate, project_for_key};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/replays", get(list))
        .route("/api/projects/{project}/replays/{session}", get(one))
}

#[derive(Deserialize)]
struct Chunk {
    key: Option<String>,
    session_id: String,
    distinct_id: String,
    seq: u32,
    events: Box<RawValue>,
}

/// `POST /r`: one chunk `{key?, session_id, distinct_id, seq, events: [...]}`.
pub(crate) async fn ingest(
    State(state): State<AppState>,
    peer: crate::capture::Peer,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if crate::capture::ip_limited(&peer, &headers) {
        return crate::capture::too_many();
    }
    if let Some(r) = crate::capture::disk_full(&state) {
        return r;
    }
    let raw = match inflate(&headers, &body) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    let chunk: Chunk = match serde_json::from_slice(&raw) {
        Ok(c) => c,
        Err(_) => return error(StatusCode::BAD_REQUEST, "body is not a valid replay chunk"),
    };
    let key = headers
        .get("x-openlens-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| query.get("key").cloned())
        .or_else(|| chunk.key.clone());
    let project = match project_for_key(&state, key).await {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let root = state.data_dir.clone();
    let res = tokio::task::spawn_blocking(move || {
        openlens_replay::append(
            &root,
            &project.id,
            &chunk.session_id,
            &chunk.distinct_id,
            chunk.seq,
            &chunk.events,
            chrono::Utc::now(),
        )
    })
    .await;
    use openlens_replay::Error as E;
    match res {
        Ok(Ok(bytes)) => Json(json!({ "stored": bytes })).into_response(),
        Ok(Err(e @ (E::InvalidId | E::InvalidSeq | E::InvalidChunk))) => {
            error(StatusCode::BAD_REQUEST, &e.to_string())
        }
        Ok(Err(e @ E::TooLarge)) => error(StatusCode::PAYLOAD_TOO_LARGE, &e.to_string()),
        Ok(Err(err)) => {
            tracing::error!(%err, "cannot store replay chunk");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "cannot store recording right now",
            )
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "storage task failed"),
    }
}

fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    (!auth::has_bearer(state, headers))
        .then(|| error(StatusCode::UNAUTHORIZED, "admin bearer token required"))
}

async fn list(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let root = state.data_dir.clone();
    let who = q.get("distinct_id").cloned();
    match tokio::task::spawn_blocking(move || {
        openlens_replay::list(&root, &project, who.as_deref())
    })
    .await
    {
        Ok(Ok(l)) => Json(json!({ "replays": l })).into_response(),
        Ok(Err(openlens_replay::Error::InvalidId)) => {
            error(StatusCode::NOT_FOUND, "unknown project")
        }
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "cannot list recordings"),
    }
}

/// The recording as `{"session_id", "distinct_id", "started", "events": [...]}`.
pub(crate) async fn load(
    state: &AppState,
    project: String,
    session: String,
) -> Result<Option<(openlens_replay::Info, String)>, openlens_replay::Error> {
    let root = state.data_dir.clone();
    tokio::task::spawn_blocking(move || openlens_replay::load(&root, &project, &session))
        .await
        .map_err(|e| openlens_replay::Error::Io(std::io::Error::other(e.to_string())))?
}

async fn one(
    State(state): State<AppState>,
    Path((project, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    match load(&state, project, session.clone()).await {
        Ok(Some((info, events))) => {
            let head = json!({
                "session_id": session,
                "distinct_id": info.distinct_id,
                "started": info.started,
            })
            .to_string();
            // Splice the stored events in without re-parsing them.
            let body = format!(
                "{},\"events\":{events}}}",
                head.strip_suffix('}').unwrap_or(&head)
            );
            ([("content-type", "application/json")], body).into_response()
        }
        Ok(None) | Err(openlens_replay::Error::InvalidId) => {
            error(StatusCode::NOT_FOUND, "no such recording")
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "cannot read recording"),
    }
}

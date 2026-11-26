//! Session recordings.
//!
//! - Public (capture key): `POST /r` stores one chunk of a recording.
//! - Admin (bearer): `GET /api/projects/{id}/replays[?distinct_id=]` lists
//!   recordings, `GET /api/projects/{id}/replays/{session}` returns one.
//! - The replay rate (#327, D-035), handed to the SDK by `GET /flags` and
//!   shown on the replay page: counted at most once a day per project.

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
    if crate::capture::ip_limited(&state, &peer, &headers) {
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
    // Forgotten people stay forgotten: their recordings are dropped too
    // (#230, review of v0.3.0).
    if state
        .meta
        .is_forgotten(&project.id, &chunk.distinct_id)
        .unwrap_or(false)
    {
        return StatusCode::OK.into_response();
    }
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

/// A project's cached replay rate: the local day it is for, and while
/// the count failed, when to try again.
#[derive(Clone)]
pub(crate) struct RateEntry {
    day: chrono::NaiveDate,
    rate: openlens_engine::ReplayRate,
    retry: Option<std::time::Instant>,
}

/// How long a failed count waits before the next try.
const RATE_RETRY: std::time::Duration = std::time::Duration::from_secs(600);

/// The replay rate of `project` (time zone `tz`). The first call of the
/// project's day counts yesterday's active users (one query); calls made
/// meanwhile, and the rest of the day, get the cached value. Before the
/// first count, and when it fails, the previous day's rate is kept (every
/// session when there is none).
pub(crate) async fn rate(state: &AppState, project: &str, tz: &str) -> openlens_engine::ReplayRate {
    let today = openlens_engine::replay_rate::local_today(tz);
    let now = std::time::Instant::now();
    let (current, count) = state
        .caches
        .replay_rates
        .update(project.to_owned(), |old| match old {
            Some(e) if e.day == today && e.retry.is_none_or(|t| now < t) => {
                let r = e.rate.clone();
                (Some(e), (r, false))
            }
            old => {
                let r = old.map_or_else(openlens_engine::ReplayRate::everything, |e| e.rate);
                // Mark the count as started: no second one meanwhile.
                let pending = RateEntry {
                    day: today,
                    rate: r.clone(),
                    retry: Some(now + RATE_RETRY),
                };
                (Some(pending), (r, true))
            }
        })
        .unwrap_or_else(|| (openlens_engine::ReplayRate::everything(), false));
    if !count {
        return current;
    }
    match openlens_engine::replay_rate(&state.query, project, tz).await {
        Ok(r) => {
            state.caches.replay_rates.insert(
                project.to_owned(),
                RateEntry {
                    day: today,
                    rate: r.clone(),
                    retry: None,
                },
            );
            r
        }
        Err(err) => {
            tracing::warn!(%err, project, "cannot count yesterday's active users for the replay rate");
            current
        }
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
    // The same order and filter as the dashboard list (#404).
    let view = openlens_replay::View::from_query(
        q.get("sort").map(String::as_str),
        q.get("active").map(String::as_str),
    );
    let pid = project.clone();
    let listed = tokio::task::spawn_blocking(move || {
        openlens_replay::list_view(&root, &pid, who.as_deref(), view)
    })
    .await;
    match listed {
        Ok(Ok(l)) => {
            // The rate and why, as on the replay page (#327).
            let tz = state
                .meta
                .project(&project)
                .ok()
                .flatten()
                .map_or_else(|| "UTC".to_owned(), |p| p.timezone);
            let rate = rate(&state, &project, &tz).await;
            Json(json!({ "replays": l, "replay_rate": rate })).into_response()
        }
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

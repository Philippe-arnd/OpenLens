//! JSON API for agents, the CLI and the MCP server. Every route needs the
//! admin bearer token.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use openlens_engine::Filters;
use serde::Deserialize;
use serde_json::json;

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{project}/sql", post(sql))
        .route("/api/projects/{project}/board", get(board))
        .route("/api/login-links", post(login_link))
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if auth::has_bearer(state, headers) {
        None
    } else {
        Some(err(StatusCode::UNAUTHORIZED, "admin bearer token required"))
    }
}

async fn known_project(
    state: &AppState,
    id: &str,
) -> Result<openlens_meta::Project, Box<Response>> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    match tokio::task::spawn_blocking(move || meta.project(&id)).await {
        Ok(Ok(Some(p))) => Ok(p),
        Ok(Ok(None)) => Err(Box::new(err(StatusCode::NOT_FOUND, "unknown project"))),
        _ => Err(Box::new(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ))),
    }
}

async fn list_projects(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.projects()).await {
        Ok(Ok(ps)) => Json(json!({
            "projects": ps.iter().map(|p| json!({"id": p.id, "name": p.name, "key": p.api_key})).collect::<Vec<_>>()
        }))
        .into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "metadata store unavailable"),
    }
}

#[derive(Deserialize)]
struct NewProject {
    name: String,
}

async fn create_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewProject>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    if body.name.trim().is_empty() || body.name.len() > 80 {
        return err(StatusCode::BAD_REQUEST, "name must be 1-80 characters");
    }
    let meta = state.meta.clone();
    let host = crate::base_url(&headers);
    match tokio::task::spawn_blocking(move || meta.create_project(body.name.trim(), None)).await {
        Ok(Ok(p)) => (
            StatusCode::CREATED,
            Json(json!({
                "id": p.id, "name": p.name, "key": p.api_key,
                "snippet": format!(r#"<script src="{host}/c.js" data-key="{}" defer></script>"#, p.api_key),
                "guide": format!("{host}/agents.md"),
            })),
        )
            .into_response(),
        Ok(Err(e)) => err(StatusCode::CONFLICT, &e.to_string()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "metadata store unavailable"),
    }
}

#[derive(Deserialize)]
struct SqlBody {
    sql: String,
}

async fn sql(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Json(body): Json<SqlBody>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    match state.query.sql(&p.id, &body.sql).await {
        Ok(res) => Json(res).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct FilterQuery {
    pub(crate) env: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) release: Option<String>,
    pub(crate) event: Option<String>,
}

impl FilterQuery {
    pub(crate) fn filters(&self) -> Filters {
        let some = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Filters {
            environment: some(&self.env).unwrap_or_else(|| "production".into()),
            role: some(&self.role),
            release: some(&self.release),
        }
    }
}

async fn board(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<FilterQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    match openlens_engine::board(&state.query, &p.id, &q.filters()).await {
        Ok(b) => Json(b).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn login_link(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    let host = crate::base_url(&headers);
    match tokio::task::spawn_blocking(move || meta.create_login_link(7 * 24 * 3600)).await {
        Ok(Ok(t)) => {
            Json(json!({ "url": format!("{host}/l/{t}"), "expires_in_days": 7 })).into_response()
        }
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

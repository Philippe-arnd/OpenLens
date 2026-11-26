//! Uptime history over the REST API (#559): the days of a range
//! (`uptime.days`) and the incidents page by page (`uptime.incidents`),
//! for the CLI and the MCP tools (D-020). Both need the `read` scope.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use openlens_engine::commands::uptime::{DaysInput, GetDays, GetIncidents, IncidentsInput};
use openlens_engine::commands::{Caller, Command, Error};
use serde::Deserialize;

use super::{err, guard, known_project};
use crate::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/uptime/days", get(days))
        .route("/api/projects/{project}/incidents", get(incidents))
}

fn answer<T: serde::Serialize>(r: Result<T, Error>) -> Response {
    match r {
        Ok(v) => axum::Json(v).into_response(),
        Err(Error::NotFound) => err(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => err(StatusCode::UNPROCESSABLE_ENTITY, &m),
        Err(Error::Unavailable(m)) => err(StatusCode::BAD_REQUEST, &m),
        Err(Error::Storage(m)) => err(StatusCode::INTERNAL_SERVER_ERROR, &m),
    }
}

/// The query of `GET …/uptime/days`.
#[derive(Debug, Deserialize)]
struct DaysQuery {
    from: Option<String>,
    to: Option<String>,
    env: Option<String>,
}

/// `GET /api/projects/{id}/uptime/days?from=&to=&env=`: `uptime.days`
/// (UTC days, at most 400; default the last 90 days of production).
async fn days(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DaysQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = DaysInput {
        project: p.id,
        from: q.from,
        to: q.to,
        environment: q.env,
    };
    answer(GetDays::run(&state, &Caller::default(), input).await)
}

/// The query of `GET …/incidents`.
#[derive(Debug, Deserialize)]
struct IncidentsQuery {
    before: Option<i64>,
    limit: Option<u32>,
}

/// `GET /api/projects/{id}/incidents?before=&limit=`: `uptime.incidents`.
async fn incidents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<IncidentsQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = IncidentsInput {
        project: p.id,
        before: q.before,
        limit: q.limit,
    };
    answer(GetIncidents::run(&state, &Caller::default(), input).await)
}

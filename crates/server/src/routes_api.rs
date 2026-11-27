//! Server routes (#594, D-083): `GET /api/projects/{id}/routes`, the
//! engine's `routes.list` as JSON. The Errors page shows the same command
//! as a table. Runtime health per release (#610, D-087):
//! `GET /api/projects/{id}/runtime`, the engine's `runtime.list`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::commands::routes::{List, ListInput};
use openlens_engine::commands::runtime;
use openlens_engine::commands::{Caller, Command, Error};
use serde::Deserialize;

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/routes", get(api))
        .route("/api/projects/{project}/runtime", get(runtime_api))
}

#[derive(Deserialize, Default)]
pub(crate) struct RoutesQuery {
    /// Environment (production when absent).
    #[serde(default)]
    env: Option<String>,
    /// Days of the period (1 to 90, default 7).
    #[serde(default)]
    days: Option<i64>,
}

/// `GET /api/projects/{id}/routes[?env=][&days=]` (bearer): `routes.list`.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<RoutesQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = ListInput {
        project: id,
        environment: q.env,
        days: q.days,
    };
    match List::run(&state, &Caller::default(), input).await {
        Ok(d) => Json(d).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct RuntimeQuery {
    /// Environment (production when absent).
    #[serde(default)]
    env: Option<String>,
}

/// `GET /api/projects/{id}/runtime[?env=]` (bearer): `runtime.list`.
pub(crate) async fn runtime_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<RuntimeQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = runtime::ListInput {
        project: id,
        environment: q.env,
    };
    match runtime::List::run(&state, &Caller::default(), input).await {
        Ok(d) => Json(d).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

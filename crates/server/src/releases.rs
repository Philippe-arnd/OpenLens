//! Releases (#500, D-049): the Releases page and `GET
//! /api/projects/{id}/releases`. Both run the engine's `releases.list`
//! command; these handlers adapt it to HTML and JSON.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::commands::releases::{List, ListInput};
use openlens_engine::commands::{Caller, Command, Error, Host};
use serde::Deserialize;

use crate::web::html_page;
use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/p/{project}/releases", get(page))
        .route("/api/projects/{project}/releases", get(api))
}

#[derive(Deserialize, Default)]
pub(crate) struct EnvQuery {
    /// One environment only.
    #[serde(default)]
    env: Option<String>,
}

/// `GET /p/{id}/releases`.
pub(crate) async fn page(
    _: crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<EnvQuery>,
) -> Response {
    let Some(p) = state.project(&id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let caller = crate::commands::caller(&state, &headers);
    let input = ListInput {
        project: p.id.clone(),
        environment: q.env,
    };
    match List::run(&state, &caller, input).await {
        Ok(r) => {
            let info = openlens_ui::ProjectInfo {
                id: &p.id,
                name: &p.name,
                key: &p.api_key,
            };
            html_page(StatusCode::OK, openlens_ui::releases(&info, &r))
        }
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

/// `GET /api/projects/{id}/releases[?env=]` (bearer): `releases.list`.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<EnvQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = ListInput {
        project: id,
        environment: q.env,
    };
    match List::run(&state, &Caller::default(), input).await {
        Ok(r) => Json(r).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

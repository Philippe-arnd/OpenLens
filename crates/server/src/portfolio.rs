//! The portfolio (#717, D-118): `GET /portfolio`, the page, and
//! `GET /api/portfolio`, both the engine's `portfolio.get` for the
//! projects the caller may see. Neither computes a board: each row reads
//! the board the cache keeps, or says it is computing.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::commands::Command;
use openlens_engine::commands::portfolio::{Get, GetInput};

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/portfolio", get(page))
        .route("/api/portfolio", get(api))
}

/// `GET /portfolio`: anyone signed in, their projects only.
async fn page(
    _: crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let caller = crate::commands::caller(&state, &headers);
    match Get::run(&state, &caller, GetInput::default()).await {
        Ok(p) => crate::web::html_page(StatusCode::OK, openlens_ui::portfolio(&p)),
        Err(e) => crate::web::html_page(
            StatusCode::SERVICE_UNAVAILABLE,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

/// `GET /api/portfolio`: the admin token sees every project, a token
/// limited to some projects sees those.
async fn api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let caller = crate::commands::caller(&state, &headers);
    match Get::run(&state, &caller, GetInput::default()).await {
        Ok(p) => Json(p).into_response(),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

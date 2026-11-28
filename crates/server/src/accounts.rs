//! Account health (#719, D-117): `GET /api/projects/{id}/accounts`, the
//! engine's `accounts.list` as JSON. People › Audience shows the same
//! command, when the app sends `account`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::commands::accounts::{List, ListInput};
use openlens_engine::commands::{Caller, Command, Error};
use serde::Deserialize;

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/projects/{project}/accounts", get(api))
}

#[derive(Deserialize, Default)]
pub(crate) struct AccountsQuery {
    /// `healthy`, `at_risk` or `inactive`.
    #[serde(default)]
    state: Option<String>,
    /// Accounts listed (1 to 500, default 50).
    #[serde(default)]
    limit: Option<usize>,
    /// Environment (production when absent).
    #[serde(default)]
    env: Option<String>,
}

/// `GET /api/projects/{id}/accounts[?state=][&limit=][&env=]` (bearer):
/// `accounts.list`.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<AccountsQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = ListInput {
        project: id,
        state: q.state,
        limit: q.limit,
        environment: q.env,
    };
    match List::run(&state, &Caller::default(), input).await {
        Ok(d) => Json(d).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// The accounts of `env` for People › Audience; none when it cannot be
/// read.
pub(crate) async fn of_project(
    state: &AppState,
    project: &str,
    env: &str,
) -> Option<openlens_engine::accounts::Accounts> {
    let input = ListInput {
        project: project.to_owned(),
        environment: Some(env.to_owned()),
        ..ListInput::default()
    };
    List::run(state, &Caller::default(), input).await.ok()
}

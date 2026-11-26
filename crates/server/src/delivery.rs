//! Delivery metrics (#523, D-068): `GET /api/projects/{id}/delivery`, the
//! engine's `releases.delivery` as JSON. The Releases page shows the same
//! command as a card.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::commands::delivery::{DeliveryInput, GetDelivery};
use openlens_engine::commands::{Caller, Command, Error};
use serde::Deserialize;

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/projects/{project}/delivery", get(api))
}

#[derive(Deserialize, Default)]
pub(crate) struct DeliveryQuery {
    /// One environment only.
    #[serde(default)]
    env: Option<String>,
    /// Days of the period (1 to 90).
    #[serde(default)]
    days: Option<i64>,
}

/// `GET /api/projects/{id}/delivery[?env=][&days=]` (bearer):
/// `releases.delivery`.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DeliveryQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = DeliveryInput {
        project: id,
        environment: q.env,
        days: q.days,
    };
    match GetDelivery::run(&state, &Caller::default(), input).await {
        Ok(d) => Json(d).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

//! Server routes (#594, D-083): `GET /api/projects/{id}/routes`, the
//! engine's `routes.list` as JSON, and the Quality › Routes page that
//! shows the same command (#622).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::commands::routes::{List, ListInput};
use openlens_engine::commands::{Caller, Command, Error, Host};
use serde::Deserialize;

use crate::api::FilterQuery;
use crate::web::html_page;
use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/routes", get(api))
        .route("/p/{project}/routes", get(page))
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

/// `GET /p/{id}/routes`: Quality › Routes, `routes.list` over the trend's
/// ten days in the context bar's environment.
pub(crate) async fn page(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    let Some(p) = state.project(&id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let input = ListInput {
        project: p.id.clone(),
        environment: Some(f.environment.clone()),
        days: Some(openlens_engine::routes::TREND_DAYS),
    };
    let caller = Caller::default();
    let (b, routes) = tokio::join!(
        openlens_engine::filter_options(&state.query, &p.id, &f, map.as_ref().map(|(m, _)| m)),
        List::run(&state, &caller, input),
    );
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match (b, routes) {
        (Ok(mut b), Ok(r)) => {
            b.environments = crate::environments::visible(
                &state,
                &p.id,
                std::mem::take(&mut b.environments),
                &f.environment,
            );
            html_page(StatusCode::OK, openlens_ui::routes_page(&info, &b, &f, &r))
        }
        (Err(e), _) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
        (_, Err(e)) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

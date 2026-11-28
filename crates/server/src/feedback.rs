//! What users say (#718, D-116): `GET /api/projects/{id}/feedback`, the
//! engine's `feedback.list` as JSON with the texts (the bearer token is an
//! automation token); the feature page and the board's insight cards read
//! the same command, the texts for admins only; and the hourly pass that
//! removes the texts older than 90 days from the files.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::commands::feedback::{List, ListInput};
use openlens_engine::commands::{Caller, Command, Error};
use openlens_engine::feedback::FeedbackReport;
use serde::Deserialize;

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/projects/{project}/feedback", get(api))
}

#[derive(Deserialize, Default)]
pub(crate) struct FeedbackQuery {
    /// One feature key.
    #[serde(default)]
    feature: Option<String>,
    /// Days of the period (1 to 90, default 30).
    #[serde(default)]
    days: Option<i64>,
    /// Environment (production when absent).
    #[serde(default)]
    env: Option<String>,
    /// Texts listed (1 to 200, default 20).
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /api/projects/{id}/feedback[?feature=][&days=][&env=][&limit=]`
/// (bearer): `feedback.list`, texts included.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FeedbackQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = ListInput {
        project: id,
        feature: q.feature,
        days: q.days,
        environment: q.env,
        limit: q.limit,
    };
    let caller = Caller {
        detail: true,
        ..Caller::default()
    };
    match List::run(&state, &caller, input).await {
        Ok(d) => Json(d).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// One feature's feedback of the last 30 days in `env`, for its page: the
/// texts when the request is an admin's. None when it cannot be read.
pub(crate) async fn of_feature(
    state: &AppState,
    headers: &HeaderMap,
    project: &str,
    feature: &str,
    env: &str,
) -> Option<FeedbackReport> {
    let input = ListInput {
        project: project.to_owned(),
        feature: Some(feature.to_owned()),
        environment: Some(env.to_owned()),
        ..ListInput::default()
    };
    List::run(state, &crate::commands::caller(state, headers), input)
        .await
        .ok()
}

/// Feedbacks of the last 7 days per feature key in `f`'s environment, for
/// the board's insight cards; empty when there is none or it cannot be
/// read (counts only, never a text).
pub(crate) async fn week(
    state: &AppState,
    project: &str,
    f: &openlens_engine::Filters,
) -> Vec<(String, i64)> {
    let f = openlens_engine::Filters {
        role: None,
        release: None,
        window: openlens_engine::Window::Fixed,
        ..f.clone()
    };
    openlens_engine::feedback::feedback_week(&state.query, project, &f)
        .await
        .unwrap_or_default()
}

/// Remove the texts of feedbacks older than 90 days (the caller holds the
/// maintenance lock).
pub(crate) fn trim(state: &AppState, project: &str, now: chrono::DateTime<chrono::Utc>) {
    match openlens_storage::trim_feedback(&state.data_dir, project, now) {
        Ok(0) => {}
        Ok(n) => {
            state.boards.closed().project_changed(project);
            tracing::info!(project, feedback = n, "removed old feedback texts");
        }
        Err(err) => tracing::warn!(project, %err, "feedback trimming failed"),
    }
}

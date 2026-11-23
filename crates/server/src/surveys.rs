//! Micro-surveys (D-016).
//!
//! - Admin (bearer): `GET /api/projects/{id}/surveys` (definitions and
//!   results), `PUT|DELETE /api/projects/{id}/surveys/{key}`.
//! - Public: eligible surveys ride along with `GET /flags` (see `flags.rs`).
//! - Dashboard: `/p/{id}/surveys` (results), `POST /p/{id}/surveys/{key}`
//!   (admins: switch on/off, sample).

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use openlens_flags::survey::Survey;
use serde::Deserialize;
use serde_json::json;

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/surveys", get(admin_list))
        .route(
            "/api/projects/{project}/surveys/{key}",
            put(admin_put).delete(admin_delete),
        )
        .route("/p/{project}/surveys", get(page))
        .route("/p/{project}/surveys/{key}", post(page_update))
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// All surveys of a project (invalid stored rows are skipped).
pub(crate) async fn load(state: &AppState, project: &str) -> Vec<Survey> {
    let meta = state.meta.clone();
    let p = project.to_owned();
    match tokio::task::spawn_blocking(move || meta.surveys(&p)).await {
        Ok(Ok(rows)) => rows
            .iter()
            .filter_map(|j| serde_json::from_str(j).ok())
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) async fn save(state: &AppState, project: &str, s: &Survey) -> Result<(), String> {
    s.validate()?;
    let json = serde_json::to_string(s).map_err(|e| e.to_string())?;
    let meta = state.meta.clone();
    let (p, k) = (project.to_owned(), s.key.clone());
    tokio::task::spawn_blocking(move || meta.save_survey(&p, &k, &json))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

async fn project(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

pub(crate) async fn results(
    state: &AppState,
    project: &str,
) -> Vec<openlens_engine::SurveyResults> {
    let map = crate::load_map(state, project).await;
    let src = openlens_engine::source(map.as_ref().map(|(m, _)| m));
    openlens_engine::survey_results(
        &state.query,
        project,
        &src,
        &openlens_engine::Filters::default(),
    )
    .await
    .unwrap_or_default()
}

async fn admin_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    Json(json!({ "surveys": load(&state, &id).await, "results": results(&state, &id).await }))
        .into_response()
}

async fn admin_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Json(mut s): Json<Survey>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    if project(&state, &id).await.is_none() {
        return err(StatusCode::NOT_FOUND, "unknown project");
    }
    s.key = key;
    match save(&state, &id, &s).await {
        Ok(()) => Json(json!({ "survey": s })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

async fn admin_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.delete_survey(&id, &key)).await {
        Ok(Ok(true)) => Json(json!({ "deleted": true })).into_response(),
        Ok(Ok(false)) => err(StatusCode::NOT_FOUND, "no such survey"),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let Some(p) = project(&state, &id).await else {
        return crate::web::html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let (surveys, res) = (load(&state, &p.id).await, results(&state, &p.id).await);
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    crate::web::html_page(StatusCode::OK, openlens_ui::surveys(&info, &surveys, &res))
}

#[derive(Deserialize)]
struct SurveyForm {
    enabled: Option<String>,
    sample: Option<u8>,
}

async fn page_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Form(form): Form<SurveyForm>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    if !crate::flags::same_origin(&headers) {
        return err(StatusCode::FORBIDDEN, "cross-site request refused");
    }
    let mut all = load(&state, &id).await;
    let Some(s) = all.iter_mut().find(|s| s.key == key) else {
        return err(StatusCode::NOT_FOUND, "no such survey");
    };
    s.enabled = form.enabled.as_deref() == Some("on");
    if let Some(n) = form.sample {
        s.sample = n.min(100);
    }
    let s = s.clone();
    match save(&state, &id, &s).await {
        Ok(()) => Redirect::to(&format!("/p/{id}/surveys")).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

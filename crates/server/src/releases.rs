//! Releases (#500, D-049): the Releases page and `GET
//! /api/projects/{id}/releases`. Both run the engine's `releases.list`
//! command; these handlers adapt it to HTML and JSON.
//!
//! The deploy hook's URL (#504, D-053) is made and rotated here too, on
//! the page (`POST /p/{id}/releases/hook`) and the API (`POST
//! /api/projects/{id}/deploy-hook`): the engine's `deploys.rotate_hook`.
//! The URL is shown once, in the response, never in a redirect.
//!
//! A deploy step records its release with its commit range (#521, D-066)
//! through `POST /api/projects/{id}/releases`: the engine's
//! `releases.record`.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use openlens_engine::commands::delivery::{DeliveryInput, with_releases};
use openlens_engine::commands::deploys::{
    ProjectInput, RecordRelease, RecordReleaseInput, RotateHook,
};
use openlens_engine::commands::releases::{List, ListInput, ReleaseReport, ReportInput};
use openlens_engine::commands::{Caller, Command, Error, Host};
use serde::Deserialize;

use crate::web::html_page;
use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/p/{project}/releases", get(page))
        .route(
            "/api/projects/{project}/releases",
            get(api).post(record_api),
        )
        .route(
            "/api/projects/{project}/releases/{release}/report",
            get(report_api),
        )
        .route("/p/{project}/releases/hook", post(rotate_page))
        .route("/api/projects/{project}/deploy-hook", post(rotate_api))
}

#[derive(Deserialize, Default)]
pub(crate) struct EnvQuery {
    /// One environment only.
    #[serde(default)]
    env: Option<String>,
}

/// `GET /p/{id}/releases`.
pub(crate) async fn page(
    crate::access::Signed(viewer): crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<EnvQuery>,
) -> Response {
    render(&state, &headers, &id, q.env, viewer.admin, None).await
}

/// The Releases page; `new_url` is a deploy hook URL just made, shown once.
async fn render(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    env: Option<String>,
    admin: bool,
    new_url: Option<&str>,
) -> Response {
    let Some(p) = state.project(id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let caller = crate::commands::caller(state, headers);
    let input = ListInput {
        project: p.id.clone(),
        environment: env.clone(),
    };
    // The delivery card (#523), read from the same list so the verdicts
    // that fail its changes (#611) are read once: left out when it cannot
    // be computed.
    let delivery = DeliveryInput {
        project: p.id.clone(),
        environment: env,
        days: None,
    };
    match List::run(state, &caller, input).await {
        Ok(r) => {
            let delivery = with_releases(state, &caller, delivery, &r).await.ok();
            let info = openlens_ui::ProjectInfo {
                id: &p.id,
                name: &p.name,
                key: &p.api_key,
            };
            let hook = openlens_ui::HookView { admin, new_url };
            html_page(
                StatusCode::OK,
                openlens_ui::releases(&info, &r, delivery.as_ref(), &hook),
            )
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

/// `GET /api/projects/{id}/releases/{release}/report[?env=]` (bearer):
/// `releases.report`, the release compared with the one before it in the
/// environment (production by default), with its verdict (#522, D-067).
pub(crate) async fn report_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, release)): Path<(String, String)>,
    Query(q): Query<EnvQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = ReportInput {
        project: id,
        release,
        environment: q.env,
    };
    match ReleaseReport::run(&state, &Caller::default(), input).await {
        Ok(r) => Json(r).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct RotateForm {
    #[serde(default)]
    confirm: String,
}

/// `POST /p/{id}/releases/hook`: make the deploy hook URL, or rotate it
/// (the box must be ticked: the old URL stops working at once). The page
/// comes back with the new URL, shown once.
pub(crate) async fn rotate_page(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<RotateForm>,
) -> Response {
    let had = state.meta.deploy_hook_since(&id).ok().flatten().is_some();
    if had && f.confirm != "yes" {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message(
                "Can't do that",
                "Tick the box: the old URL stops working at once.",
            ),
        );
    }
    match RotateHook::run(
        &state,
        &Caller::default(),
        ProjectInput {
            project: id.clone(),
        },
    )
    .await
    {
        Ok(s) => {
            crate::privacy::audit(&state, &headers, Some(&id), "deploys.rotate_hook", "");
            let url = crate::hooks::url(&headers, &s.secret);
            render(&state, &headers, &id, None, true, Some(&url)).await
        }
        Err(Error::NotFound) => html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        ),
        Err(e) => html_page(
            StatusCode::SERVICE_UNAVAILABLE,
            openlens_ui::message("Can't do that", &e.to_string()),
        ),
    }
}

/// `POST /api/projects/{id}/deploy-hook` (bearer, write): a new deploy
/// hook URL; the old one stops working at once. The URL is in this
/// response only.
pub(crate) async fn rotate_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    match RotateHook::run(
        &state,
        &Caller::default(),
        ProjectInput {
            project: id.clone(),
        },
    )
    .await
    {
        Ok(s) => {
            crate::privacy::audit(&state, &headers, Some(&id), "deploys.rotate_hook", "");
            Json(serde_json::json!({
                "url": crate::hooks::url(&headers, &s.secret),
                "note": "Shown once. Paste it in the platform's deploy notifications; the previous URL no longer works.",
            }))
            .into_response()
        }
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `POST /api/projects/{id}/releases` (bearer, write): record a release
/// named by the deploy step, `{release, commit?, from?, environment?,
/// first_commit_at?, dry_run?}`,
/// as a succeeded deploy with its commit range (`releases.record`).
pub(crate) async fn record_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<RecordReleaseInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let Ok(Json(mut input)) = body else {
        return crate::apierr::json(
            StatusCode::BAD_REQUEST,
            "send JSON: {release, commit?, from?, environment?, first_commit_at?, dry_run?}",
        );
    };
    input.project = id;
    match RecordRelease::run(&state, &Caller::default(), input).await {
        Ok(r) => Json(r).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(Error::Invalid(m)) => crate::apierr::json(StatusCode::BAD_REQUEST, m),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

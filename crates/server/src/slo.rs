//! Availability over long windows and the SLO (#560, D-078): the card of
//! the Uptime page with its target form, the API routes, and the status
//! route that carries production's figures. All of them run the engine's
//! `uptime.report` and `uptime.set_target`; every target change is
//! written to the audit log.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use maud::Markup;
use openlens_engine::commands::uptime::{GetReport, ReportInput, SetTarget, Target, TargetInput};
use openlens_engine::commands::{Caller, Command, Error, alerts};
use openlens_meta::Project;
use openlens_meta::uptime_history::{PRODUCTION, RAW_DAYS};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

/// Most environments the Uptime page shows figures for.
const MAX_SHOWN: usize = 10;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/uptime/report", get(api_report))
        .route("/api/projects/{project}/uptime/target", put(api_target))
        .route("/p/{project}/uptime/target", post(page_target))
}

fn failed(e: Error) -> Response {
    let (status, m) = match e {
        Error::NotFound => (StatusCode::NOT_FOUND, "unknown project".to_owned()),
        Error::Invalid(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
        Error::Unavailable(m) => (StatusCode::BAD_REQUEST, m),
        Error::Storage(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
    };
    crate::apierr::json(status, m)
}

fn audit(state: &AppState, headers: &HeaderMap, project: &str, t: &Target) {
    let detail = match t.target {
        Some(v) => format!("{}: {:.3}%", t.environment, v * 100.0),
        None => format!("{}: removed", t.environment),
    };
    crate::privacy::audit(state, headers, Some(project), "uptime.set_target", &detail);
}

async fn api_open(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
) -> Result<String, Box<Response>> {
    if let Some(r) = crate::api::guard(state, headers) {
        return Err(Box::new(r));
    }
    crate::api::known_project(state, id).await.map(|p| p.id)
}

/// The query of `GET …/uptime/report`.
#[derive(Debug, Deserialize)]
struct ReportQuery {
    env: Option<String>,
}

/// `GET /api/projects/{id}/uptime/report?env=`: `uptime.report`.
async fn api_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ReportQuery>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = ReportInput {
        project,
        environment: q.env,
    };
    match GetReport::run(&state, &Caller::default(), input).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => failed(e),
    }
}

/// The body of `PUT …/uptime/target`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetBody {
    #[serde(default)]
    environment: Option<String>,
    target: Option<f64>,
}

/// `PUT /api/projects/{id}/uptime/target`: `uptime.set_target`.
async fn api_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<TargetBody>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = TargetInput {
        project: project.clone(),
        environment: b.environment,
        target: b.target,
    };
    match SetTarget::run(&state, &Caller::default(), input).await {
        Ok(t) => {
            audit(&state, &headers, &project, &t);
            Json(t).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `GET /api/projects/{id}/status` (bearer): up, down, maintenance,
/// unknown or off, with open incidents and maintenance windows (for
/// agents around a deploy), and production's `availability`
/// (`uptime.report`, #560).
pub(crate) async fn api_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !crate::auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = alerts::ProjectInput {
        project: id.clone(),
    };
    let st = match alerts::GetStatus::run(&state, &Caller::default(), input).await {
        Ok(st) => st,
        Err(e) => return failed(e),
    };
    let report = ReportInput {
        project: id,
        environment: None,
    };
    let availability = GetReport::run(&state, &Caller::default(), report)
        .await
        .ok();
    let mut v = serde_json::to_value(&st).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.insert(
            "availability".into(),
            serde_json::to_value(availability).unwrap_or_default(),
        );
    }
    Json(v).into_response()
}

/// The form of the Uptime page: a target in percent, empty removes it.
#[derive(Deserialize, Default)]
pub(crate) struct TargetForm {
    #[serde(default)]
    environment: String,
    #[serde(default)]
    target: String,
}

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
}

/// `POST /p/{id}/uptime/target`: set or remove a target.
async fn page_target(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<TargetForm>,
) -> Response {
    let raw = f.target.trim().trim_end_matches('%').trim();
    let target = if raw.is_empty() {
        None
    } else {
        match raw.replace(',', ".").parse::<f64>() {
            Ok(pct) => Some(pct / 100.0),
            Err(_) => return oops("The target is a percentage, for example 99.9."),
        }
    };
    let env = f.environment.trim();
    let input = TargetInput {
        project: id.clone(),
        environment: (!env.is_empty()).then(|| env.to_owned()),
        target,
    };
    match SetTarget::run(&state, &crate::commands::caller(&state, &headers), input).await {
        Ok(t) => {
            audit(&state, &headers, &id, &t);
            Redirect::to(&format!("/p/{id}/uptime#availability")).into_response()
        }
        Err(Error::NotFound) => oops("No such project."),
        Err(Error::Invalid(m)) if m.starts_with("target") => {
            oops("The target is a percentage from 50 to 99.999, for example 99.9.")
        }
        Err(e) => oops(&e.to_string()),
    }
}

/// The availability card of the Uptime page: production, then the
/// environments checked within the last days or with a target.
pub(crate) async fn section(state: &AppState, headers: &HeaderMap, p: &Project) -> Option<Markup> {
    let since = chrono::Utc::now().timestamp() - RAW_DAYS * 86_400;
    let mut envs = vec![PRODUCTION.to_owned()];
    let checked = state
        .meta
        .uptime_environments(&p.id, since)
        .unwrap_or_default();
    let targeted = openlens_engine::slo::targets(&state.meta, &p.id).into_keys();
    for e in checked.into_iter().chain(targeted) {
        if !envs.contains(&e) && envs.len() < MAX_SHOWN {
            envs.push(e);
        }
    }
    let mut reports = Vec::new();
    for env in envs {
        let input = ReportInput {
            project: p.id.clone(),
            environment: Some(env),
        };
        if let Ok(r) = GetReport::run(state, &Caller::default(), input).await {
            reports.push(r);
        }
    }
    let view = openlens_ui::AvailabilityView {
        reports,
        admin: crate::auth::viewer(state, headers).is_some_and(|v| v.admin),
    };
    Some(openlens_ui::availability_card(&p.id, &view))
}

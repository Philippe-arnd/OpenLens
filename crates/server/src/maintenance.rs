//! Planned maintenance windows (#563, D-076): the forms of the Uptime
//! page and the API routes, both over the engine's `maintenance.*`
//! commands. Every change is written to the audit log.

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use openlens_engine::commands::maintenance::{
    self as cmd, ExtendInput, ListInput, MaintenanceWindow, ScheduleInput, WindowInput,
};
use openlens_engine::commands::{Caller, Command, Error};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/projects/{project}/maintenance",
            get(api_list).post(api_schedule),
        )
        .route(
            "/api/projects/{project}/maintenance/{window}",
            delete(api_cancel),
        )
        .route(
            "/api/projects/{project}/maintenance/{window}/extend",
            post(api_extend),
        )
        .route(
            "/api/projects/{project}/maintenance/{window}/end",
            post(api_end),
        )
        .route("/p/{project}/uptime/maintenance", post(page_schedule))
        .route(
            "/p/{project}/uptime/maintenance/{window}/{action}",
            post(page_change),
        )
}

/// The audit entry of a change.
fn audit(
    state: &AppState,
    headers: &HeaderMap,
    project: &str,
    action: &str,
    w: &MaintenanceWindow,
) {
    let detail = format!(
        "#{} {} ({}): {}",
        w.id,
        w.when,
        w.components.join(", "),
        w.text
    );
    crate::privacy::audit(state, headers, Some(project), action, &detail);
}

// ----- API (bearer) -----

fn api_failed(e: Error) -> Response {
    let (status, m) = match e {
        Error::NotFound => (StatusCode::NOT_FOUND, "unknown project".to_owned()),
        Error::Invalid(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
        Error::Unavailable(m) => (StatusCode::BAD_REQUEST, m),
        Error::Storage(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
    };
    crate::apierr::json(status, m)
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

fn window_id(raw: &str) -> Result<i64, Box<Response>> {
    raw.parse::<i64>().map_err(|_| {
        Box::new(crate::apierr::json(
            StatusCode::NOT_FOUND,
            "no such maintenance window",
        ))
    })
}

/// `GET /api/projects/{id}/maintenance`: `maintenance.list`.
async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    match cmd::List::run(&state, &Caller::default(), ListInput { project }).await {
        Ok(v) => Json(serde_json::json!({ "windows": v })).into_response(),
        Err(e) => api_failed(e),
    }
}

/// The body of `POST /api/projects/{id}/maintenance`.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ScheduleBody {
    start: Option<String>,
    end: Option<String>,
    minutes: Option<u32>,
    components: Vec<String>,
    text: String,
    announce: bool,
}

/// `POST /api/projects/{id}/maintenance`: `maintenance.schedule`.
async fn api_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<ScheduleBody>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = ScheduleInput {
        project: project.clone(),
        start: b.start,
        end: b.end,
        minutes: b.minutes,
        components: b.components,
        text: b.text,
        announce: b.announce,
        by: crate::privacy::actor(&state, &headers),
    };
    match cmd::Schedule::run(&state, &Caller::default(), input).await {
        Ok(w) => {
            audit(&state, &headers, &project, "maintenance.schedule", &w);
            (StatusCode::CREATED, Json(w)).into_response()
        }
        Err(e) => api_failed(e),
    }
}

/// The body of `POST …/maintenance/{window}/extend`.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ExtendBody {
    end: Option<String>,
    minutes: Option<u32>,
}

/// `POST /api/projects/{id}/maintenance/{window}/extend`:
/// `maintenance.extend`.
async fn api_extend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, window)): Path<(String, String)>,
    Json(b): Json<ExtendBody>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let id = match window_id(&window) {
        Ok(w) => w,
        Err(r) => return *r,
    };
    let input = ExtendInput {
        project: project.clone(),
        id,
        end: b.end,
        minutes: b.minutes,
    };
    match cmd::Extend::run(&state, &Caller::default(), input).await {
        Ok(w) => {
            audit(&state, &headers, &project, "maintenance.extend", &w);
            Json(w).into_response()
        }
        Err(e) => api_failed(e),
    }
}

/// `POST /api/projects/{id}/maintenance/{window}/end`:
/// `maintenance.end_now`.
async fn api_end(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, window)): Path<(String, String)>,
) -> Response {
    api_window(&state, &headers, &id, &window, false).await
}

/// `DELETE /api/projects/{id}/maintenance/{window}`: `maintenance.cancel`.
async fn api_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, window)): Path<(String, String)>,
) -> Response {
    api_window(&state, &headers, &id, &window, true).await
}

async fn api_window(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    window: &str,
    cancel: bool,
) -> Response {
    let project = match api_open(state, headers, id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let id = match window_id(window) {
        Ok(w) => w,
        Err(r) => return *r,
    };
    let input = WindowInput {
        project: project.clone(),
        id,
    };
    let caller = Caller::default();
    let (r, action) = if cancel {
        (
            cmd::Cancel::run(state, &caller, input).await,
            "maintenance.cancel",
        )
    } else {
        (
            cmd::EndNow::run(state, &caller, input).await,
            "maintenance.end_now",
        )
    };
    match r {
        Ok(w) => {
            audit(state, headers, &project, action, &w);
            Json(w).into_response()
        }
        Err(e) => api_failed(e),
    }
}

// ----- The Uptime page's forms (admins) -----

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
}

fn back(project: &str) -> Response {
    Redirect::to(&format!("/p/{project}/uptime#maintenance")).into_response()
}

/// The form of the Uptime page.
#[derive(Deserialize, Default)]
pub(crate) struct ScheduleForm {
    #[serde(default)]
    start: String,
    #[serde(default)]
    minutes: Option<u32>,
    #[serde(default)]
    components: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    announce: Option<String>,
}

/// `POST /p/{id}/uptime/maintenance`: schedule a window. The start is
/// read in the viewer's zone (D-017); empty is now.
async fn page_schedule(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ScheduleForm>,
) -> Response {
    let components: Vec<String> = f
        .components
        .split([',', ' '])
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .take(cmd::MAX_COMPONENTS + 1)
        .map(str::to_owned)
        .collect();
    let input = ScheduleInput {
        project: id.clone(),
        start: Some(f.start).filter(|s| !s.trim().is_empty()),
        end: None,
        minutes: f.minutes,
        components,
        text: f.text,
        announce: f.announce.as_deref() == Some("on"),
        by: crate::privacy::actor(&state, &headers),
    };
    let caller = crate::commands::caller(&state, &headers);
    match cmd::Schedule::run(&state, &caller, input).await {
        Ok(w) => {
            audit(&state, &headers, &id, "maintenance.schedule", &w);
            back(&id)
        }
        Err(Error::NotFound) => oops("No such project."),
        Err(e) => oops(&e.to_string()),
    }
}

/// The form of a window's buttons.
#[derive(Deserialize, Default)]
pub(crate) struct ChangeForm {
    #[serde(default)]
    minutes: Option<u32>,
}

/// `POST /p/{id}/uptime/maintenance/{window}/{extend|end|cancel}`.
async fn page_change(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, window, action)): Path<(String, String, String)>,
    Form(f): Form<ChangeForm>,
) -> Response {
    let Ok(wid) = window.parse::<i64>() else {
        return oops("No such maintenance window.");
    };
    let caller = crate::commands::caller(&state, &headers);
    let one = || WindowInput {
        project: id.clone(),
        id: wid,
    };
    let r = match action.as_str() {
        "extend" => {
            let input = ExtendInput {
                project: id.clone(),
                id: wid,
                end: None,
                minutes: Some(f.minutes.unwrap_or(30)),
            };
            cmd::Extend::run(&state, &caller, input).await
        }
        "end" => cmd::EndNow::run(&state, &caller, one()).await,
        "cancel" => cmd::Cancel::run(&state, &caller, one()).await,
        _ => return oops("Unknown action."),
    };
    match r {
        Ok(w) => {
            let name = match action.as_str() {
                "extend" => "maintenance.extend",
                "end" => "maintenance.end_now",
                _ => "maintenance.cancel",
            };
            audit(&state, &headers, &id, name, &w);
            back(&id)
        }
        Err(Error::NotFound) => oops("No such project."),
        Err(e) => oops(&e.to_string()),
    }
}

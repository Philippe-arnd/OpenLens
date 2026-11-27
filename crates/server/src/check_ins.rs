//! Job check-ins (#609, D-086): `POST /check-in/{key}/{slug}` for a shell
//! script at the end of a cron, `POST /check-in/{slug}` with the project
//! key in `X-OpenLens-Key` for the server SDKs, the API routes and the
//! Jobs card of the Uptime page, all over the engine's `jobs.*` commands.
//!
//! The endpoint is bounded and fails closed:
//! - the capture limiter's requests per client address first, before
//!   anything is looked up;
//! - the slug is checked before the key (1 to 64 of `a-z0-9_-`);
//! - one check-in per job every 10 seconds (429 with `Retry-After`);
//! - at most `jobs::MAX_JOBS` jobs per project, in the store;
//! - no body is read.
//!
//! The key may be in the path, so the request log shows the path as
//! `/check-in/<key>/<slug>` ([`log_path`]) and no answer or log line names
//! the key or the path.

use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use openlens_engine::commands::jobs::{
    self as cmd, AvailabilityInput, CheckInInput, ProjectInput, RemoveInput,
};
use openlens_engine::commands::{Caller, Command, Error};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::capture::{Peer, error, ip_limited, project_for_key};
use crate::web::html_page;

/// The path prefix of the endpoint.
const PREFIX: &str = "/check-in/";
/// Check-ins a second per job (one every 10 seconds), with no burst.
const JOB_RATE: f64 = 0.1;
const JOB_BURST: f64 = 1.0;

pub(crate) fn routes() -> Router<AppState> {
    let public = Router::new()
        .route("/check-in/{key}/{slug}", post(by_path))
        .route("/check-in/{slug}", post(by_header))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ));
    Router::new()
        .merge(public)
        .route("/api/projects/{project}/jobs", get(api_list))
        .route(
            "/api/projects/{project}/jobs/availability",
            put(api_availability),
        )
        .route("/api/projects/{project}/jobs/{slug}", delete(api_remove))
        .route(
            "/api/projects/{project}/jobs/{slug}/check-in",
            post(api_check_in),
        )
        .route(
            "/p/{project}/uptime/jobs/availability",
            post(page_availability),
        )
        .route("/p/{project}/uptime/jobs/{slug}/remove", post(page_remove))
}

/// The path as logged: a project key in the path is never written.
pub(crate) fn log_path(path: &str) -> &str {
    match path.strip_prefix(PREFIX) {
        Some(rest) if rest.contains('/') => "/check-in/<key>/<slug>",
        _ => path,
    }
}

/// The endpoint's options, in the query string.
#[derive(Debug, Deserialize, Default)]
pub(crate) struct CheckInQuery {
    /// `ok` (default) or `error`.
    #[serde(default)]
    status: Option<String>,
    /// The run's duration, in milliseconds.
    #[serde(default)]
    ms: Option<i64>,
    /// The gap between runs, in seconds.
    #[serde(default, rename = "expectEvery", alias = "expect_every")]
    expect_every: Option<i64>,
}

/// Whether this job checked in less than 10 seconds ago.
fn job_limited(state: &AppState, project: &str, slug: &str) -> bool {
    !state
        .caches
        .check_in_buckets
        .lock()
        .is_ok_and(|mut b| b.take(&format!("{project}|{slug}"), 1.0, JOB_RATE, JOB_BURST))
}

fn too_fast() -> Response {
    let mut r = error(
        StatusCode::TOO_MANY_REQUESTS,
        "one check-in per job every 10 seconds; retry later",
    );
    r.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("10"),
    );
    r
}

fn refused(e: Error) -> Response {
    match e {
        Error::Invalid(m) | Error::Unavailable(m) => error(StatusCode::BAD_REQUEST, &m),
        Error::NotFound => error(StatusCode::UNAUTHORIZED, "unknown project key"),
        Error::Storage(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the check-in could not be recorded; retry later",
        ),
    }
}

/// `POST /check-in/{key}/{slug}[?status=ok|error&ms=&expectEvery=]`.
async fn by_path(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Path((key, slug)): Path<(String, String)>,
    q: Result<Query<CheckInQuery>, QueryRejection>,
) -> Response {
    check_in(&state, &peer, &headers, Some(key), slug, q).await
}

/// `POST /check-in/{slug}[?…]` with the key in `X-OpenLens-Key` (SDKs).
async fn by_header(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Path(slug): Path<String>,
    q: Result<Query<CheckInQuery>, QueryRejection>,
) -> Response {
    let key = headers
        .get("x-openlens-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    check_in(&state, &peer, &headers, key, slug, q).await
}

async fn check_in(
    state: &AppState,
    peer: &Peer,
    headers: &HeaderMap,
    key: Option<String>,
    slug: String,
    q: Result<Query<CheckInQuery>, QueryRejection>,
) -> Response {
    if ip_limited(state, peer, headers) {
        return crate::capture::too_many();
    }
    if let Err(e) = cmd::valid_slug(&slug) {
        return refused(e);
    }
    // Axum's own rejection text is not sent: ours names no input.
    let Ok(Query(q)) = q else {
        return error(
            StatusCode::BAD_REQUEST,
            "status is ok or error; ms and expectEvery are whole numbers",
        );
    };
    let project = match project_for_key(state, key).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if job_limited(state, &project.id, &slug) {
        return too_fast();
    }
    let input = CheckInInput {
        project: project.id.clone(),
        slug,
        status: q.status,
        ms: q.ms,
        expect_every: q.expect_every,
    };
    match cmd::CheckIn::run(state, &Caller::default(), input).await {
        Ok(c) => Json(json!({ "recorded": true, "slug": c.slug, "ok": c.ok })).into_response(),
        Err(Error::Storage(e)) => {
            tracing::warn!(project = %project.id, %e, "check-in not recorded");
            refused(Error::Storage(e))
        }
        // Refused input is answered, never logged.
        Err(e) => refused(e),
    }
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

/// `GET /api/projects/{id}/jobs`: `jobs.list`.
async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    match cmd::List::run(&state, &Caller::default(), ProjectInput { project }).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_failed(e),
    }
}

/// The body of `POST …/jobs/{slug}/check-in` (all optional).
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct CheckInBody {
    status: Option<String>,
    ms: Option<i64>,
    #[serde(alias = "expectEvery")]
    expect_every: Option<i64>,
}

/// `POST /api/projects/{id}/jobs/{slug}/check-in`: `jobs.check_in`, for
/// an agent or a script that holds a token rather than the project key.
async fn api_check_in(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, slug)): Path<(String, String)>,
    body: Option<Json<CheckInBody>>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if let Err(e) = cmd::valid_slug(&slug) {
        return api_failed(e);
    }
    if job_limited(&state, &project, &slug) {
        return too_fast();
    }
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let input = CheckInInput {
        project,
        slug,
        status: b.status,
        ms: b.ms,
        expect_every: b.expect_every,
    };
    match cmd::CheckIn::run(&state, &Caller::default(), input).await {
        Ok(c) => Json(c).into_response(),
        Err(e) => api_failed(e),
    }
}

/// `DELETE /api/projects/{id}/jobs/{slug}`: `jobs.remove`.
async fn api_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, slug)): Path<(String, String)>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = RemoveInput {
        project: project.clone(),
        slug: slug.clone(),
    };
    match cmd::Remove::run(&state, &Caller::default(), input).await {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&project), "jobs.remove", &slug);
            Json(json!({ "removed": slug })).into_response()
        }
        Err(e) => api_failed(e),
    }
}

/// The body of `PUT …/jobs/availability`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AvailabilityBody {
    on: bool,
}

/// `PUT /api/projects/{id}/jobs/availability`: `jobs.set_availability`.
async fn api_availability(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<AvailabilityBody>,
) -> Response {
    let project = match api_open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = AvailabilityInput {
        project: project.clone(),
        on: b.on,
    };
    match cmd::SetAvailability::run(&state, &Caller::default(), input).await {
        Ok(on) => {
            let detail = if on { "on" } else { "off" };
            crate::privacy::audit(
                &state,
                &headers,
                Some(&project),
                "jobs.set_availability",
                detail,
            );
            Json(json!({ "in_availability": on })).into_response()
        }
        Err(e) => api_failed(e),
    }
}

// ----- The Uptime page (admins change, everyone reads) -----

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
}

fn back(project: &str) -> Response {
    Redirect::to(&format!("/p/{project}/uptime#jobs")).into_response()
}

/// The availability form: the checkbox is absent when unticked.
#[derive(Deserialize, Default)]
pub(crate) struct AvailabilityForm {
    #[serde(default)]
    on: Option<String>,
}

/// `POST /p/{id}/uptime/jobs/availability`.
async fn page_availability(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<AvailabilityForm>,
) -> Response {
    let on = f.on.as_deref() == Some("on");
    let input = AvailabilityInput {
        project: id.clone(),
        on,
    };
    let caller = crate::commands::caller(&state, &headers);
    match cmd::SetAvailability::run(&state, &caller, input).await {
        Ok(on) => {
            let detail = if on { "on" } else { "off" };
            crate::privacy::audit(&state, &headers, Some(&id), "jobs.set_availability", detail);
            back(&id)
        }
        Err(Error::NotFound) => oops("No such project."),
        Err(e) => oops(&e.to_string()),
    }
}

/// `POST /p/{id}/uptime/jobs/{slug}/remove`.
async fn page_remove(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, slug)): Path<(String, String)>,
) -> Response {
    let input = RemoveInput {
        project: id.clone(),
        slug: slug.clone(),
    };
    let caller = crate::commands::caller(&state, &headers);
    match cmd::Remove::run(&state, &caller, input).await {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&id), "jobs.remove", &slug);
            back(&id)
        }
        Err(Error::NotFound) => oops("No such project."),
        Err(e) => oops(&e.to_string()),
    }
}

fn when(t: i64, tz: &str) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| {
        openlens_ui::local_time(&d.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)
    })
}

/// The Jobs card of the Uptime page, from `jobs.list`.
pub(crate) async fn section(
    state: &AppState,
    headers: &HeaderMap,
    p: &openlens_meta::Project,
    tz: &str,
) -> openlens_ui::CheckInsView {
    let input = ProjectInput {
        project: p.id.clone(),
    };
    let jobs = cmd::List::run(state, &Caller::default(), input).await.ok();
    let (rows, in_availability, max) = jobs.map_or_else(
        || (Vec::new(), false, cmd::MAX_JOBS),
        |j| {
            let rows = j
                .jobs
                .into_iter()
                .map(|s| openlens_ui::CheckInRow {
                    last: s.last_at.map(|t| when(t, tz)),
                    every: s.every.map(cmd::every_words),
                    usual: s.usual_ms,
                    slug: s.slug,
                    state: s.state,
                    last_ok: s.last_ok,
                    last_ms: s.last_ms,
                    declared: s.declared,
                    to_learn: s.to_learn,
                })
                .collect();
            (rows, j.in_availability, j.max)
        },
    );
    openlens_ui::CheckInsView {
        rows,
        in_availability,
        max,
        admin: crate::auth::viewer(state, headers).is_some_and(|v| v.admin),
        base: crate::public_url(headers),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_key_is_never_in_the_logged_path() {
        let p = super::log_path("/check-in/olk_secretkey/nightly-export");
        assert!(!p.contains("olk_secretkey"));
        // The SDK form has no key in its path: logged as it is.
        assert_eq!(
            super::log_path("/check-in/nightly-export"),
            "/check-in/nightly-export"
        );
        assert_eq!(super::log_path("/p/demo/uptime"), "/p/demo/uptime");
    }
}

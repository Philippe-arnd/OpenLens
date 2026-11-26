//! The board job routes (#515): the dashboard's form, progress page and
//! Cancel button, and the API's start, read and cancel.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::commands::board::GetEstimate;
use openlens_engine::commands::{Caller, Command};
use openlens_engine::{Filters, Window};
use serde_json::json;

use super::{Job, JobState, NewJob, Refused};
use crate::AppState;
use crate::access::Signed;
use crate::board_query::BoardQuery;

/// Seconds between two loads of the progress page.
const REFRESH_SECS: &str = "2";

async fn find(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let (meta, id) = (state.meta.clone(), id.to_owned());
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

/// The window, said, when the estimate has no label.
fn said(w: &Window) -> String {
    match w {
        Window::Range { from, to } => format!("From {from} to {to}"),
        Window::Since { release } => format!("While {release} was the newest release"),
        Window::Release => "Since the current release".to_owned(),
        Window::Fixed => "The last 7, 14 and 30 days".to_owned(),
    }
}

/// The job of `q` for `owner`: the filters, the feature map and the
/// estimate's label and time.
///
/// # Errors
/// A bad date, range or release, said for the person.
async fn new_job(
    state: &AppState,
    owner: String,
    p: &openlens_meta::Project,
    q: &BoardQuery,
    tz: &str,
) -> Result<NewJob, String> {
    let f: Filters = q.filters(tz).map_err(|e| e.to_string())?;
    let caller = Caller {
        tz: Some(f.tz.clone()),
    };
    let estimate = match GetEstimate::run(state, &caller, q.estimate_input(&p.id)).await {
        Ok(e) => e,
        Err(openlens_engine::commands::Error::Invalid(m)) => return Err(m),
        // The board says what went wrong, if anything does.
        Err(_) => None,
    };
    Ok(NewJob {
        owner,
        project: p.id.clone(),
        label: estimate
            .as_ref()
            .map_or_else(|| said(&f.window), |e| e.label.clone()),
        estimate_secs: estimate.as_ref().map_or(0.0, |e| e.secs),
        map: crate::load_map(state, &p.id).await,
        params: q.params(),
        filters: f,
    })
}

fn not_found() -> Response {
    crate::web::html_page(
        StatusCode::NOT_FOUND,
        openlens_ui::message(
            "Not found",
            "No such board job: it is kept for an hour, for the person who started it only.",
        ),
    )
}

fn job_url(project: &str, id: &str) -> String {
    format!(
        "/p/{}/board/jobs/{}",
        openlens_ui::url_enc(project),
        openlens_ui::url_enc(id)
    )
}

/// `POST /p/{project}/board/jobs`: the confirm page's "Run it" (D-061)
/// starts the board in the background and goes to its progress page; a
/// person who already has a board running goes to that one.
pub(crate) async fn page_start(
    Signed(v): Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Form(q): Form<BoardQuery>,
) -> Response {
    let Some(p) = find(&state, &project).await else {
        return not_found();
    };
    let tz = crate::auth::tz_for(&state, &headers, &p.timezone);
    let new = match new_job(&state, v.id, &p, &q, &tz).await {
        Ok(n) => n,
        Err(e) => {
            return crate::web::html_page(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't show that", &e),
            );
        }
    };
    match super::start(&state, new) {
        Ok(id) | Err(Refused::Yours(id)) => Redirect::to(&job_url(&p.id, &id)).into_response(),
        Err(Refused::Full) => crate::web::html_page(
            StatusCode::TOO_MANY_REQUESTS,
            openlens_ui::message(
                "Too many long boards",
                "This server is already computing as many long boards as it takes. Try again in a few minutes.",
            ),
        ),
    }
}

/// `GET /p/{project}/board/jobs/{job}`: the board once the job is done;
/// before, where it is, refreshed every two seconds (a `Refresh` header,
/// no script); after a failure or a cancel, what happened.
pub(crate) async fn page(
    Signed(v): Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    let Some(job) = state
        .caches
        .board_jobs
        .get(&v.id, &id)
        .filter(|j| j.project == project)
    else {
        return not_found();
    };
    let Some(p) = find(&state, &project).await else {
        return not_found();
    };
    if let Some(done) = &job.result {
        let (board, revenue) = &**done;
        let host = crate::public_url(&headers);
        let revenue = revenue.as_ref().ok().and_then(Option::as_ref);
        return crate::web::render_board(
            &state,
            &p,
            &job.filters,
            &host,
            v.admin,
            Ok(board.clone()),
            revenue,
        );
    }
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let back = format!(
        "/p/{}?{}",
        openlens_ui::url_enc(&p.id),
        serde_urlencoded::to_string(&job.params).unwrap_or_default()
    );
    let view = openlens_ui::JobView {
        id: &job.id,
        label: &job.label,
        state: job.state.name(),
        elapsed_secs: job.elapsed_secs(),
        estimate_secs: job.estimate_secs,
        error: job.error.as_deref(),
        back: &back,
    };
    let mut r = crate::web::html_page(StatusCode::OK, openlens_ui::board_job(&info, &view));
    if job.state.active() {
        r.headers_mut()
            .insert(header::REFRESH, HeaderValue::from_static(REFRESH_SECS));
    }
    r
}

/// `POST /p/{project}/board/jobs/{job}/cancel`: stop it, back to its page.
pub(crate) async fn page_cancel(
    Signed(v): Signed,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    let jobs = &state.caches.board_jobs;
    if jobs.get(&v.id, &id).is_none_or(|j| j.project != project) {
        return not_found();
    }
    jobs.cancel(&v.id, &id);
    Redirect::to(&job_url(&project, &id)).into_response()
}

/// Who calls the API: the admin token is the owner (the same person as
/// the owner's dashboard session); a scoped token is itself, by the hash
/// of its secret.
fn api_owner(state: &AppState, headers: &HeaderMap) -> String {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    if crate::admin_token_matches(state, token) {
        crate::auth::OWNER.to_owned()
    } else {
        format!("token:{}", openlens_meta::sha256_hex(token))
    }
}

/// A job as the API shows it: the board (as `GET …/board` answers it) and
/// its revenue once done.
fn job_json(j: &Job) -> serde_json::Value {
    let (board, revenue) = match &j.result {
        Some(done) => (
            serde_json::to_value(&done.0).unwrap_or_default(),
            json!(done.1.as_ref().ok()),
        ),
        None => (serde_json::Value::Null, serde_json::Value::Null),
    };
    json!({
        "id": j.id,
        "project": j.project,
        "state": j.state,
        "window": j.label,
        "progress": j.progress(),
        "elapsed_secs": j.elapsed_secs(),
        "estimate_secs": j.estimate_secs,
        "error": j.error,
        "board": board,
        "revenue": revenue,
    })
}

fn api_job_path(project: &str, id: &str) -> String {
    format!(
        "/api/projects/{}/board/jobs/{}",
        openlens_ui::url_enc(project),
        openlens_ui::url_enc(id)
    )
}

/// `POST /api/projects/{project}/board/jobs` with the board's parameters
/// in the query: 202 with the job's id and where to read it; 429 when the
/// caller already has a job queued or running (its id is in `job`) or the
/// instance is full.
pub(crate) async fn api_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Query(q): Query<BoardQuery>,
) -> Response {
    if let Some(r) = crate::api::guard(&state, &headers) {
        return r;
    }
    let p = match crate::api::known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let owner = api_owner(&state, &headers);
    let new = match new_job(&state, owner, &p, &q, &p.timezone).await {
        Ok(n) => n,
        Err(e) => return crate::apierr::json(StatusCode::BAD_REQUEST, e),
    };
    match super::start(&state, new) {
        Ok(id) => (
            StatusCode::ACCEPTED,
            axum::Json(json!({
                "id": id,
                "state": JobState::Queued,
                "url": api_job_path(&p.id, &id),
            })),
        )
            .into_response(),
        Err(Refused::Yours(id)) => {
            let m = "you already have a board job queued or running: wait for it or cancel it";
            (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(json!({
                    "code": "rate_limited",
                    "message": m,
                    "error": m,
                    "job": id,
                    "url": api_job_path(&p.id, &id),
                })),
            )
                .into_response()
        }
        Err(Refused::Full) => crate::apierr::json(
            StatusCode::TOO_MANY_REQUESTS,
            "the server is computing as many board jobs as it takes; try again in a few minutes",
        ),
    }
}

fn api_find(
    state: &AppState,
    headers: &HeaderMap,
    project: &str,
    id: &str,
) -> Result<(String, Job), Box<Response>> {
    if let Some(r) = crate::api::guard(state, headers) {
        return Err(Box::new(r));
    }
    let owner = api_owner(state, headers);
    match state
        .caches
        .board_jobs
        .get(&owner, id)
        .filter(|j| j.project == project)
    {
        Some(j) => Ok((owner, j)),
        // Someone else's job is not found either (D-063).
        None => Err(Box::new(crate::apierr::json(
            StatusCode::NOT_FOUND,
            "no such board job (kept for an hour, for whoever started it)",
        ))),
    }
}

/// `GET /api/projects/{project}/board/jobs/{job}`: its state, progress
/// and, once done, the board.
pub(crate) async fn api_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    match api_find(&state, &headers, &project, &id) {
        Ok((_, j)) => axum::Json(job_json(&j)).into_response(),
        Err(r) => *r,
    }
}

/// `DELETE /api/projects/{project}/board/jobs/{job}`: cancel it (a
/// finished job is left as it is).
pub(crate) async fn api_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    let owner = match api_find(&state, &headers, &project, &id) {
        Ok((owner, _)) => owner,
        Err(r) => return *r,
    };
    let jobs = &state.caches.board_jobs;
    jobs.cancel(&owner, &id);
    match jobs.get(&owner, &id) {
        Some(j) => axum::Json(job_json(&j)).into_response(),
        None => crate::apierr::json(StatusCode::NOT_FOUND, "no such board job"),
    }
}

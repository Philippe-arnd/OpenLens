//! Tickets (#245): the page, form and REST adapters of the engine's
//! `tickets.*` commands (#503, D-042), and the GitHub transport they run
//! on: where the token comes from, and the HTTP calls. The drafting, the
//! filing and the hourly follow-up live in
//! `openlens_engine::commands::tickets`.
//!
//! Nothing is posted without a click (or an admin API call): the hourly
//! pass only comments on tickets someone already created.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::commands::tickets::{self as cmd, FindingInput, ProjectInput, TargetInput};
use openlens_engine::commands::{Caller, Command, Error, Host};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

/// The GitHub target: (API base, owner/repo, token), when configured.
pub(crate) fn github(state: &AppState, project: &str) -> Option<(String, String, String)> {
    let repo = state
        .meta
        .project_setting(project, "tickets_repo")
        .ok()
        .flatten()
        .filter(|r| r.split('/').count() == 2)?;
    let token = std::env::var("OPENLENS_GITHUB_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            state
                .meta
                .setting("github_token")
                .ok()
                .flatten()
                .and_then(|s| crate::secrets::open(&state.data_dir, &s).ok())
        })?;
    let api = state
        .meta
        .setting("github_api")
        .ok()
        .flatten()
        .unwrap_or_else(|| "https://api.github.com".into());
    Some((api, repo, token))
}

pub(crate) fn gh(
    method: &str,
    url: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let auth = format!("Bearer {token}");
    let mut resp = match (method, body) {
        ("GET", _) => agent
            .get(url)
            .header("authorization", &auth)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "OpenLens")
            .call(),
        (m, b) => {
            let r = if m == "PATCH" {
                agent.patch(url)
            } else {
                agent.post(url)
            };
            r.header("authorization", &auth)
                .header("accept", "application/vnd.github+json")
                .header("user-agent", "OpenLens")
                .send_json(b.unwrap_or_default())
        }
    }
    .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let v: serde_json::Value = resp.body_mut().read_json().unwrap_or_default();
    if status >= 300 {
        return Err(format!(
            "GitHub answered {status}: {}",
            v["message"].as_str().unwrap_or("")
        ));
    }
    Ok(v)
}

fn host(headers: &HeaderMap) -> String {
    crate::public_url(headers)
}

#[derive(Deserialize)]
pub(crate) struct Finding {
    kind: String,
    subject: String,
    /// The flag the change ships behind (#704).
    #[serde(default)]
    flag: Option<String>,
}

fn info(p: &openlens_meta::Project) -> openlens_ui::ProjectInfo<'_> {
    openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    }
}

fn not_found() -> Response {
    html_page(
        StatusCode::NOT_FOUND,
        openlens_ui::message("Not found", "No such project."),
    )
}

fn finding(id: &str, f: &Finding, headers: &HeaderMap) -> FindingInput {
    FindingInput {
        project: id.to_owned(),
        kind: f.kind.clone(),
        subject: f.subject.clone(),
        host: host(headers),
        flag: f.flag.clone(),
    }
}

/// `GET /p/{id}/tickets/new?kind=&subject=`: the drafted ticket.
pub(crate) async fn new(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Finding>,
) -> Response {
    let Some(p) = state.project(&id).await else {
        return not_found();
    };
    let input = finding(&p.id, &q, &headers);
    match cmd::DraftTicket::run(&state, &Caller::default(), input).await {
        Ok(d) => html_page(
            StatusCode::OK,
            openlens_ui::ticket_draft(&openlens_ui::TicketDraft {
                p: &info(&p),
                title: &d.draft.ticket.title,
                markdown: &d.draft.markdown,
                kind: &q.kind,
                subject: &q.subject,
                flag: d.draft.ticket.flag.as_deref(),
                repo: d.repo,
                existing: d.existing,
                admin: v.admin,
            }),
        ),
        Err(e) => html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Can't draft that ticket", &e.to_string()),
        ),
    }
}

/// `POST /p/{id}/tickets` (admins): open or update the GitHub issue.
pub(crate) async fn create(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<Finding>,
) -> Response {
    let Some(p) = state.project(&id).await else {
        return not_found();
    };
    let input = finding(&p.id, &f, &headers);
    match cmd::File::run(&state, &Caller::default(), input).await {
        Ok(t) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "ticket.file", &t.title);
            Redirect::to(&format!("/p/{}/tickets", p.id)).into_response()
        }
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't open the issue", &e.to_string()),
        ),
    }
}

/// `GET /p/{id}/tickets`: tickets and the learning log.
pub(crate) async fn list(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = state.project(&id).await else {
        return not_found();
    };
    let input = ProjectInput {
        project: p.id.clone(),
    };
    let Ok(out) = cmd::List::run(&state, &Caller::default(), input).await else {
        return not_found();
    };
    let rows: Vec<openlens_ui::TicketRow> = out
        .tickets
        .into_iter()
        .rev()
        .map(|t| openlens_ui::TicketRow {
            title: t.title,
            kind: t.kind,
            created: t.created,
            url: t.issue.map(|i| i.1),
            metric: t.metric.map(|m| openlens_engine::goals::kpi_info(&m).0),
            shipped_in: t
                .shipped_in
                .map(|s| s.split('|').next().unwrap_or("").to_owned()),
            outcome: t.outcome,
        })
        .collect();
    html_page(
        StatusCode::OK,
        openlens_ui::tickets(&info(&p), &rows, &out.repo, out.github, v.admin),
    )
}

#[derive(Deserialize)]
pub(crate) struct TargetForm {
    #[serde(default)]
    repo: String,
    /// Empty keeps the stored token.
    #[serde(default)]
    token: String,
}

/// `POST /p/{id}/tickets/target` (admins): the GitHub repository (and the
/// token, shared by every project, stored sealed).
pub(crate) async fn target(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<TargetForm>,
) -> Response {
    let input = TargetInput {
        project: id.clone(),
        repo: f.repo,
        token: f.token,
    };
    match cmd::SetTarget::run(&state, &Caller::default(), input).await {
        Ok(repo) => {
            crate::privacy::audit(&state, &headers, Some(&id), "tickets.target", &repo);
            Redirect::to(&format!("/p/{id}/tickets")).into_response()
        }
        Err(Error::Invalid(e)) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Not a repository", &e),
        ),
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't store it", &e.to_string()),
        ),
    }
}

/// `GET /api/projects/{id}/tickets` (bearer): tickets and the learning log.
pub(crate) async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match cmd::List::run(&state, &Caller::default(), ProjectInput { project: id }).await {
        Ok(out) => axum::Json(json!({
            "tickets": out.tickets,
            "learning": out.learning,
        }))
        .into_response(),
        Err(e) => crate::apierr::json(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
pub(crate) struct ApiTicket {
    kind: String,
    subject: String,
    /// `markdown` (default: only draft it) or `github` (open or update).
    #[serde(default)]
    to: String,
    /// The flag the change ships behind (#704): judged on its cohorts.
    #[serde(default)]
    flag: Option<String>,
}

/// `POST /api/projects/{id}/tickets` (bearer): draft a ticket, or file it.
pub(crate) async fn api_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::Json(b): axum::Json<ApiTicket>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let input = FindingInput {
        project: id.clone(),
        kind: b.kind,
        subject: b.subject,
        host: host(&headers),
        flag: b.flag,
    };
    if b.to != "github" {
        return match cmd::DraftTicket::run(&state, &Caller::default(), input).await {
            Ok(d) => axum::Json(json!({
                "title": d.draft.ticket.title,
                "markdown": d.draft.markdown,
                "fingerprint": d.draft.ticket.fp,
                "flag": d.draft.ticket.flag,
            }))
            .into_response(),
            Err(Error::NotFound) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => crate::apierr::json(StatusCode::UNPROCESSABLE_ENTITY, e),
        };
    }
    match cmd::File::run(&state, &Caller::default(), input).await {
        Ok(t) => {
            crate::privacy::audit(&state, &headers, Some(&id), "ticket.file", &t.title);
            axum::Json(json!({ "ticket": t })).into_response()
        }
        Err(Error::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e @ Error::Invalid(_)) => crate::apierr::json(StatusCode::UNPROCESSABLE_ENTITY, e),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e),
    }
}

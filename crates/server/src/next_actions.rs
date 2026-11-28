//! Next actions (#715, D-104): the page and its Approve and Dismiss forms,
//! and the REST routes the CLI and the MCP tools use (D-020): the list,
//! and an agent's proposal. The ranking, the states and the storage live
//! in `openlens_engine::commands::actions`. Every change is written to the
//! audit log.
//!
//! Approving and dismissing are a person's decisions: they are dashboard
//! forms for admins, not API routes. Proposing posts nothing outside
//! OpenLens, so a `read` token may propose (`crate::scopes`).

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::commands::actions::{
    self as cmd, ApproveInput, DismissInput, NextInput, ProposeInput,
};
use openlens_engine::commands::{Caller, Command, Error, Host};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

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

/// `GET /p/{id}/actions`: the ranked actions.
pub(crate) async fn page(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = state.project(&id).await else {
        return not_found();
    };
    let input = NextInput {
        project: p.id.clone(),
        state: None,
    };
    match cmd::Next::run(&state, &Caller::default(), input).await {
        Ok(out) => {
            let github = state.github(&p.id).is_some();
            html_page(
                StatusCode::OK,
                openlens_ui::next_actions(&info(&p), &out, v.admin, github),
            )
        }
        Err(e) => html_page(
            StatusCode::SERVICE_UNAVAILABLE,
            openlens_ui::message("Next actions are not available now", &e.to_string()),
        ),
    }
}

/// `POST /p/{id}/actions/{action}/approve` (admins): file its ticket.
pub(crate) async fn approve(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, action)): Path<(String, String)>,
) -> Response {
    let input = ApproveInput {
        project: id.clone(),
        id: action,
        host: crate::public_url(&headers),
    };
    match cmd::Approve::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&id),
                "action.approve",
                &a.finding.title,
            );
            Redirect::to(&format!("/p/{id}/actions")).into_response()
        }
        Err(Error::NotFound) => not_found(),
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't approve it", &e.to_string()),
        ),
    }
}

#[derive(Deserialize)]
pub(crate) struct DismissForm {
    #[serde(default)]
    reason: String,
}

/// `POST /p/{id}/actions/{action}/dismiss` (admins): hide it for 30 days.
pub(crate) async fn dismiss(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, action)): Path<(String, String)>,
    Form(f): Form<DismissForm>,
) -> Response {
    let input = DismissInput {
        project: id.clone(),
        id: action,
        reason: f.reason,
    };
    match cmd::Dismiss::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            let reason = a.dismissed.as_ref().map_or("", |d| d.reason.as_str());
            let detail = format!("{}: {reason}", a.finding.title);
            crate::privacy::audit(&state, &headers, Some(&id), "action.dismiss", &detail);
            Redirect::to(&format!("/p/{id}/actions")).into_response()
        }
        Err(Error::NotFound) => not_found(),
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't dismiss it", &e.to_string()),
        ),
    }
}

/// A command's error as an API answer.
fn failed(e: Error) -> Response {
    let status = match e {
        Error::NotFound => StatusCode::NOT_FOUND,
        Error::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        Error::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        Error::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    crate::apierr::json(status, e)
}

#[derive(Deserialize)]
pub(crate) struct NextQuery {
    #[serde(default)]
    state: Option<String>,
}

/// `GET /api/projects/{id}/next-actions?state=`: `actions.next`.
pub(crate) async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<NextQuery>,
) -> Response {
    if let Some(r) = crate::api::guard(&state, &headers) {
        return r;
    }
    let input = NextInput {
        project: id,
        state: q.state,
    };
    match cmd::Next::run(&state, &Caller::default(), input).await {
        Ok(out) => axum::Json(out).into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
pub(crate) struct ProposeBody {
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/projects/{id}/next-actions/{action}/propose`:
/// `actions.propose`. The ticket is drafted and kept; nothing is posted.
pub(crate) async fn api_propose(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, action)): Path<(String, String)>,
    body: Option<axum::Json<ProposeBody>>,
) -> Response {
    if let Some(r) = crate::api::guard(&state, &headers) {
        return r;
    }
    let note = body.and_then(|b| b.0.note);
    let input = ProposeInput {
        project: id.clone(),
        id: action,
        note,
        host: crate::public_url(&headers),
    };
    match cmd::Propose::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&id),
                "action.propose",
                &a.finding.title,
            );
            axum::Json(a).into_response()
        }
        Err(e) => failed(e),
    }
}

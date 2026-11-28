//! Goals on KPIs (#235): the Goals page, setting a goal from a suggested
//! target, deleting one, and the API for agents. The logic is the engine's
//! `goals.*` commands (#494); these handlers adapt them to forms, pages
//! and JSON.

use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::commands::goals::{self as cmd, NewGoal, NewGuardrail};
use openlens_engine::commands::{Caller, Command, Error, Host};
use openlens_engine::goals::Goal;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

/// Hourly (#272): breached guardrails become incidents (and close when
/// they recover), objectives past their period are archived with the
/// final values, and a snapshot feeds the board banner and the digest.
pub(crate) async fn hourly(state: &AppState, p: &openlens_meta::Project, alert: bool) {
    let Ok(ev) = Box::pin(cmd::evaluate(state, p, &p.timezone)).await else {
        return;
    };
    if alert {
        for c in cmd::guard_checks(&ev) {
            Box::pin(openlens_engine::commands::alerts::track(
                state, &p.id, &p.name, &c.rule, c.breached, &c.text, 0,
            ))
            .await;
        }
    }
    cmd::close_periods(&state.meta, &p.id, &ev);
}

/// The last hourly snapshot: (objectives at risk, goal lines with status).
pub(crate) fn snapshot(state: &AppState, project: &str) -> (Vec<String>, Vec<(String, String)>) {
    cmd::snapshot(&state.meta, project)
}

/// `GET /p/{id}/goals`.
pub(crate) async fn page(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = state.project(&id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let caller = crate::commands::caller(&state, &headers);
    let input = cmd::ProjectInput {
        project: p.id.clone(),
    };
    match cmd::List::run(&state, &caller, input).await {
        Ok(ev) => {
            let info = openlens_ui::ProjectInfo {
                id: &p.id,
                name: &p.name,
                key: &p.api_key,
            };
            let default_deadline = (chrono::Utc::now() + chrono::Duration::days(90))
                .format("%Y-%m-%d")
                .to_string();
            html_page(
                StatusCode::OK,
                openlens_ui::goals(
                    &info,
                    &ev.rows,
                    &ev.choices,
                    &ev.objectives,
                    &ev.unavailable,
                    v.admin,
                    &default_deadline,
                ),
            )
        }
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

#[derive(Deserialize)]
pub(crate) struct NewGoalForm {
    kpi: String,
    /// In display units: a percentage for shares.
    target: f64,
    #[serde(default)]
    deadline: String,
    /// Or a number of releases (#272).
    #[serde(default)]
    releases: String,
    #[serde(default)]
    owner: String,
    /// Guardrails (#272): KPI keys, and limits in display units (empty:
    /// 5% worse than now).
    #[serde(default)]
    guard1: String,
    #[serde(default)]
    guard1_limit: String,
    #[serde(default)]
    guard2: String,
    #[serde(default)]
    guard2_limit: String,
}

/// `POST /p/{id}/goals` (admins): `goals.set`, with the form's
/// percentages turned into shares first.
pub(crate) async fn create(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<NewGoalForm>,
) -> Response {
    let oops = |m: &str| {
        html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't set that goal", m),
        )
    };
    let Some(p) = state.project(&id).await else {
        return oops("No such project.");
    };
    let tz = auth::tz_for(&state, &headers, &p.timezone);
    let Ok(ev) = cmd::evaluate(&state, &p, &tz).await else {
        return oops("The board is unavailable.");
    };
    let share_of = |k: &str| {
        ev.choices
            .iter()
            .find(|c| c.kpi == k)
            .is_some_and(|c| c.share)
    };
    let raw = |k: &str, v: f64| if share_of(k) { v / 100.0 } else { v };
    let guard = |k: &str, l: &str| NewGuardrail {
        kpi: k.to_owned(),
        limit: l.trim().parse::<f64>().ok().map(|v| raw(k, v)),
    };
    let input = NewGoal {
        target: raw(&f.kpi, f.target),
        kpi: f.kpi.clone(),
        deadline: Some(f.deadline.clone()),
        releases: f.releases.trim().parse().ok(),
        owner: f.owner.clone(),
        guardrails: vec![
            guard(&f.guard1, &f.guard1_limit),
            guard(&f.guard2, &f.guard2_limit),
        ],
    };
    match cmd::set_evaluated(&state.meta, &p.id, &ev, input) {
        Ok(_) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "goal.set", "");
            Redirect::to(&format!("/p/{}/goals", p.id)).into_response()
        }
        Err(e) => oops(&e.to_string()),
    }
}

#[derive(Deserialize)]
pub(crate) struct NewObjective {
    title: String,
    #[serde(default)]
    goal1: String,
    #[serde(default)]
    goal2: String,
    #[serde(default)]
    goal3: String,
    end: String,
}

/// `POST /p/{id}/objectives` (admins, #272): `goals.set_objective`.
pub(crate) async fn create_objective(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<NewObjective>,
) -> Response {
    let input = cmd::SetObjectiveInput {
        project: id,
        title: f.title,
        goals: vec![f.goal1, f.goal2, f.goal3],
        end: f.end,
    };
    let project = input.project.clone();
    match cmd::SetObjective::run(&state, &Caller::default(), input).await {
        Ok(_) => {
            crate::privacy::audit(&state, &headers, Some(&project), "objective.set", "");
            Redirect::to(&format!("/p/{project}/goals")).into_response()
        }
        Err(e) => {
            let m = match e {
                Error::NotFound => "No such project.".to_owned(),
                e => e.to_string(),
            };
            html_page(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't set that objective", &m),
            )
        }
    }
}

/// `POST /p/{id}/objectives/{oid}/delete` (admins): `goals.delete_objective`.
pub(crate) async fn delete_objective(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, oid)): Path<(String, String)>,
) -> Response {
    let input = cmd::DeleteObjectiveInput {
        project: id.clone(),
        objective: oid.clone(),
    };
    let _ = cmd::DeleteObjective::run(&state, &Caller::default(), input).await;
    crate::privacy::audit(&state, &headers, Some(&id), "objective.remove", &oid);
    Redirect::to(&format!("/p/{id}/goals")).into_response()
}

/// `POST /p/{id}/goals/{gid}/delete` (admins): `goals.delete`.
pub(crate) async fn delete(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, gid)): Path<(String, String)>,
) -> Response {
    let input = cmd::DeleteGoalInput {
        project: id.clone(),
        goal: gid.clone(),
    };
    let _ = cmd::Delete::run(&state, &Caller::default(), input).await;
    crate::privacy::audit(&state, &headers, Some(&id), "goal.remove", &gid);
    Redirect::to(&format!("/p/{id}/goals")).into_response()
}

/// `GET /api/projects/{id}/goals` (bearer): `goals.list`, every goal with
/// its current value, projection and status, and the suggested targets.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = cmd::ProjectInput { project: id };
    match cmd::List::run(&state, &Caller::default(), input).await {
        Ok(ev) => Json(ev).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// Set a goal from the Ask chat once a person confirmed it (#244):
/// `goals.set`, the same checks as the page and the API.
pub(crate) async fn set_confirmed(
    state: &AppState,
    p: &openlens_meta::Project,
    kpi: &str,
    target: f64,
    deadline: &str,
) -> Result<Goal, String> {
    let input = cmd::SetGoalInput {
        project: p.id.clone(),
        goal: NewGoal {
            kpi: kpi.to_owned(),
            target,
            deadline: Some(deadline.to_owned()),
            releases: None,
            owner: String::new(),
            guardrails: Vec::new(),
        },
    };
    cmd::Set::run(state, &Caller::default(), input)
        .await
        .map_err(|e| e.to_string())
}

/// `POST /api/projects/{id}/goals` (bearer, #272): `goals.set`, with the
/// same checks as the page.
pub(crate) async fn api_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(g): Json<NewGoal>,
) -> Response {
    let bad = |s: StatusCode, m: &str| crate::apierr::json(s, m);
    if !auth::has_bearer(&state, &headers) {
        return bad(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = cmd::SetGoalInput {
        project: id.clone(),
        goal: g,
    };
    match cmd::Set::run(&state, &Caller::default(), input).await {
        Ok(goal) => {
            crate::privacy::audit(&state, &headers, Some(&id), "goal.set", "api");
            Json(json!({ "stored": true, "goal": goal })).into_response()
        }
        Err(Error::NotFound) => bad(StatusCode::NOT_FOUND, "unknown project"),
        Err(e @ Error::Unavailable(_)) => bad(StatusCode::BAD_REQUEST, &e.to_string()),
        Err(e @ Error::Invalid(_)) => bad(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()),
        Err(e @ Error::Storage(_)) => bad(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

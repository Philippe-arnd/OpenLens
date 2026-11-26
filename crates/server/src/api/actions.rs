//! REST routes over engine commands that only a page ran before (#525,
//! follow-up of #308): deleting goals, objectives, the alert settings and
//! uptime, the GitHub repository of tickets, the win-back flag of Churn
//! and the explore reports. They exist so that the MCP tools, which are
//! HTTP clients of this API (D-020), reach every command (D-058). Each
//! handler checks the bearer token, builds the command's input and
//! answers its output as JSON; the scope a token needs comes from the
//! method (`crate::scopes`).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use openlens_engine::commands::{Caller, Command, Error, alerts, explore, goals, tickets};
use openlens_meta::Project;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{FilterQuery, err, guard, known_project};
use crate::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/goals/{goal}", delete(goal_remove))
        .route("/api/projects/{project}/objectives", post(objective_set))
        .route(
            "/api/projects/{project}/objectives/{objective}",
            delete(objective_remove),
        )
        .route("/api/projects/{project}/uptime", get(uptime))
        .route(
            "/api/projects/{project}/alerts",
            get(alert_settings).put(alerts_change),
        )
        .route("/api/projects/{project}/alerts/test", post(alerts_test))
        .route("/api/projects/{project}/tickets/target", put(ticket_repo))
        .route(
            "/api/projects/{project}/churn/flags",
            get(churn_flags).post(churn_flag_make),
        )
        .route("/api/projects/{project}/acquisition", get(acquisition))
        .route("/api/projects/{project}/forms", get(forms))
        .route("/api/projects/{project}/audience", get(audience))
        .route("/api/projects/{project}/live", get(live))
}

/// The bearer check, then the project.
async fn open(state: &AppState, headers: &HeaderMap, id: &str) -> Result<Project, Box<Response>> {
    if let Some(r) = guard(state, headers) {
        return Err(Box::new(r));
    }
    known_project(state, id).await
}

/// A command's error as an API answer, as `POST /api/projects/{id}/goals`
/// answers them.
fn failed(e: Error) -> Response {
    match e {
        Error::NotFound => err(StatusCode::NOT_FOUND, "unknown project"),
        Error::Invalid(m) => err(StatusCode::UNPROCESSABLE_ENTITY, &m),
        Error::Unavailable(m) => err(StatusCode::BAD_REQUEST, &m),
        Error::Storage(m) => err(StatusCode::INTERNAL_SERVER_ERROR, &m),
    }
}

fn answer<T: serde::Serialize>(r: Result<T, Error>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => failed(e),
    }
}

/// `DELETE /api/projects/{id}/goals/{goal}`: `goals.delete`.
async fn goal_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, goal)): Path<(String, String)>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = goals::DeleteGoalInput {
        project: p.id.clone(),
        goal: goal.clone(),
    };
    match goals::Delete::run(&state, &Caller::default(), input).await {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "goal.remove", &goal);
            Json(json!({ "removed": goal })).into_response()
        }
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct ObjectiveBody {
    title: String,
    #[serde(default)]
    goals: Vec<String>,
    end: String,
}

/// `POST /api/projects/{id}/objectives`: `goals.set_objective`.
async fn objective_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<ObjectiveBody>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = goals::SetObjectiveInput {
        project: p.id.clone(),
        title: b.title,
        goals: b.goals,
        end: b.end,
    };
    match goals::SetObjective::run(&state, &Caller::default(), input).await {
        Ok(o) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "objective.set", "api");
            Json(json!({ "stored": true, "objective": o })).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `DELETE /api/projects/{id}/objectives/{objective}`:
/// `goals.delete_objective`.
async fn objective_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, objective)): Path<(String, String)>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = goals::DeleteObjectiveInput {
        project: p.id.clone(),
        objective: objective.clone(),
    };
    match goals::DeleteObjective::run(&state, &Caller::default(), input).await {
        Ok(()) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "objective.remove",
                &objective,
            );
            Json(json!({ "removed": objective })).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `GET /api/projects/{id}/uptime`: `alerts.uptime`.
async fn uptime(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = alerts::ProjectInput { project: p.id };
    answer(alerts::GetUptime::run(&state, &Caller::default(), input).await)
}

/// What the API shows of a webhook URL or an ntfy topic that is set: they
/// work as passwords, and a `read` token must not learn them.
const HIDDEN: &str = "(set)";

fn shown(s: &alerts::Settings) -> Value {
    let mut v = json!(s);
    if let Some(o) = v.as_object_mut() {
        for k in ["webhook", "ntfy"] {
            if o.get(k)
                .and_then(Value::as_str)
                .is_some_and(|t| !t.is_empty())
            {
                o.insert(k.into(), json!(HIDDEN));
            }
        }
    }
    v
}

/// `GET /api/projects/{id}/alerts`: `alerts.settings`, the webhook and the
/// ntfy topic hidden.
async fn alert_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = alerts::ProjectInput { project: p.id };
    match alerts::GetSettings::run(&state, &Caller::default(), input).await {
        Ok(s) => Json(shown(&s)).into_response(),
        Err(e) => failed(e),
    }
}

/// The stored settings with the fields of `change` over them. A hidden
/// value sent back unchanged keeps the stored one; an unknown field is
/// refused.
fn merged(stored: &alerts::Settings, change: &Value) -> Result<alerts::Settings, String> {
    let Some(change) = change.as_object() else {
        return Err("send the settings to change as an object".into());
    };
    let mut v = json!(stored);
    let Some(o) = v.as_object_mut() else {
        return Err("settings unreadable".into());
    };
    for (k, val) in change {
        if !o.contains_key(k) {
            return Err(format!("unknown setting `{k}`"));
        }
        if val.as_str() == Some(HIDDEN) {
            continue;
        }
        o.insert(k.clone(), val.clone());
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// `PUT /api/projects/{id}/alerts`: `alerts.set` with the fields sent over
/// the stored settings (the others unchanged).
async fn alerts_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(change): Json<Value>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let caller = Caller::default();
    let stored = match alerts::GetSettings::run(
        &state,
        &caller,
        alerts::ProjectInput {
            project: p.id.clone(),
        },
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return failed(e),
    };
    let settings = match merged(&stored, &change) {
        Ok(s) => s,
        Err(m) => return err(StatusCode::UNPROCESSABLE_ENTITY, &m),
    };
    let input = alerts::SetSettingsInput {
        project: p.id.clone(),
        settings: settings.clone(),
    };
    match alerts::SetSettings::run(&state, &caller, input).await {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "alerts.update", "api");
            Json(json!({ "stored": true, "settings": shown(&settings) })).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `POST /api/projects/{id}/alerts/test`: `alerts.test`, one message on
/// every channel set; `sent` is empty when none is.
async fn alerts_test(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = alerts::ProjectInput { project: p.id };
    match alerts::Test::run(&state, &Caller::default(), input).await {
        Ok(sent) => Json(json!({ "sent": sent })).into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct RepoBody {
    #[serde(default)]
    repo: String,
}

/// `PUT /api/projects/{id}/tickets/target`: `tickets.target`, the GitHub
/// repository only. The token is shared by every project, so it is set
/// on the Tickets page, not with a project's token.
async fn ticket_repo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<RepoBody>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = tickets::TargetInput {
        project: p.id.clone(),
        repo: b.repo,
        token: String::new(),
    };
    match tickets::SetTarget::run(&state, &Caller::default(), input).await {
        Ok(repo) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "tickets.target", &repo);
            Json(json!({ "repo": repo })).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `GET /api/projects/{id}/churn/flags`: `explore.flag_audiences`, the
/// flags made from the users at risk and how many of each group were
/// active in the last 7 days.
async fn churn_flags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = explore::ProjectInput { project: p.id };
    match explore::FlagAudiences::run(&state, &Caller::default(), input).await {
        Ok(flags) => Json(json!({ "flags": flags })).into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct HoldoutBody {
    #[serde(default)]
    holdout: Option<u8>,
}

/// `POST /api/projects/{id}/churn/flags`: `explore.make_audience`, a flag,
/// off, listing the users at risk now with a control group.
async fn churn_flag_make(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<HoldoutBody>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = explore::MakeAudienceInput {
        project: p.id.clone(),
        holdout: b.holdout,
    };
    match explore::MakeAudience::run(&state, &Caller::default(), input).await {
        Ok(made) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "flag.save", &made.key);
            Json(json!({ "flag": made.key, "enabled": false })).into_response()
        }
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct AcquisitionQuery {
    #[serde(flatten)]
    f: FilterQuery,
    by: Option<String>,
    touch: Option<String>,
}

/// `GET /api/projects/{id}/acquisition[?by=&touch=]`:
/// `explore.acquisition`.
async fn acquisition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<AcquisitionQuery>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = explore::AcquisitionInput {
        filters: q.f.filters(&p.timezone),
        project: p.id,
        by: q.by,
        touch: q.touch,
    };
    answer(explore::GetAcquisition::run(&state, &Caller::default(), input).await)
}

fn filtered(p: Project, q: &FilterQuery) -> explore::FilteredInput {
    explore::FilteredInput {
        filters: q.filters(&p.timezone),
        project: p.id,
    }
}

/// `GET /api/projects/{id}/forms`: `explore.forms`.
async fn forms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    match explore::Forms::run(&state, &Caller::default(), filtered(p, &q)).await {
        Ok(forms) => Json(json!({ "forms": forms })).into_response(),
        Err(e) => failed(e),
    }
}

/// `GET /api/projects/{id}/audience`: `explore.audience`; `geo` says
/// whether countries can be known on this server.
async fn audience(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    crate::audience::init(&state.data_dir);
    match explore::GetAudience::run(&state, &Caller::default(), filtered(p, &q)).await {
        Ok(a) => Json(json!({ "audience": a, "geo": crate::audience::has_geo() })).into_response(),
        Err(e) => failed(e),
    }
}

/// `GET /api/projects/{id}/live`: `explore.live`.
async fn live(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    answer(explore::GetLive::run(&state, &Caller::default(), filtered(p, &q)).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_alert_change_keeps_what_it_does_not_name_and_refuses_unknown_fields() {
        let stored = alerts::Settings {
            webhook: "https://hooks.example/secret".into(),
            ..alerts::Settings::default()
        };
        let shown = shown(&stored);
        assert_eq!(shown["webhook"], HIDDEN, "the webhook is not shown");
        // Sending back what was shown keeps the stored secret.
        let mut back = shown.clone();
        back["traffic"] = json!(!stored.traffic);
        let s = merged(&stored, &back).unwrap();
        assert_eq!(s.webhook, stored.webhook);
        assert_ne!(s.traffic, stored.traffic);
        assert_eq!(s.down_wait, stored.down_wait);
        assert!(merged(&stored, &json!({ "nope": true })).is_err());
        assert!(merged(&stored, &json!({ "down_wait": "soon" })).is_err());
    }
}

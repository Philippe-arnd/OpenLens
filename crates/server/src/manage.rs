//! Create, edit and delete flags and surveys from the dashboard (#220),
//! with the same validation as the API and CLI (the models check
//! themselves). Admin-only: the access layer refuses other POSTs.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_flags::Flag;
use openlens_flags::survey::Survey;
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

fn refused() -> Response {
    html_page(
        StatusCode::FORBIDDEN,
        openlens_ui::message("Refused", "Cross-site form."),
    )
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_owned())
        .filter(|x| !x.is_empty())
        .collect()
}

async fn project(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

#[derive(Deserialize, Default)]
pub(crate) struct KeyQuery {
    #[serde(default)]
    key: Option<String>,
}

fn not_found() -> Response {
    html_page(
        StatusCode::NOT_FOUND,
        openlens_ui::message("Not found", "No such project."),
    )
}

/// `GET /p/{id}/flags-edit?key=` (no key: a new flag).
pub(crate) async fn flag_page(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<KeyQuery>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found();
    };
    let flags = crate::flags::load(&state, &p.id).await;
    let current = q
        .key
        .as_deref()
        .and_then(|k| flags.iter().find(|f| f.key == k));
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html_page(StatusCode::OK, openlens_ui::flag_edit(&info, current, None))
}

#[derive(Deserialize)]
pub(crate) struct FlagForm {
    /// Empty for a new flag.
    #[serde(default)]
    original: String,
    key: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    enabled: Option<String>,
    #[serde(default)]
    rollout: Option<u8>,
    #[serde(default)]
    roles: String,
    #[serde(default)]
    accounts: String,
}

/// `POST /p/{id}/flags-edit`: create or update; errors show on the form.
pub(crate) async fn flag_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<FlagForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    let Some(p) = project(&state, &id).await else {
        return not_found();
    };
    let flag = Flag {
        key: f.key.trim().to_owned(),
        description: f.description.trim().to_owned(),
        enabled: f.enabled.as_deref() == Some("on"),
        rollout: f.rollout.unwrap_or(0).min(100),
        roles: list(&f.roles),
        accounts: list(&f.accounts),
    };
    let existing = crate::flags::load(&state, &p.id).await;
    let renamed = !f.original.is_empty() && f.original != flag.key;
    let err = if (f.original.is_empty() || renamed) && existing.iter().any(|x| x.key == flag.key) {
        Some(format!("A flag `{}` already exists.", flag.key))
    } else {
        flag.validate().err()
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    if let Some(e) = err {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::flag_edit(&info, Some(&flag), Some(&e)),
        );
    }
    if let Err(e) = crate::flags::save(&state, &p.id, &flag).await {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::flag_edit(&info, Some(&flag), Some(&e)),
        );
    }
    if renamed {
        let meta = state.meta.clone();
        let (pid, old) = (p.id.clone(), f.original.clone());
        let _ = tokio::task::spawn_blocking(move || meta.delete_flag(&pid, &old)).await;
    }
    crate::privacy::audit(&state, &headers, Some(&p.id), "flag.save", &flag.key);
    Redirect::to(&format!("/p/{}/flags", p.id)).into_response()
}

#[derive(Deserialize, Default)]
pub(crate) struct ConfirmForm {
    #[serde(default)]
    confirm: String,
}

/// `POST /p/{id}/flags/{key}/delete`: the key must be typed again.
pub(crate) async fn flag_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    if f.confirm.trim() != key {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Not deleted", "Type the flag's key exactly to delete it."),
        );
    }
    let meta = state.meta.clone();
    let pid = id.clone();
    let _ = tokio::task::spawn_blocking(move || meta.delete_flag(&pid, &key)).await;
    crate::privacy::audit(&state, &headers, Some(&id), "flag.delete", "");
    Redirect::to(&format!("/p/{id}/flags")).into_response()
}

/// `GET /p/{id}/surveys-edit?key=` (no key: a new survey).
pub(crate) async fn survey_page(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<KeyQuery>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found();
    };
    let all = crate::surveys::load(&state, &p.id).await;
    let current = q
        .key
        .as_deref()
        .and_then(|k| all.iter().find(|s| s.key == k));
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html_page(
        StatusCode::OK,
        openlens_ui::survey_edit(&info, current, None),
    )
}

#[derive(Deserialize)]
pub(crate) struct SurveyForm {
    #[serde(default)]
    original: String,
    key: String,
    question: String,
    kind: String,
    #[serde(default)]
    choices: String,
    trigger: String,
    #[serde(default)]
    enabled: Option<String>,
    #[serde(default)]
    sample: Option<u8>,
    #[serde(default)]
    roles: String,
}

/// `POST /p/{id}/surveys-edit`.
pub(crate) async fn survey_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<SurveyForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    let Some(p) = project(&state, &id).await else {
        return not_found();
    };
    let s = Survey {
        key: f.key.trim().to_owned(),
        question: f.question.trim().to_owned(),
        kind: f.kind.trim().to_owned(),
        choices: if f.kind == "choice" {
            list(&f.choices)
        } else {
            Vec::new()
        },
        trigger: f.trigger.trim().to_owned(),
        enabled: f.enabled.as_deref() == Some("on"),
        sample: f.sample.unwrap_or(0).min(100),
        roles: list(&f.roles),
    };
    let existing = crate::surveys::load(&state, &p.id).await;
    let renamed = !f.original.is_empty() && f.original != s.key;
    let err = if (f.original.is_empty() || renamed) && existing.iter().any(|x| x.key == s.key) {
        Some(format!("A survey `{}` already exists.", s.key))
    } else {
        s.validate().err()
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    if let Some(e) = err {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::survey_edit(&info, Some(&s), Some(&e)),
        );
    }
    if let Err(e) = crate::surveys::save(&state, &p.id, &s).await {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::survey_edit(&info, Some(&s), Some(&e)),
        );
    }
    if renamed {
        let meta = state.meta.clone();
        let (pid, old) = (p.id.clone(), f.original.clone());
        let _ = tokio::task::spawn_blocking(move || meta.delete_survey(&pid, &old)).await;
    }
    crate::privacy::audit(&state, &headers, Some(&p.id), "survey.save", &s.key);
    Redirect::to(&format!("/p/{}/surveys", p.id)).into_response()
}

/// `POST /p/{id}/surveys/{key}/delete`: the key must be typed again (the
/// answers, which are events, stay).
pub(crate) async fn survey_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    if f.confirm.trim() != key {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Not deleted", "Type the survey's key exactly to delete it."),
        );
    }
    let meta = state.meta.clone();
    let pid = id.clone();
    let _ = tokio::task::spawn_blocking(move || meta.delete_survey(&pid, &key)).await;
    crate::privacy::audit(&state, &headers, Some(&id), "survey.delete", "");
    Redirect::to(&format!("/p/{id}/surveys")).into_response()
}

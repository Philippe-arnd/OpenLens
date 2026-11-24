//! The timeline of changes (#267): releases (first seen), flag and survey
//! changes and feature map pushes (from the audit log), alerts (incidents)
//! and notes anyone signed in can add. Shown as markers on the active
//! users chart and on one page.

use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

/// Everything that happened since `days` ago, newest first.
pub(crate) fn collect(
    state: &AppState,
    project: &str,
    releases: &[(String, String)],
    days: i64,
) -> Vec<openlens_ui::Mark> {
    let now = chrono::Utc::now();
    let since_day = (now - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string();
    let since = now.timestamp() - days * 86400;
    let day_of = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .map_or_else(String::new, |d| d.format("%Y-%m-%d").to_string())
    };
    let mut out: Vec<openlens_ui::Mark> = Vec::new();
    for (release, first) in releases {
        let d = first.get(..10).unwrap_or(first).to_owned();
        if d >= since_day {
            out.push(openlens_ui::Mark {
                day: d,
                kind: "release".into(),
                text: format!("Release {release} first seen"),
                id: None,
                by: None,
                author_id: None,
            });
        }
    }
    for (at, actor, _, action, detail) in
        state.meta.project_audit(project, since).unwrap_or_default()
    {
        let text = match action.as_str() {
            "flag.save" => format!("Flag {detail} changed"),
            "flag.delete" => "A flag was deleted".to_owned(),
            "survey.save" => format!("Survey {detail} changed"),
            "survey.delete" => "A survey was deleted".to_owned(),
            "feature_map.save" => format!("Feature map {detail}"),
            "goal.set" => "A goal was set".to_owned(),
            _ => continue,
        };
        out.push(openlens_ui::Mark {
            day: day_of(at),
            kind: "change".into(),
            text,
            id: None,
            by: Some(actor),
            author_id: None,
        });
    }
    for (rule, start, end, cause, _) in state.meta.incidents(project, since).unwrap_or_default() {
        let what = match rule.as_str() {
            "down" => "App down",
            "traffic" => "Traffic stopped",
            r if r.starts_with("error:") => "New error",
            _ => "Regression",
        };
        let mins = (end.unwrap_or(now.timestamp()) - start).max(0) / 60;
        let text = format!(
            "{what}: {}{}",
            cause.chars().take(140).collect::<String>(),
            if end.is_some() && rule != "down" && rule != "traffic" {
                String::new()
            } else {
                format!(" ({mins} min)")
            }
        );
        out.push(openlens_ui::Mark {
            day: day_of(start),
            kind: "alert".into(),
            text,
            id: None,
            by: None,
            author_id: None,
        });
    }
    for (id, day, author, author_id, body) in
        state.meta.notes(project, &since_day).unwrap_or_default()
    {
        out.push(openlens_ui::Mark {
            day,
            kind: "note".into(),
            text: body,
            id: Some(id),
            by: Some(author),
            author_id: Some(author_id),
        });
    }
    out.sort_by(|a, b| b.day.cmp(&a.day).then(a.kind.cmp(&b.kind)));
    out
}

/// `GET /p/{id}/timeline`.
pub(crate) async fn page(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Ok(Some(p)) = state.meta.project(&id) else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let map = crate::load_map(&state, &p.id).await;
    let f = openlens_engine::Filters {
        tz: p.timezone.clone(),
        ..openlens_engine::Filters::default()
    };
    let releases: Vec<(String, String)> =
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
            .await
            .map(|b| {
                b.releases
                    .iter()
                    .map(|r| (r.release.clone(), r.first_seen.clone()))
                    .collect()
            })
            .unwrap_or_default();
    let marks = collect(&state, &p.id, &releases, 90);
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    html_page(
        StatusCode::OK,
        openlens_ui::timeline(&info, &marks, &v.id, v.admin, &today),
    )
}

#[derive(Deserialize)]
pub(crate) struct NoteForm {
    day: String,
    body: String,
}

fn author(state: &AppState, v: &auth::Viewer) -> String {
    if v.id == auth::OWNER {
        return "owner".into();
    }
    state
        .meta
        .user(&v.id)
        .ok()
        .flatten()
        .map_or_else(|| v.id.clone(), |u| u.name.unwrap_or(u.email))
}

/// `POST /p/{id}/timeline` (anyone signed in): add a note.
pub(crate) async fn add(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(f): Form<NoteForm>,
) -> Response {
    match state
        .meta
        .add_note(&id, f.day.trim(), &v.id, &author(&state, &v), &f.body)
    {
        Ok(_) => Redirect::to(&format!("/p/{id}/timeline")).into_response(),
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't add that note", &e.to_string()),
        ),
    }
}

/// `POST /p/{id}/timeline/{nid}/delete`: its author or an admin.
pub(crate) async fn delete(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Path((id, nid)): Path<(String, i64)>,
) -> Response {
    match state.meta.delete_note(&id, nid, &v.id, v.admin) {
        Ok(true) => Redirect::to(&format!("/p/{id}/timeline")).into_response(),
        _ => html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message(
                "Not yours",
                "Only its author or an admin can delete a note.",
            ),
        ),
    }
}

#[derive(Deserialize)]
pub(crate) struct ApiNote {
    /// YYYY-MM-DD (default today).
    #[serde(default)]
    day: Option<String>,
    body: String,
}

/// `GET /api/projects/{id}/timeline` (bearer).
pub(crate) async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let marks = collect(&state, &id, &[], 90);
    Json(json!({ "timeline": marks.iter().map(|m| json!({ "day": m.day, "kind": m.kind, "text": m.text })).collect::<Vec<_>>() })).into_response()
}

/// `POST /api/projects/{id}/timeline` with `{day?, body}` (bearer): a note
/// from an agent (for example right after a deploy).
pub(crate) async fn api_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<ApiNote>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let day = b
        .day
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    match state
        .meta
        .add_note(&id, &day, "api", "agent (api)", &b.body)
    {
        Ok(n) => (StatusCode::CREATED, Json(json!({ "id": n, "day": day }))).into_response(),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

//! The project settings page (#219): every action of `openlens-cli init`,
//! `timezone`, `delete-project`, `verify` and `export`, from the browser.
//! Changes are admin-only (the access layer refuses other POSTs under /p/).

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

fn refused() -> Response {
    html_page(
        StatusCode::FORBIDDEN,
        openlens_ui::message("Refused", "Cross-site form."),
    )
}

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
}

async fn load(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

#[derive(Deserialize, Default)]
pub(crate) struct Notice {
    #[serde(default)]
    done: Option<String>,
}

/// `GET /p/{id}/settings`.
pub(crate) async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(nq): Query<Notice>,
) -> Response {
    let Some(viewer) = crate::auth::viewer(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };
    let Some(p) = load(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let app_url = state.meta.project_setting(&p.id, "app_url").ok().flatten();
    let detected = crate::icon::detected_origin(&state, &p.id).await;
    let sql = "SELECT count(*) AS events, \
               count(CASE WHEN event = '$identify' THEN 1 END) AS identified, \
               count(role) AS with_role, count(DISTINCT release) AS releases \
               FROM events WHERE timestamp >= now() - INTERVAL '1 day'";
    let row = state
        .query
        .sql(&p.id, sql)
        .await
        .ok()
        .and_then(|r| r.rows.into_iter().next());
    let n = |k: &str| {
        row.as_ref()
            .and_then(|r| r.get(k))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    let checks = [
        (
            "Events",
            n("events"),
            "add the script tag or an SDK to the app",
        ),
        (
            "Logins (identify)",
            n("identified"),
            "call identify(id, { role }) after sign-in",
        ),
        (
            "Events with a role",
            n("with_role"),
            "pass the user's role to identify",
        ),
        (
            "Releases",
            n("releases"),
            "set data-release (or release) to the app's version",
        ),
    ];
    let map_version = state.meta.feature_map(&p.id).ok().flatten().map(|(v, _)| v);
    let yesterday = {
        let now = chrono::Utc::now() - chrono::Duration::days(1);
        now.format("%Y-%m-%d").to_string()
    };
    let notice = match nq.done.as_deref() {
        Some("saved") => Some("Saved."),
        Some("rotated") => Some("New capture key: update every app that sends events."),
        Some("created") => Some("Project created. Add the snippet below to the app."),
        _ => None,
    };
    let host = crate::base_url(&headers);
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html_page(
        StatusCode::OK,
        openlens_ui::project_settings(
            &info,
            &openlens_ui::SettingsView {
                timezone: &p.timezone,
                app_url: app_url.as_deref(),
                detected_url: detected.as_deref(),
                host: &host,
                checks: &checks,
                map_version,
                admin: viewer.admin,
                notice,
                yesterday: &yesterday,
            },
        ),
    )
}

#[derive(Deserialize)]
pub(crate) struct SaveForm {
    name: String,
    #[serde(default)]
    app_url: String,
    timezone: String,
}

/// `POST /p/{id}/settings`: name, app address, time zone.
pub(crate) async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<SaveForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    let Ok(tz) = crate::api::check_timezone(&f.timezone) else {
        return oops("The time zone must be an IANA name like Europe/Paris.");
    };
    let url = f.app_url.trim().to_owned();
    let app_url = if url.is_empty() {
        String::new()
    } else {
        match crate::icon::origin(&url) {
            Some(o) => o,
            None => return oops("The app address must start with https:// (or http://)."),
        }
    };
    let st = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        let m = &st.meta;
        if !m.rename_project(&id, &f.name)? {
            return Ok(None);
        }
        m.set_project_timezone(&id, &tz)?;
        let before = m.project_setting(&id, "app_url")?.unwrap_or_default();
        if before != app_url {
            m.set_project_setting(&id, "app_url", &app_url)?;
            crate::icon::forget(&st, &id);
        }
        Ok::<_, openlens_meta::MetaError>(Some(id))
    })
    .await;
    match res {
        Ok(Ok(Some(id))) => Redirect::to(&format!("/p/{id}/settings?done=saved")).into_response(),
        Ok(Ok(None)) => html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        ),
        Ok(Err(e)) => oops(&e.to_string()),
        Err(_) => oops("Try again."),
    }
}

#[derive(Deserialize)]
pub(crate) struct ConfirmForm {
    #[serde(default)]
    confirm: String,
}

/// `POST /p/{id}/settings/rotate-key`.
pub(crate) async fn rotate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    if f.confirm != "yes" {
        return oops("Tick the box: the old key stops working at once.");
    }
    let meta = state.meta.clone();
    let pid = id.clone();
    match tokio::task::spawn_blocking(move || meta.rotate_project_key(&pid)).await {
        Ok(Ok(Some(_))) => Redirect::to(&format!("/p/{id}/settings?done=rotated")).into_response(),
        _ => oops("No such project."),
    }
}

/// `POST /p/{id}/settings/delete`: the id must be typed again.
pub(crate) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    if f.confirm.trim() != id {
        return oops("Type the project's id exactly to delete it.");
    }
    let st = state.clone();
    let pid = id.clone();
    match tokio::task::spawn_blocking(move || {
        let r = crate::api::purge_project(&st, &pid);
        crate::icon::forget(&st, &pid);
        r
    })
    .await
    {
        Ok(Ok(_)) => Redirect::to("/projects").into_response(),
        Ok(Err(e)) => oops(&e),
        Err(_) => oops("Try again."),
    }
}

#[derive(Deserialize)]
pub(crate) struct ExportQuery {
    date: String,
}

/// `GET /p/{id}/export?date=YYYY-MM-DD`: one day as an NDJSON download
/// (admins: the export holds every event property).
pub(crate) async fn export(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Response {
    match crate::auth::viewer(&state, &headers) {
        None => return Redirect::to("/login").into_response(),
        Some(v) if !v.admin => {
            return html_page(
                StatusCode::FORBIDDEN,
                openlens_ui::message("Admins only", "Ask an admin for an export."),
            );
        }
        _ => {}
    }
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    let mut r = crate::api::export_day(&state, &p.id, &q.date).await;
    if r.status().is_success()
        && let Ok(v) = axum::http::HeaderValue::from_str(&format!(
            "attachment; filename=\"{}-{}.ndjson\"",
            p.id, q.date
        ))
    {
        r.headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    r
}

#[derive(Deserialize)]
pub(crate) struct NewForm {
    name: String,
    #[serde(default)]
    timezone: String,
}

/// `POST /projects`: a new project (admins), then its settings page.
pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<NewForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return refused();
    }
    match crate::auth::viewer(&state, &headers) {
        None => return Redirect::to("/login").into_response(),
        Some(v) if !v.admin => {
            return html_page(
                StatusCode::FORBIDDEN,
                openlens_ui::message("Admins only", "Ask an admin to create a project."),
            );
        }
        _ => {}
    }
    let tz = if f.timezone.trim().is_empty() {
        None
    } else {
        match crate::api::check_timezone(&f.timezone) {
            Ok(t) => Some(t),
            Err(e) => return oops(e),
        }
    };
    let name = f.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 {
        return oops("The name must be 1-80 characters.");
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || {
        let p = meta.create_project(&name, None)?;
        if let Some(tz) = tz {
            meta.set_project_timezone(&p.id, &tz)?;
        }
        Ok::<_, openlens_meta::MetaError>(p.id)
    })
    .await
    {
        Ok(Ok(id)) => Redirect::to(&format!("/p/{id}/settings?done=created")).into_response(),
        Ok(Err(_)) => oops("A project with that name already exists."),
        Err(_) => oops("Try again."),
    }
}

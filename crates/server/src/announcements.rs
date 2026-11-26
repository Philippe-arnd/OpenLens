//! Status announcements (#562): the Uptime page's section and the page
//! that writes them, and the REST routes the CLI and MCP call. Every
//! action is an `announcements.*` engine command; every write is audited.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use maud::Markup;
use openlens_engine::commands::announcements::{
    self as ann, AddUpdateInput, CloseInput, CreateInput, DeleteInput, ListInput, NO_SUCH,
    SendInput, UpdateInput,
};
use openlens_engine::commands::{Caller, Command, Error};
use openlens_meta::Project;
use openlens_meta::announcements::Announcement;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::web::html_page;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/p/{project}/uptime/announcements", post(page_create))
        .route("/p/{project}/uptime/announcements/new", get(page_new))
        .route("/p/{project}/uptime/announcements/send", post(page_send))
        .route(
            "/p/{project}/uptime/announcements/{id}",
            get(page_edit).post(page_save),
        )
        .route(
            "/p/{project}/uptime/announcements/{id}/updates",
            post(page_update),
        )
        .route(
            "/p/{project}/uptime/announcements/{id}/close",
            post(page_close),
        )
        .route(
            "/p/{project}/uptime/announcements/{id}/delete",
            post(page_delete),
        )
        .route(
            "/api/projects/{project}/announcements",
            get(api_list).post(api_create),
        )
        .route(
            "/api/projects/{project}/announcements/send",
            axum::routing::put(api_send),
        )
        .route(
            "/api/projects/{project}/announcements/{id}",
            axum::routing::patch(api_change).delete(api_delete),
        )
        .route(
            "/api/projects/{project}/announcements/{id}/updates",
            post(api_add_update),
        )
        .route(
            "/api/projects/{project}/announcements/{id}/close",
            post(api_close),
        )
}

/// The audit detail of an announcement: its id and the start of its
/// title.
fn detail(a: &Announcement) -> String {
    let t: String = a.fields.title.chars().take(80).collect();
    format!("{} {t}", a.id)
}

fn when(t: i64, tz: &str) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| {
        openlens_ui::local_time(&d.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)
    })
}

/// `YYYY-MM-DDTHH:MM` in `tz`, for a `datetime-local` field.
fn field_time(t: i64, tz: &str) -> String {
    let zone: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
    chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| {
        d.with_timezone(&zone).format("%Y-%m-%dT%H:%M").to_string()
    })
}

fn updates(a: &Announcement, tz: &str) -> Vec<(String, String, String)> {
    a.updates
        .iter()
        .map(|u| (when(u.at, tz), u.state.clone(), u.body.clone()))
        .collect()
}

fn info(p: &Project) -> openlens_ui::ProjectInfo<'_> {
    openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    }
}

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
}

async fn load(state: &AppState, id: &str) -> Option<Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

/// The Uptime page's announcements section; `None` when they cannot be
/// read (the rest of the page still shows).
pub(crate) async fn section(state: &AppState, p: &Project, tz: &str) -> Option<Markup> {
    let input = ListInput {
        project: p.id.clone(),
        days: None,
    };
    let l = ann::List::run(state, &Caller::default(), input)
        .await
        .ok()?;
    let view = openlens_ui::NoticesView {
        items: l
            .announcements
            .iter()
            .map(|i| {
                let a = &i.announcement;
                openlens_ui::NoticeRow {
                    id: a.id,
                    title: a.fields.title.clone(),
                    body: a.fields.body.clone(),
                    level: a.fields.level.clone(),
                    components: a.fields.components.join(", "),
                    start: when(a.fields.starts_at, tz),
                    end: a.fields.ends_at.map(|e| when(e, tz)),
                    pinned: a.fields.pinned,
                    active: i.active,
                    incident: a.fields.incident,
                    updates: updates(a, tz),
                }
            })
            .collect(),
        offers: l
            .offers
            .into_iter()
            .map(|o| openlens_ui::OfferRow {
                kind: o.kind,
                incident: o.incident.id,
                what: o.incident.cause,
                since: when(o.incident.started_at, tz),
                announcement: o.announcement,
            })
            .collect(),
        send: l.send,
    };
    Some(openlens_ui::notices(&info(p), &view))
}

#[derive(Deserialize, Default)]
pub(crate) struct NewQuery {
    #[serde(default)]
    incident: Option<i64>,
}

/// `GET /p/{id}/uptime/announcements/new[?incident=]`: the form of a new
/// announcement, following an incident when one is named.
pub(crate) async fn page_new(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<NewQuery>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    let incident = q
        .incident
        .and_then(|i| state.meta.incident_ref(&p.id, i).ok().flatten());
    let f = openlens_ui::NoticeForm {
        incident: incident.as_ref().map(|i| i.id),
        incident_text: incident.as_ref().map(|i| i.cause.clone()),
        level: if incident.is_some() {
            "outage".into()
        } else {
            "information".into()
        },
        components: "production".into(),
        ..openlens_ui::NoticeForm::default()
    };
    html_page(StatusCode::OK, openlens_ui::notice_page(&info(&p), &f))
}

/// The fields of the announcement form.
#[derive(Deserialize, Default)]
pub(crate) struct NoticeFields {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    components: String,
    #[serde(default)]
    start: String,
    #[serde(default)]
    end: String,
    #[serde(default)]
    pinned: Option<String>,
    #[serde(default)]
    incident: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

fn components(raw: &str) -> Vec<String> {
    raw.split(',').map(|c| c.trim().to_owned()).collect()
}

/// The form again, with what was typed and why it was refused.
fn refused(p: &Project, f: &NoticeFields, id: Option<i64>, e: &Error) -> Response {
    let form = openlens_ui::NoticeForm {
        id,
        incident: f.incident.as_deref().and_then(|i| i.trim().parse().ok()),
        title: f.title.clone(),
        body: f.body.clone(),
        level: f.level.clone(),
        components: f.components.clone(),
        start: f.start.clone(),
        end: f.end.clone(),
        pinned: f.pinned.is_some(),
        active: true,
        error: Some(e.to_string()),
        ..openlens_ui::NoticeForm::default()
    };
    html_page(
        StatusCode::UNPROCESSABLE_ENTITY,
        openlens_ui::notice_page(&info(p), &form),
    )
}

fn back(p: &Project) -> Response {
    Redirect::to(&format!("/p/{}/uptime", p.id)).into_response()
}

/// `POST /p/{id}/uptime/announcements`: `announcements.create`.
pub(crate) async fn page_create(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<NoticeFields>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    let input = CreateInput {
        project: p.id.clone(),
        title: f.title.clone(),
        body: f.body.clone(),
        level: Some(f.level.clone()).filter(|l| !l.is_empty()),
        components: components(&f.components),
        start: Some(f.start.clone()),
        end: Some(f.end.clone()),
        pinned: f.pinned.is_some(),
        incident: f.incident.as_deref().and_then(|i| i.trim().parse().ok()),
        state: f.state.clone(),
    };
    let caller = crate::commands::caller(&state, &headers);
    match ann::Create::run(&state, &caller, input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "announcement.create",
                &detail(&a),
            );
            back(&p)
        }
        Err(e) => refused(&p, &f, None, &e),
    }
}

/// `GET /p/{id}/uptime/announcements/{a}`: edit an announcement, write an
/// update, close or delete it.
pub(crate) async fn page_edit(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    let Some(a) = state.meta.announcement(&p.id, a).ok().flatten() else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such announcement."),
        );
    };
    let tz = crate::auth::tz_for(&state, &headers, &p.timezone);
    let now = chrono::Utc::now().timestamp();
    let incident = a
        .fields
        .incident
        .and_then(|i| state.meta.incident_ref(&p.id, i).ok().flatten());
    let f = openlens_ui::NoticeForm {
        id: Some(a.id),
        incident: None,
        incident_text: incident.map(|i| i.cause),
        title: a.fields.title.clone(),
        body: a.fields.body.clone(),
        level: a.fields.level.clone(),
        components: a.fields.components.join(", "),
        start: field_time(a.fields.starts_at, &tz),
        end: a
            .fields
            .ends_at
            .map_or_else(String::new, |e| field_time(e, &tz)),
        pinned: a.fields.pinned,
        updates: updates(&a, &tz),
        active: ann::active(&a, now) && a.fields.level != "resolved",
        error: None,
    };
    html_page(StatusCode::OK, openlens_ui::notice_page(&info(&p), &f))
}

/// `POST /p/{id}/uptime/announcements/{a}`: `announcements.update`.
pub(crate) async fn page_save(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
    Form(f): Form<NoticeFields>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    let input = UpdateInput {
        project: p.id.clone(),
        id: a,
        title: Some(f.title.clone()),
        body: Some(f.body.clone()),
        level: Some(f.level.clone()).filter(|l| !l.is_empty()),
        components: Some(components(&f.components)),
        start: Some(f.start.clone()),
        end: Some(f.end.clone()),
        pinned: Some(f.pinned.is_some()),
        incident: None,
    };
    let caller = crate::commands::caller(&state, &headers);
    match ann::Update::run(&state, &caller, input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "announcement.update",
                &detail(&a),
            );
            back(&p)
        }
        Err(e) => refused(&p, &f, Some(a), &e),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct UpdateFields {
    #[serde(default)]
    state: String,
    #[serde(default)]
    body: String,
}

/// `POST /p/{id}/uptime/announcements/{a}/updates`:
/// `announcements.add_update`.
pub(crate) async fn page_update(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
    Form(f): Form<UpdateFields>,
) -> Response {
    let input = AddUpdateInput {
        project: id.clone(),
        id: a,
        state: f.state,
        body: f.body,
    };
    match ann::AddUpdate::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&id),
                "announcement.add_update",
                &detail(&a),
            );
            Redirect::to(&format!("/p/{id}/uptime")).into_response()
        }
        Err(e) => oops(&e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct CloseFields {
    #[serde(default)]
    body: Option<String>,
}

/// `POST /p/{id}/uptime/announcements/{a}/close`: `announcements.close`
/// (*Mark resolved*).
pub(crate) async fn page_close(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
    Form(f): Form<CloseFields>,
) -> Response {
    let input = CloseInput {
        project: id.clone(),
        id: a,
        body: f.body,
    };
    match ann::Close::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&id),
                "announcement.close",
                &detail(&a),
            );
            Redirect::to(&format!("/p/{id}/uptime")).into_response()
        }
        Err(e) => oops(&e.to_string()),
    }
}

/// `POST /p/{id}/uptime/announcements/{a}/delete`: `announcements.delete`.
pub(crate) async fn page_delete(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
) -> Response {
    let input = DeleteInput {
        project: id.clone(),
        id: a,
    };
    match ann::Delete::run(&state, &Caller::default(), input).await {
        Ok(()) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&id),
                "announcement.delete",
                &a.to_string(),
            );
            Redirect::to(&format!("/p/{id}/uptime")).into_response()
        }
        Err(e) => oops(&e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct SendFields {
    #[serde(default)]
    send: String,
}

/// `POST /p/{id}/uptime/announcements/send`: `announcements.send`.
pub(crate) async fn page_send(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<SendFields>,
) -> Response {
    let input = SendInput {
        project: id.clone(),
        send: f.send == "on",
    };
    match ann::SetSend::run(&state, &Caller::default(), input).await {
        Ok(on) => {
            let d = if on { "on" } else { "off" };
            crate::privacy::audit(&state, &headers, Some(&id), "announcement.send", d);
            Redirect::to(&format!("/p/{id}/uptime")).into_response()
        }
        Err(e) => oops(&e.to_string()),
    }
}

// The REST routes (bearer; the scope comes from the method, `scopes.rs`).

async fn open(state: &AppState, headers: &HeaderMap, id: &str) -> Result<Project, Box<Response>> {
    if let Some(r) = crate::api::guard(state, headers) {
        return Err(Box::new(r));
    }
    crate::api::known_project(state, id).await
}

fn failed(e: Error) -> Response {
    let (status, m) = match e {
        Error::NotFound => (StatusCode::NOT_FOUND, "unknown project".to_owned()),
        Error::Invalid(m) if m == NO_SUCH => (StatusCode::NOT_FOUND, m),
        Error::Invalid(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
        Error::Unavailable(m) => (StatusCode::BAD_REQUEST, m),
        Error::Storage(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
    };
    crate::apierr::json(status, m)
}

/// A JSON body as the command's input, with the project of the path.
fn body_as<T: serde::de::DeserializeOwned>(
    mut v: Value,
    project: &str,
) -> Result<T, Box<Response>> {
    if let Some(o) = v.as_object_mut() {
        o.insert("project".into(), json!(project));
    }
    serde_json::from_value(v).map_err(|e| {
        Box::new(crate::apierr::json(
            StatusCode::UNPROCESSABLE_ENTITY,
            e.to_string(),
        ))
    })
}

#[derive(Deserialize, Default)]
pub(crate) struct ListQuery {
    #[serde(default)]
    days: Option<i64>,
}

/// `GET /api/projects/{id}/announcements[?days=]`: `announcements.list`.
async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = ListInput {
        project: p.id,
        days: q.days,
    };
    match ann::List::run(&state, &Caller::default(), input).await {
        Ok(l) => Json(l).into_response(),
        Err(e) => failed(e),
    }
}

/// `POST /api/projects/{id}/announcements`: `announcements.create`.
async fn api_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input: CreateInput = match body_as(v, &p.id) {
        Ok(i) => i,
        Err(r) => return *r,
    };
    match ann::Create::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "announcement.create",
                &detail(&a),
            );
            (StatusCode::CREATED, Json(a)).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `PATCH /api/projects/{id}/announcements/{a}`: `announcements.update`.
async fn api_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
    Json(mut v): Json<Value>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("id".into(), json!(a));
    }
    let input: UpdateInput = match body_as(v, &p.id) {
        Ok(i) => i,
        Err(r) => return *r,
    };
    match ann::Update::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "announcement.update",
                &detail(&a),
            );
            Json(a).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `POST /api/projects/{id}/announcements/{a}/updates`:
/// `announcements.add_update`.
async fn api_add_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
    Json(mut v): Json<Value>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("id".into(), json!(a));
    }
    let input: AddUpdateInput = match body_as(v, &p.id) {
        Ok(i) => i,
        Err(r) => return *r,
    };
    match ann::AddUpdate::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "announcement.add_update",
                &detail(&a),
            );
            Json(a).into_response()
        }
        Err(e) => failed(e),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct CloseBody {
    #[serde(default)]
    body: Option<String>,
}

/// `POST /api/projects/{id}/announcements/{a}/close`:
/// `announcements.close`.
async fn api_close(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
    raw: axum::body::Bytes,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    // The body is optional: none, or `{"body": "Fixed."}`.
    let body = if raw.iter().all(u8::is_ascii_whitespace) {
        None
    } else {
        match serde_json::from_slice::<CloseBody>(&raw) {
            Ok(b) => b.body,
            Err(e) => {
                return crate::apierr::json(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
            }
        }
    };
    let input = CloseInput {
        project: p.id.clone(),
        id: a,
        body,
    };
    match ann::Close::run(&state, &Caller::default(), input).await {
        Ok(a) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "announcement.close",
                &detail(&a),
            );
            Json(a).into_response()
        }
        Err(e) => failed(e),
    }
}

/// `DELETE /api/projects/{id}/announcements/{a}`: `announcements.delete`.
async fn api_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, a)): Path<(String, i64)>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = DeleteInput {
        project: p.id.clone(),
        id: a,
    };
    match ann::Delete::run(&state, &Caller::default(), input).await {
        Ok(()) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&p.id),
                "announcement.delete",
                &a.to_string(),
            );
            Json(json!({ "removed": a })).into_response()
        }
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
pub(crate) struct SendBody {
    send: bool,
}

/// `PUT /api/projects/{id}/announcements/send`: `announcements.send`.
async fn api_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<SendBody>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = SendInput {
        project: p.id.clone(),
        send: b.send,
    };
    match ann::SetSend::run(&state, &Caller::default(), input).await {
        Ok(on) => {
            let d = if on { "on" } else { "off" };
            crate::privacy::audit(&state, &headers, Some(&p.id), "announcement.send", d);
            Json(json!({ "send": on })).into_response()
        }
        Err(e) => failed(e),
    }
}

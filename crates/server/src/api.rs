//! JSON API for agents, the CLI and the MCP server. Every route needs the
//! admin bearer token.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use openlens_engine::Filters;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::board_query::BoardQuery;
use crate::{AppState, auth};

mod tokens;
mod transfer;
use tokens::*;
pub(crate) use transfer::*;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{project}",
            axum::routing::delete(delete_project).patch(update_project),
        )
        .route("/api/projects/{project}/sql", post(sql))
        .route(
            "/api/projects/{project}/environments",
            get(crate::environments::api_list).post(crate::environments::api_relabel),
        )
        .route(
            "/api/projects/{project}/environments/allow",
            post(crate::allowlist::api_allow),
        )
        .route("/api/projects/{project}/board", get(board))
        .route(
            "/api/projects/{project}/board/estimate",
            get(crate::board_query::api_estimate),
        )
        .route("/api/projects/{project}/errors", get(errors))
        .route("/api/projects/{project}/breakdown", get(breakdown))
        .route("/api/projects/{project}/people/{distinct_id}", get(person))
        .route(
            "/api/projects/{project}/sourcemaps/{release}/{bundle}",
            axum::routing::put(put_sourcemap).layer(axum::extract::DefaultBodyLimit::max(
                openlens_errors::MAX_MAP_BYTES,
            )),
        )
        .route(
            "/api/projects/{project}/feature-map",
            get(get_map).put(put_map),
        )
        .route(
            "/api/projects/{project}/feature-map/health",
            get(crate::features::api_health),
        )
        .route(
            "/api/projects/{project}/feature-map/relations",
            get(crate::features::api_relations),
        )
        .route(
            "/api/projects/{project}/feature-map/versions",
            get(crate::features::api_versions),
        )
        .route(
            "/api/projects/{project}/feature-map/features/{key}",
            get(crate::features::api_feature),
        )
        .route(
            "/api/projects/{project}/tickets",
            get(crate::tickets::api_list).post(crate::tickets::api_create),
        )
        .route("/api/feature-map/validate", post(validate_map))
        .route(
            "/api/projects/{project}/import",
            post(import).layer(axum::extract::DefaultBodyLimit::max(IMPORT_MAX)),
        )
        .route("/api/projects/{project}/export", get(export))
        .route(
            "/api/projects/{project}/replays/{session}/comments",
            get(crate::comments::api_list).post(crate::comments::api_add),
        )
        .route(
            "/api/projects/{project}/flows",
            get(crate::explore::api_flows),
        )
        .route(
            "/api/projects/{project}/status",
            get(crate::settings::api_status),
        )
        .route(
            "/api/projects/{project}/vitals",
            get(crate::explore::api_vitals),
        )
        .route(
            "/api/projects/{project}/churn",
            get(crate::explore::api_churn),
        )
        .route(
            "/api/projects/{project}/goals",
            get(crate::goals::api).post(crate::goals::api_set),
        )
        .route(
            "/api/projects/{project}/people/{who}/forget",
            axum::routing::delete(crate::privacy::api_forget),
        )
        .route("/api/audit", get(crate::privacy::api_audit))
        .route("/api/tokens", get(list_tokens).post(create_token))
        .route("/api/tokens/{id}", axum::routing::delete(revoke_token))
        .route(
            "/api/projects/{project}/timeline",
            get(crate::timeline::api_list).post(crate::timeline::api_add),
        )
        .route("/api/login-links", post(login_link))
        .route("/api/users", get(list_users))
        .route(
            "/api/users/{id}",
            axum::routing::delete(delete_user).patch(update_user),
        )
}

/// Largest import chunk, in bytes (the CLI sends ~5,000 lines per chunk).
const IMPORT_MAX: usize = 16 * 1024 * 1024;
/// Largest number of lines in one import chunk.
const IMPORT_MAX_LINES: usize = 20_000;

fn err(status: StatusCode, msg: &str) -> Response {
    crate::apierr::json(status, msg)
}

pub(crate) fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if auth::has_bearer(state, headers) {
        None
    } else {
        Some(err(StatusCode::UNAUTHORIZED, "admin bearer token required"))
    }
}

pub(crate) async fn known_project(
    state: &AppState,
    id: &str,
) -> Result<openlens_meta::Project, Box<Response>> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    match tokio::task::spawn_blocking(move || meta.project(&id)).await {
        Ok(Ok(Some(p))) => Ok(p),
        Ok(Ok(None)) => Err(Box::new(err(StatusCode::NOT_FOUND, "unknown project"))),
        _ => Err(Box::new(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ))),
    }
}

async fn list_projects(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    // A token limited to some projects lists only those (review of v0.3.0).
    let only = crate::scopes::limited_to(&state, &headers);
    match tokio::task::spawn_blocking(move || meta.projects()).await {
        Ok(Ok(ps)) => Json(json!({
            "projects": ps.iter().filter(|p| only.as_ref().is_none_or(|l| l.contains(&p.id))).map(|p| json!({"id": p.id, "name": p.name, "key": p.api_key, "timezone": p.timezone})).collect::<Vec<_>>()
        }))
        .into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "metadata store unavailable"),
    }
}

#[derive(Deserialize)]
struct NewProject {
    name: String,
    /// IANA time zone of the team (D-017); `UTC` when absent.
    timezone: Option<String>,
}

/// A valid IANA time zone name (`Europe/Paris`, `UTC`), or an error message.
pub(crate) fn check_timezone(tz: &str) -> Result<String, &'static str> {
    let tz = tz.trim();
    tz.parse::<chrono_tz::Tz>()
        .map(|_| tz.to_owned())
        .map_err(|_| "timezone must be an IANA name like Europe/Paris or America/New_York")
}

#[derive(Deserialize)]
struct ProjectUpdate {
    timezone: Option<String>,
    /// Environment host rules (#410, D-034).
    #[serde(flatten)]
    rules: crate::environments::RuleChange,
}

/// `PATCH /api/projects/{id}` with `{"timezone": "Europe/Paris"}` (where
/// the project's days and weeks start, D-017) and/or the environment
/// changes of [`crate::environments::RuleChange`] (D-034).
async fn update_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Json(body): Json<ProjectUpdate>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    if body.timezone.is_none() && body.rules.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "nothing to change: send timezone or an environment change (environment_rule, environment_alias, hide_environment, environment_monitor, ...)",
        );
    }
    // A bad time zone changes nothing, rules included.
    if let Some(Err(e)) = body.timezone.as_deref().map(check_timezone) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if !body.rules.is_empty() {
        if let Err(r) = known_project(&state, &project).await {
            return *r;
        }
        let (st, id) = (state.clone(), project.clone());
        let changed =
            tokio::task::spawn_blocking(move || crate::environments::change(&st, &id, &body.rules))
                .await;
        let rules = match changed {
            Ok(Ok(r)) => r,
            Ok(Err((status, msg))) => return err(status, &msg),
            Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "update failed"),
        };
        crate::privacy::audit(&state, &headers, Some(&project), "environments.set", "api");
        let Some(tz) = body.timezone else {
            let mut v = crate::environments::as_json(&rules);
            v["id"] = json!(project);
            return Json(v).into_response();
        };
        return set_timezone(&state, project, &tz, Some(rules)).await;
    }
    let Some(tz) = body.timezone else {
        return err(StatusCode::BAD_REQUEST, "nothing to change");
    };
    set_timezone(&state, project, &tz, None).await
}

async fn set_timezone(
    state: &AppState,
    project: String,
    tz: &str,
    rules: Option<openlens_engine::environments::Rules>,
) -> Response {
    let tz = match check_timezone(tz) {
        Ok(t) => t,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let meta = state.meta.clone();
    let id = project.clone();
    let t = tz.clone();
    match tokio::task::spawn_blocking(move || meta.set_project_timezone(&id, &t)).await {
        Ok(Ok(true)) => {
            let mut v = rules.map_or_else(|| json!({}), |r| crate::environments::as_json(&r));
            (v["id"], v["timezone"]) = (json!(project), json!(tz));
            Json(v).into_response()
        }
        Ok(Ok(false)) => err(StatusCode::NOT_FOUND, "no such project"),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, &e.to_string()),
        Err(_) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

#[derive(Deserialize)]
struct ConfirmQuery {
    confirm: Option<String>,
}

/// `DELETE /api/projects/{id}?confirm={id}`: delete a project and all its
/// data (events, replays, source maps, people, flags, surveys, feature
/// map). Irreversible, so the id must be repeated in `confirm`.
async fn delete_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<ConfirmQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    if q.confirm.as_deref() != Some(project.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            "deleting a project removes all its data: repeat its id in ?confirm=<id>",
        );
    }
    let st = state.clone();
    let pid = project.clone();
    let res = tokio::task::spawn_blocking(move || purge_project(&st, &project)).await;
    match res {
        Ok(Ok(true)) => {
            crate::privacy::audit(&state, &headers, Some(&pid), "project.delete", "(api)");
            Json(json!({ "deleted": true })).into_response()
        }
        Ok(Ok(false)) => err(StatusCode::NOT_FOUND, "no such project"),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, &e),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "delete failed"),
    }
}

async fn create_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewProject>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    if body.name.trim().is_empty() || body.name.len() > 80 {
        return err(StatusCode::BAD_REQUEST, "name must be 1-80 characters");
    }
    let tz = match body.timezone.as_deref().map(check_timezone).transpose() {
        Ok(t) => t,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let meta = state.meta.clone();
    let host = crate::public_url(&headers);
    match tokio::task::spawn_blocking(move || {
        let mut p = meta.create_project(body.name.trim(), None)?;
        if let Some(tz) = tz {
            meta.set_project_timezone(&p.id, &tz)?;
            p.timezone = tz;
        }
        Ok::<_, openlens_meta::MetaError>(p)
    })
    .await
    {
        Ok(Ok(p)) => (
            StatusCode::CREATED,
            Json(json!({
                "id": p.id, "name": p.name, "key": p.api_key, "timezone": p.timezone,
                "snippet": format!(r#"<script src="{host}/c.js" data-key="{}" defer></script>"#, p.api_key),
                "guide": format!("{host}/agents.md"),
            })),
        )
            .into_response(),
        Ok(Err(e)) => err(StatusCode::CONFLICT, &e.to_string()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "metadata store unavailable"),
    }
}

/// Delete a project and every file of it (blocking): events, replays,
/// source maps and its metadata. False when it did not exist.
pub(crate) fn purge_project(st: &AppState, project: &str) -> Result<bool, String> {
    // Events still in the write-ahead log go to their files first, so
    // nothing of this project is written back after the delete.
    crate::flush_now(st)?;
    let existed = st.meta.delete_project(project).map_err(|e| e.to_string())?;
    if existed {
        let mut dirs = vec![
            st.data_dir
                .join("replay")
                .join(format!("project={project}")),
            st.data_dir
                .join("sourcemaps")
                .join(format!("project={project}")),
        ];
        if let Ok(d) = openlens_storage::project_dir(&st.data_dir, project) {
            dirs.push(d);
        }
        if let Ok(d) = openlens_storage::rollup::rollups_dir(&st.data_dir, project) {
            dirs.push(d);
        }
        for d in dirs {
            if d.exists() {
                std::fs::remove_dir_all(&d).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(existed)
}

#[derive(Deserialize)]
struct SqlBody {
    sql: String,
}

async fn sql(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Json(body): Json<SqlBody>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    // Free-form SQL is a cold query: one at a time (#326).
    match state.query.cold().sql(&p.id, &body.sql).await {
        Ok(res) => Json(res).into_response(),
        Err(e @ openlens_query::QueryError::Busy) => {
            let mut r = crate::apierr::with_code(StatusCode::SERVICE_UNAVAILABLE, "busy", &e);
            r.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("5"),
            );
            r
        }
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct FilterQuery {
    pub(crate) env: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) release: Option<String>,
    pub(crate) event: Option<String>,
    /// `1` or `true`: include the team's own users.
    pub(crate) internal: Option<String>,
    /// Read times and days in this IANA zone instead (#215; the CLI's
    /// `--tz` / `OPENLENS_TZ`).
    pub(crate) tz: Option<String>,
}

impl FilterQuery {
    pub(crate) fn filters(&self, tz: &str) -> Filters {
        let some = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Filters {
            environment: some(&self.env).unwrap_or_else(|| "production".into()),
            role: some(&self.role),
            release: some(&self.release),
            internal: matches!(self.internal.as_deref(), Some("1" | "true" | "on")),
            tz: some(&self.tz)
                .filter(|t| t.parse::<chrono_tz::Tz>().is_ok())
                .unwrap_or_else(|| tz.to_owned()),
            window: openlens_engine::Window::Fixed,
        }
    }
}

async fn board(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<BoardQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let f = match q.filters(&p.timezone) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let map = crate::load_map(&state, &p.id).await;
    let map = map.as_ref().map(|(m, v)| (m, *v));
    // A long date range or past release runs once confirmed (D-061).
    match crate::board_query::unconfirmed(&state, &p.id, &q, &f, map).await {
        Ok(None) => {}
        Ok(Some(e)) => return crate::board_query::conflict(&e),
        Err(e) => return err(StatusCode::BAD_REQUEST, &e),
    }
    match crate::boards::board(&state, &p.id, &f, map).await {
        Ok(b) => Json(b).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize, Default)]
struct LinkRequest {
    email: Option<String>,
    role: Option<String>,
    projects: Option<Vec<String>>,
}

/// `POST /api/login-links`: a one-time link. Without a body it logs in as the
/// owner; with `{email, role, projects?}` it invites (or updates) that user
/// and logs in as them (D-016).
async fn login_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let req: LinkRequest = if body.is_empty() {
        LinkRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(_) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "body must be {email, role, projects?}",
                );
            }
        }
    };
    let meta = state.meta.clone();
    let host = crate::public_url(&headers);
    let res = tokio::task::spawn_blocking(move || {
        let user = match &req.email {
            Some(email) => Some(meta.upsert_user(
                email,
                req.role.as_deref().unwrap_or("viewer"),
                req.projects.as_deref(),
            )?),
            None => None,
        };
        let t = meta.create_login_link_for(
            user.as_ref().map(|u| u.id.as_str()),
            // An owner link opens everything: one day (#315). Invites: a week.
            if user.is_some() {
                7 * 24 * 3600
            } else {
                24 * 3600
            },
        )?;
        Ok::<_, openlens_meta::MetaError>((user, t))
    })
    .await;
    match res {
        Ok(Ok((user, t))) => {
            let who = user
                .as_ref()
                .map_or_else(|| "owner".to_owned(), |u| u.email.clone());
            crate::privacy::audit(
                &state,
                &headers,
                None,
                "user.invite",
                &format!("login link for {who} (api)"),
            );
            Json(json!({
                "url": format!("{host}/l/{t}"),
                "expires_in_days": 7,
                "user": user,
            }))
            .into_response()
        }
        Ok(Err(openlens_meta::MetaError::Invalid(m))) => err(StatusCode::BAD_REQUEST, m),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

/// `GET /api/users`: the team.
async fn list_users(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.users()).await {
        Ok(Ok(u)) => Json(json!({ "users": u })).into_response(),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

/// `DELETE /api/users/{id}`: remove someone; their sessions stop at once.
async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    let who = id.clone();
    match tokio::task::spawn_blocking(move || meta.delete_user(&id)).await {
        Ok(Ok(true)) => {
            crate::privacy::audit(
                &state,
                &headers,
                None,
                "user.delete",
                &format!("{who} (api)"),
            );
            Json(json!({ "deleted": true })).into_response()
        }
        Ok(Ok(false)) => err(StatusCode::NOT_FOUND, "no such user"),
        Ok(Err(openlens_meta::MetaError::Invalid(why))) => err(StatusCode::CONFLICT, why),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

#[derive(Deserialize)]
struct UserPatch {
    name: Option<String>,
    email: Option<String>,
    role: Option<String>,
    projects: Option<Vec<String>>,
    /// `[]` or `null`-free: give `"all"` to clear the project list.
    #[serde(default)]
    all_projects: bool,
    disabled: Option<bool>,
    timezone: Option<String>,
}

/// `PATCH /api/users/{id}` with any of `{name, email, role, projects,
/// all_projects, disabled, timezone}`. 409 when it would leave no admin.
async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<UserPatch>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    if let Some(tz) = b.timezone.as_deref().filter(|t| !t.is_empty())
        && check_timezone(tz).is_err()
    {
        return err(StatusCode::BAD_REQUEST, "timezone must be an IANA name");
    }
    let meta = state.meta.clone();
    let res = tokio::task::spawn_blocking(move || {
        let Some(u) = meta.user(&id)? else {
            return Ok(None);
        };
        let projects = if b.all_projects {
            None
        } else {
            b.projects.clone().or(u.projects.clone())
        };
        let name = b.name.clone().or(u.name.clone());
        meta.update_user(
            &id,
            name.as_deref(),
            b.email.as_deref().unwrap_or(&u.email),
            b.role.as_deref().unwrap_or(&u.role),
            projects.as_deref(),
        )?;
        if let Some(d) = b.disabled {
            meta.set_user_disabled(&id, d)?;
        }
        if let Some(tz) = &b.timezone {
            meta.set_user_timezone(&id, Some(tz.as_str()).filter(|t| !t.is_empty()))?;
        }
        meta.user(&id)
    })
    .await;
    match res {
        Ok(Ok(Some(u))) => {
            crate::privacy::audit(
                &state,
                &headers,
                None,
                "user.update",
                &format!("{} (api)", u.email),
            );
            Json(json!({ "user": u })).into_response()
        }
        Ok(Ok(None)) => err(StatusCode::NOT_FOUND, "no such user"),
        Ok(Err(openlens_meta::MetaError::Invalid(why))) => err(StatusCode::CONFLICT, why),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

async fn get_map(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.feature_map(&p.id)).await {
        Ok(Ok(Some((version, yaml)))) => {
            Json(json!({ "version": version, "yaml": yaml })).into_response()
        }
        Ok(Ok(None)) => err(
            StatusCode::NOT_FOUND,
            "no feature map yet: PUT one (see /agents.md)",
        ),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

#[derive(Deserialize, Default)]
struct PutMapQuery {
    /// Why this version (`features push --message`), #240.
    message: Option<String>,
}

/// `PUT /api/projects/{id}/feature-map[?message=…]` with the YAML as the
/// raw body. 422 with the issues when the map has errors; otherwise stored
/// (with who pushed it and why), and the warnings are returned so the
/// agent can improve it.
async fn put_map(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<PutMapQuery>,
    body: String,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let parsed = openlens_featuremap::parse(&body);
    if parsed.map.is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "stored": false, "issues": parsed.issues })),
        )
            .into_response();
    }
    let mut issues = parsed.issues;
    if let Some(next) = &parsed.map {
        let prev = crate::load_map(&state, &p.id).await;
        issues.extend(crate::features::rename_warnings(
            prev.as_ref().map(|(m, _)| m),
            next,
        ));
    }
    let meta = state.meta.clone();
    let project_id = p.id.clone();
    let author = crate::privacy::actor(&state, &headers);
    let message = q.message.filter(|m| !m.trim().is_empty());
    match tokio::task::spawn_blocking(move || {
        meta.save_feature_map_by(&p.id, &body, Some(&author), message.as_deref())
    })
    .await
    {
        Ok(Ok(version)) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&project_id),
                "feature_map.save",
                &format!("version {version} (api)"),
            );
            Json(json!({ "stored": true, "version": version, "issues": issues })).into_response()
        }
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

/// Validate a map without storing it (lint).
async fn validate_map(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let parsed = openlens_featuremap::parse(&body);
    Json(json!({ "valid": parsed.map.is_some(), "issues": parsed.issues })).into_response()
}

async fn errors(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<FilterQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let map = crate::load_map(&state, &p.id).await;
    let src = openlens_engine::source(map.as_ref().map(|(m, _)| m));
    match openlens_engine::error_groups(&state.query, &p.id, &src, &q.filters(&p.timezone)).await {
        Ok(g) => {
            let original = crate::resolve_frames(&state, &p.id, &g).await;
            let groups: Vec<Value> = g
                .iter()
                .zip(original)
                .map(|(g, o)| {
                    let mut v = json!(g);
                    v["original"] = json!(o);
                    v
                })
                .collect();
            Json(json!({ "groups": groups })).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct BreakdownQuery {
    by: String,
    #[serde(flatten)]
    filters: FilterQuery,
}

/// `GET /api/projects/{id}/breakdown?by=role|account|plan|page|feature`.
async fn breakdown(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<BreakdownQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let map = crate::load_map(&state, &p.id).await;
    let f = q.filters.filters(&p.timezone);
    match openlens_engine::breakdown(&state.query, &p.id, map.as_ref().map(|(m, _)| m), &f, &q.by)
        .await
    {
        Ok(rows) => Json(json!({
            "by": q.by,
            "window_days": 30,
            "role_rule": "role at event time: each event keeps the role the user had when it was sent",
            "rows": rows,
        }))
        .into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// `PUT /api/projects/{id}/sourcemaps/{release}/{bundle}`: the source map of
/// one minified file (`bundle` is its file name, e.g. `app.3f9a.js`).
async fn put_sourcemap(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, release, bundle)): Path<(String, String, String)>,
    body: String,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let root = state.data_dir.clone();
    let res = tokio::task::spawn_blocking(move || {
        openlens_errors::store(&root, &p.id, &release, &bundle, &body)
    })
    .await;
    match res {
        Ok(Ok(())) => Json(json!({ "stored": true })).into_response(),
        Ok(Err(openlens_errors::Error::Io(e))) => {
            tracing::error!(%e, "cannot store source map");
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "cannot store the source map right now",
            )
        }
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, &e.to_string()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "storage task failed"),
    }
}

/// `GET /api/projects/{id}/people/{distinct_id}`: one user's timeline: who
/// they are, their role changes, their latest events and recorded sessions.
async fn person(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, distinct_id)): Path<(String, String)>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let map = crate::load_map(&state, &p.id).await;
    let (meta, root, pid, who) = (
        state.meta.clone(),
        state.data_dir.clone(),
        p.id.clone(),
        distinct_id.clone(),
    );
    let stored = tokio::task::spawn_blocking(move || {
        let person = meta.person(&pid, &who).ok().flatten();
        let roles = meta.role_history(&pid, &who).unwrap_or_default();
        let replays = openlens_replay::list(&root, &pid, Some(&who)).unwrap_or_default();
        (person, roles, replays)
    })
    .await;
    let Ok((person, roles, replays)) = stored else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        );
    };
    let events = match openlens_engine::person_events(
        &state.query,
        &p.id,
        map.as_ref().map(|(m, _)| m),
        &distinct_id,
        100,
    )
    .await
    {
        Ok(e) => e,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    if person.is_none() && events.is_empty() {
        return err(StatusCode::NOT_FOUND, "no such person in the last 90 days");
    }
    Json(json!({
        "distinct_id": distinct_id,
        "person": person.map(|p| json!({
            "role": p.role, "account": p.account, "plan": p.plan,
            "first_seen": p.first_seen, "last_seen": p.last_seen,
        })),
        "role_history": roles.iter().map(|(r, since)| json!({ "role": r, "since": since })).collect::<Vec<_>>(),
        "recordings": replays.iter().map(|r| json!({
            "session_id": r.session_id, "started": r.started, "chunks": r.chunks,
            "dashboard": format!("/p/{}/replays/{}", p.id, r.session_id),
        })).collect::<Vec<_>>(),
        "events": events,
    }))
    .into_response()
}

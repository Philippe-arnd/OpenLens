//! Feature flags over HTTP.
//!
//! - `GET /flags?key=pk_…&distinct_id=…&role=…&account=…` (public, capture
//!   key): the keys of the flags that are on for that user. The raw role is
//!   resolved to its canonical role with the feature map before targeting.
//!   It also carries `replay_rate`, the share of sessions the SDK records
//!   from their start (#327), and `sample_rate`, the share of sessions
//!   that send the distribution-only events (`$vital`, `$scroll`,
//!   `$field`; #702, D-107).
//! - Admin (bearer): `GET /api/projects/{id}/flags`,
//!   `PUT|DELETE /api/projects/{id}/flags/{key}`, and `PATCH` to switch a
//!   flag on or off or change its rollout (`flags.update`).
//! - Dashboard: `POST /p/{id}/flags/{key}` (session, same-origin) to switch
//!   a flag or change its rollout.

use std::collections::HashMap;

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use openlens_engine::commands::{Caller, Command};
use openlens_flags::{Flag, Subject};
use serde::Deserialize;
use serde_json::json;

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project}/flags", get(admin_list))
        .route(
            "/api/projects/{project}/flags/{key}",
            put(admin_put).delete(admin_delete).patch(admin_switch),
        )
        .route("/p/{project}/flags", get(page))
        .route("/p/{project}/flags/{key}", post(page_update))
}

fn err(status: StatusCode, msg: &str) -> Response {
    crate::apierr::json(status, msg)
}

/// All flags of a project (invalid stored rows are skipped).
pub(crate) async fn load(state: &AppState, project: &str) -> Vec<Flag> {
    let meta = state.meta.clone();
    let p = project.to_owned();
    tokio::task::spawn_blocking(move || openlens_engine::commands::flags::load(&meta, &p))
        .await
        .unwrap_or_default()
}

pub(crate) async fn save(state: &AppState, project: &str, f: &Flag) -> Result<(), String> {
    let meta = state.meta.clone();
    let (p, f) = (project.to_owned(), f.clone());
    tokio::task::spawn_blocking(move || openlens_engine::commands::flags::save(&meta, &p, &f))
        .await
        .map_err(|e| e.to_string())?
}

/// Properties for flag rules (#279), from `p.<key>=<value>` in the query
/// (and `plan`), capped in number and length; `country` comes from the
/// caller's address when the app does not send one. Values are used to
/// evaluate and never stored.
pub(crate) fn properties(
    q: &HashMap<String, String>,
    peer: &crate::capture::Peer,
    headers: &HeaderMap,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = q
        .iter()
        .filter_map(|(k, v)| {
            let key = if k == "plan" {
                k.as_str()
            } else {
                k.strip_prefix("p.")?
            };
            let key = key.to_lowercase();
            let v = v.trim();
            (openlens_flags::valid_property(&key)
                && !v.is_empty()
                && v.len() <= openlens_flags::MAX_VALUE_LEN)
                .then(|| (key, v.to_owned()))
        })
        .collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out.truncate(openlens_flags::MAX_PROPERTIES);
    if !out.iter().any(|(k, _)| k == "country")
        && let Some(c) =
            crate::audience::client_ip(peer, headers).and_then(crate::audience::country)
    {
        out.push(("country".into(), c));
    }
    out
}

pub(crate) async fn public(
    State(state): State<AppState>,
    peer: crate::capture::Peer,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if crate::capture::ip_limited(&state, &peer, &headers) {
        return crate::capture::too_many();
    }
    let cors = [
        ("access-control-allow-origin", "*"),
        ("cache-control", "no-store"),
    ];
    let (Some(key), Some(id)) = (q.get("key"), q.get("distinct_id")) else {
        return (
            StatusCode::BAD_REQUEST,
            cors,
            Json(json!({ "error": "key and distinct_id are required" })),
        )
            .into_response();
    };
    if id.is_empty() || id.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            cors,
            Json(json!({ "error": "invalid distinct_id" })),
        )
            .into_response();
    }
    let meta = state.meta.clone();
    let k = key.clone();
    let project = match tokio::task::spawn_blocking(move || meta.project_by_key(&k)).await {
        Ok(Ok(Some(p))) => p,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                cors,
                Json(json!({ "error": "unknown project key" })),
            )
                .into_response();
        }
    };
    let map = crate::load_map(&state, &project.id).await;
    let raw_role = q.get("role").map(String::as_str).filter(|r| !r.is_empty());
    let role = match (&map, raw_role) {
        (Some((m, _)), Some(r)) => m.resolve_role(r).or(Some(r)),
        (None, r) => r,
        (_, None) => None,
    };
    let flags = load(&state, &project.id).await;
    let props = properties(&q, &peer, &headers);
    let subject = Subject {
        distinct_id: id,
        role,
        account: q.get("account").map(String::as_str),
        properties: &props,
    };
    let on = openlens_flags::evaluate(&flags, &subject);
    let variants = openlens_flags::variants(&flags, &subject);
    let all = crate::surveys::load(&state, &project.id).await;
    let surveys: Vec<_> = all
        .iter()
        .filter(|s| s.eligible(&subject))
        .map(|s| s.public())
        .collect();
    // The share of sessions to record from their start (#327, D-035), and
    // of those that send the distribution-only events (#702, D-107): one
    // daily count for both.
    let replay = crate::replay::rate(&state, &project.id, &project.timezone).await;
    (
        StatusCode::OK,
        cors,
        Json(json!({
            "flags": on,
            "variants": variants,
            "surveys": surveys,
            "replay_rate": replay.rate,
            "sample_rate": replay.sample_rate,
        })),
    )
        .into_response()
}

fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    (!auth::has_bearer(state, headers))
        .then(|| err(StatusCode::UNAUTHORIZED, "admin bearer token required"))
}

async fn admin_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    // `flags.list` (#525): the same rows as `load`.
    let input = openlens_engine::commands::flags::ListInput { project };
    match openlens_engine::commands::flags::List::run(&state, &Caller::default(), input).await {
        Ok(flags) => Json(json!({ "flags": flags })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct FlagSwitch {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    rollout: Option<u8>,
}

/// `PATCH /api/projects/{id}/flags/{key}` (#525): `flags.update`, switch an
/// existing flag on or off and/or set its rollout, the rest unchanged.
async fn admin_switch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, key)): Path<(String, String)>,
    Json(b): Json<FlagSwitch>,
) -> Response {
    use openlens_engine::commands::flags::{Update, UpdateInput};
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    if project_exists(&state, &project).await.is_none() {
        return err(StatusCode::NOT_FOUND, "unknown project");
    }
    if !load(&state, &project).await.iter().any(|f| f.key == key) {
        return err(StatusCode::NOT_FOUND, "no such flag");
    }
    let input = UpdateInput {
        project,
        key,
        enabled: b.enabled,
        rollout: b.rollout,
    };
    match Update::run(&state, &Caller::default(), input).await {
        Ok(f) => Json(json!({ "flag": f })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn admin_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, key)): Path<(String, String)>,
    Json(mut f): Json<Flag>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    if project_exists(&state, &project).await.is_none() {
        return err(StatusCode::NOT_FOUND, "unknown project");
    }
    f.key = key;
    match save(&state, &project, &f).await {
        Ok(()) => Json(json!({ "flag": f })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

async fn admin_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, key)): Path<(String, String)>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.delete_flag(&project, &key)).await {
        Ok(Ok(true)) => Json(json!({ "deleted": true })).into_response(),
        Ok(Ok(false)) => err(StatusCode::NOT_FOUND, "no such flag"),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ),
    }
}

async fn project_exists(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

/// Same-origin check for dashboard form posts (on top of SameSite=Lax).
pub(crate) fn same_origin(headers: &HeaderMap) -> bool {
    let base = crate::public_url(headers);
    match headers.get("origin").and_then(|v| v.to_str().ok()) {
        Some(o) => {
            o == base || o.replace("http://", "https://") == base.replace("http://", "https://")
        }
        None => headers
            .get("referer")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|r| r.starts_with(&base)),
    }
}

async fn page(
    _: crate::access::Session,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = project_exists(&state, &id).await else {
        return crate::web::html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let flags = load(&state, &p.id).await;
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    crate::web::html_page(StatusCode::OK, openlens_ui::flags(&info, &flags))
}

#[derive(Deserialize)]
struct FlagForm {
    enabled: Option<String>,
    rollout: Option<u8>,
}

async fn page_update(
    _: crate::access::Session,
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    Form(form): Form<FlagForm>,
) -> Response {
    let mut flags = load(&state, &id).await;
    let Some(f) = flags.iter_mut().find(|f| f.key == key) else {
        return err(StatusCode::NOT_FOUND, "no such flag");
    };
    f.enabled = form.enabled.as_deref() == Some("on");
    if let Some(r) = form.rollout {
        f.rollout = r.min(100);
    }
    let f = f.clone();
    match save(&state, &id, &f).await {
        Ok(()) => Redirect::to(&format!("/p/{id}/flags")).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

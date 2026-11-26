//! Privacy (#230): forget a person (right to erasure), export a person's
//! data (right of access), and the audit log of admin actions.

use std::collections::HashSet;
use std::sync::Mutex;

use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

/// Held while files are rewritten (compaction, erasure), so two rewrites
/// never race on the same day.
pub(crate) static MAINTENANCE: Mutex<()> = Mutex::new(());

/// Who did it, for the audit log: `owner`, the user's email, or `api`.
pub(crate) fn actor(state: &AppState, headers: &HeaderMap) -> String {
    if auth::has_bearer(state, headers) {
        return "api (admin token)".into();
    }
    match auth::session_id(state, headers) {
        Some(id) if id == auth::OWNER => "owner".into(),
        Some(id) => state.meta.user(&id).ok().flatten().map_or(id, |u| u.email),
        None => "unknown".into(),
    }
}

/// Record an admin action (best effort: a failed write is logged).
pub(crate) fn audit(
    state: &AppState,
    headers: &HeaderMap,
    project: Option<&str>,
    action: &str,
    detail: &str,
) {
    if let Err(e) = state
        .meta
        .audit(&actor(state, headers), project, action, detail)
    {
        tracing::warn!(%e, action, "audit entry not written");
    }
}

/// Record an action done by a known user id (no request at hand).
pub(crate) fn audit_as(
    state: &AppState,
    user: &str,
    project: Option<&str>,
    action: &str,
    detail: &str,
) {
    let who = if user == auth::OWNER {
        "owner".to_owned()
    } else {
        state
            .meta
            .user(user)
            .ok()
            .flatten()
            .map_or_else(|| user.to_owned(), |u| u.email)
    };
    if let Err(e) = state.meta.audit(&who, project, action, detail) {
        tracing::warn!(%e, action, "audit entry not written");
    }
}

/// Forget a person and every anonymous id linked to them (blocking):
/// tombstones, person rows, events rewritten out of the files, replays
/// deleted. Returns (ids forgotten, events removed, recordings removed).
pub(crate) fn forget_blocking(
    state: &AppState,
    project: &str,
    who: &str,
    linked: Vec<String>,
) -> Result<(usize, usize, usize), String> {
    let mut ids: HashSet<String> = linked.into_iter().collect();
    ids.insert(who.to_owned());
    let list: Vec<String> = ids.iter().cloned().collect();
    state
        .meta
        .forget(project, &list)
        .map_err(|e| e.to_string())?;
    let _guard = MAINTENANCE
        .lock()
        .map_err(|_| "maintenance lock poisoned")?;
    crate::flush_now(state)?;
    let events =
        openlens_storage::erase(&state.data_dir, project, &ids).map_err(|e| e.to_string())?;
    let mut replays = 0;
    for id in &ids {
        replays +=
            openlens_replay::erase(&state.data_dir, project, id).map_err(|e| e.to_string())?;
    }
    state.boards.bump(project);
    Ok((ids.len(), events, replays))
}

/// The anonymous ids that were linked to a person by `$identify`.
async fn linked_ids(state: &AppState, project: &str, who: &str) -> Vec<String> {
    let mut params = openlens_sqlgen::Params::new();
    let Ok(person) = params.bind(who) else {
        return Vec::new();
    };
    let sql = format!(
        "SELECT DISTINCT regexp_match(properties, '\"\\$anon_distinct_id\":\"([^\"]+)\"')[1] AS anon \
         FROM raw_events WHERE event = '$identify' AND distinct_id = {person}"
    );
    let mut ids: Vec<String> = state
        .query
        .with_params(&params)
        .sql(project, &sql)
        .await
        .map(|r| {
            r.rows
                .iter()
                .filter_map(|x| x.get("anon").and_then(|v| v.as_str()).map(str::to_owned))
                .filter(|a| a != who)
                .collect()
        })
        .unwrap_or_default();
    // And those recorded at ingestion (#450), even if their `$identify`
    // is no longer in the files.
    let (meta, p, w) = (state.meta.clone(), project.to_owned(), who.to_owned());
    if let Ok(Ok(kept)) = tokio::task::spawn_blocking(move || meta.aliases_of(&p, &w)).await {
        for a in kept {
            if !ids.contains(&a) {
                ids.push(a);
            }
        }
    }
    ids
}

#[derive(Deserialize, Default)]
pub(crate) struct ConfirmForm {
    #[serde(default)]
    confirm: String,
}

/// `POST /p/{id}/people/{who}/forget` (admins; the id typed again).
pub(crate) async fn forget(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, who)): Path<(String, String)>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    if f.confirm.trim() != who {
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message(
                "Not forgotten",
                "Type the person's id exactly to forget them.",
            ),
        );
    }
    let linked = linked_ids(&state, &project, &who).await;
    let (st, p, w) = (state.clone(), project.clone(), who.clone());
    match tokio::task::spawn_blocking(move || forget_blocking(&st, &p, &w, linked)).await {
        Ok(Ok((ids, events, replays))) => {
            audit(
                &state,
                &headers,
                Some(&project),
                "person.forget",
                &format!("{} ids, {events} events, {replays} recordings", ids),
            );
            html_page(
                StatusCode::OK,
                openlens_ui::message(
                    "Forgotten",
                    &format!(
                        "Removed {events} events and {replays} recordings ({ids} ids, the person and the anonymous ids linked to them). New events from them are dropped."
                    ),
                ),
            )
        }
        Ok(Err(e)) => html_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Not done", &e),
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `DELETE /api/projects/{id}/people/{who}` (bearer): the same.
pub(crate) async fn api_forget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, who)): Path<(String, String)>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let linked = linked_ids(&state, &project, &who).await;
    let (st, p, w) = (state.clone(), project.clone(), who.clone());
    match tokio::task::spawn_blocking(move || forget_blocking(&st, &p, &w, linked)).await {
        Ok(Ok((ids, events, replays))) => {
            audit(
                &state,
                &headers,
                Some(&project),
                "person.forget",
                &format!("{ids} ids, {events} events, {replays} recordings"),
            );
            Json(json!({ "forgotten": ids, "events_removed": events, "recordings_removed": replays })).into_response()
        }
        Ok(Err(e)) => crate::apierr::json(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /p/{id}/people/{who}/export.json` (admins; also with the bearer):
/// everything stored about a person (right of access).
pub(crate) async fn export(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, who)): Path<(String, String)>,
) -> Response {
    if openlens_sqlgen::checked(&who).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    // Days in the cold tier come back first: the export covers everything
    // kept about the person (#326). They return to S3 at the next pass.
    if let Some((cfg, _)) = crate::s3::configured(&state.meta, &state.data_dir) {
        let root = state.data_dir.clone();
        let p = project.clone();
        let space = state.disk_space;
        let back = tokio::task::spawn_blocking(move || {
            // Room for the cold days twice over, or nothing is fetched
            // (review of v0.3.0): an export must not fill the disk.
            let need = crate::cold::project_bytes(&cfg, &root, &p)?;
            if let Some((free, _)) = space(&root)
                && free < need.saturating_mul(2)
            {
                return Err(format!("{need} bytes on S3 do not fit on this disk"));
            }
            let _guard = MAINTENANCE
                .lock()
                .map_err(|_| "maintenance lock poisoned".to_owned())?;
            crate::cold::restore_project(&cfg, &root, &p)
        })
        .await;
        if !matches!(back, Ok(Ok(_))) {
            return crate::apierr::json(
                StatusCode::SERVICE_UNAVAILABLE,
                "older days are on S3 and could not be read back; try again",
            );
        }
    }
    let mut ids = linked_ids(&state, &project, &who).await;
    ids.push(who.clone());
    let mut params = openlens_sqlgen::Params::new();
    let list = ids
        .iter()
        .filter_map(|i| params.bind(i).ok())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT uuid, event, distinct_id, session_id, CAST(timestamp AS VARCHAR) AS timestamp, url, path, feature, \
         role, account, plan, environment, release, properties FROM raw_events WHERE distinct_id IN ({list}) ORDER BY timestamp"
    );
    let engine = openlens_query::QueryEngine::new(
        &state.data_dir,
        openlens_query::Limits {
            max_rows: 200_000,
            timeout: std::time::Duration::from_secs(120),
            ..openlens_query::Limits::default()
        },
    );
    let events = match engine.with_params(&params).sql(&project, &sql).await {
        // A right-of-access export is all or nothing (review of v0.3.0).
        Ok(r) if r.truncated => {
            return crate::apierr::json(
                StatusCode::PAYLOAD_TOO_LARGE,
                "more than 200,000 events for this person: export them day by day with `openlens-cli export`",
            );
        }
        Ok(r) => r.rows,
        Err(e) => {
            return crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string());
        }
    };
    let person = state.meta.person(&project, &who).ok().flatten();
    let roles = state.meta.role_history(&project, &who).unwrap_or_default();
    let recordings: Vec<_> = ids
        .iter()
        .flat_map(|i| openlens_replay::list(&state.data_dir, &project, Some(i)).unwrap_or_default())
        .map(|r| json!({ "session_id": r.session_id, "started": r.started, "chunks": r.chunks }))
        .collect();
    audit(
        &state,
        &headers,
        Some(&project),
        "person.export",
        &format!("{} events", events.len()),
    );
    let body = json!({
        "project": project,
        "distinct_id": who,
        "linked_anonymous_ids": ids.iter().filter(|i| **i != who).collect::<Vec<_>>(),
        "person": person.map(|p| json!({
            "role": p.role, "account": p.account, "plan": p.plan,
            "first_seen": p.first_seen, "last_seen": p.last_seen,
        })),
        "role_history": roles.iter().map(|(r, s)| json!({ "role": r, "since": s })).collect::<Vec<_>>(),
        "recordings": recordings,
        "events": events,
        "note": "OpenLens never stores IP addresses; page views carry a country, device, browser and OS derived at capture.",
    });
    let mut r = Json(body).into_response();
    if let Ok(v) = HeaderValue::from_str(&format!(
        "attachment; filename=\"openlens-person-{}.json\"",
        crate::short_hash(&who)
    )) {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    r
}

/// `GET /audit` (admins).
pub(crate) async fn audit_page(_: crate::access::Admin, State(state): State<AppState>) -> Response {
    let rows: Vec<(String, String, Option<String>, String, String)> = state
        .meta
        .audit_log(500)
        .unwrap_or_default()
        .into_iter()
        .map(|(at, a, p, act, d)| {
            let when = chrono::DateTime::from_timestamp(at, 0).map_or_else(String::new, |t| {
                t.format("%Y-%m-%d %H:%M:%S UTC").to_string()
            });
            (when, a, p, act, d)
        })
        .collect();
    html_page(StatusCode::OK, openlens_ui::audit(&rows))
}

/// `GET /api/audit` (bearer).
pub(crate) async fn api_audit(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let rows: Vec<_> = state
        .meta
        .audit_log(1000)
        .unwrap_or_default()
        .into_iter()
        .map(|(at, actor, project, action, detail)| json!({ "at": at, "actor": actor, "project": project, "action": action, "detail": detail }))
        .collect();
    Json(json!({ "audit": rows })).into_response()
}

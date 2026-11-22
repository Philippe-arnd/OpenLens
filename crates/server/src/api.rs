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

use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{project}/sql", post(sql))
        .route("/api/projects/{project}/board", get(board))
        .route("/api/projects/{project}/errors", get(errors))
        .route(
            "/api/projects/{project}/feature-map",
            get(get_map).put(put_map),
        )
        .route("/api/feature-map/validate", post(validate_map))
        .route(
            "/api/projects/{project}/import",
            post(import).layer(axum::extract::DefaultBodyLimit::max(IMPORT_MAX)),
        )
        .route("/api/projects/{project}/export", get(export))
        .route("/api/login-links", post(login_link))
}

/// Largest import chunk, in bytes (the CLI sends ~5,000 lines per chunk).
const IMPORT_MAX: usize = 16 * 1024 * 1024;
/// Largest number of lines in one import chunk.
const IMPORT_MAX_LINES: usize = 20_000;

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if auth::has_bearer(state, headers) {
        None
    } else {
        Some(err(StatusCode::UNAUTHORIZED, "admin bearer token required"))
    }
}

async fn known_project(
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
    match tokio::task::spawn_blocking(move || meta.projects()).await {
        Ok(Ok(ps)) => Json(json!({
            "projects": ps.iter().map(|p| json!({"id": p.id, "name": p.name, "key": p.api_key})).collect::<Vec<_>>()
        }))
        .into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "metadata store unavailable"),
    }
}

#[derive(Deserialize)]
struct NewProject {
    name: String,
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
    let meta = state.meta.clone();
    let host = crate::base_url(&headers);
    match tokio::task::spawn_blocking(move || meta.create_project(body.name.trim(), None)).await {
        Ok(Ok(p)) => (
            StatusCode::CREATED,
            Json(json!({
                "id": p.id, "name": p.name, "key": p.api_key,
                "snippet": format!(r#"<script src="{host}/c.js" data-key="{}" defer></script>"#, p.api_key),
                "guide": format!("{host}/agents.md"),
            })),
        )
            .into_response(),
        Ok(Err(e)) => err(StatusCode::CONFLICT, &e.to_string()),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "metadata store unavailable"),
    }
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
    match state.query.sql(&p.id, &body.sql).await {
        Ok(res) => Json(res).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct FilterQuery {
    pub(crate) env: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) release: Option<String>,
    pub(crate) event: Option<String>,
}

impl FilterQuery {
    pub(crate) fn filters(&self) -> Filters {
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
        }
    }
}

async fn board(
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
    match openlens_engine::board(
        &state.query,
        &p.id,
        &q.filters(),
        map.as_ref().map(|(m, v)| (m, *v)),
    )
    .await
    {
        Ok(b) => Json(b).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn login_link(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    let host = crate::base_url(&headers);
    match tokio::task::spawn_blocking(move || meta.create_login_link(7 * 24 * 3600)).await {
        Ok(Ok(t)) => {
            Json(json!({ "url": format!("{host}/l/{t}"), "expires_in_days": 7 })).into_response()
        }
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

/// `PUT /api/projects/{id}/feature-map` with the YAML as the raw body.
/// 422 with the issues when the map has errors; otherwise stored, and the
/// warnings are returned so the agent can improve it.
async fn put_map(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
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
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.save_feature_map(&p.id, &body)).await {
        Ok(Ok(version)) => {
            Json(json!({ "stored": true, "version": version, "issues": parsed.issues }))
                .into_response()
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

/// `POST /api/projects/{id}/import`: backfill from the app's own data.
///
/// Body: JSONL. A line with `"type": "person"` is a user
/// (`id`, `role`, `account`, `plan`, `created_at`, `last_seen_at`): it
/// updates the person and adds `$identify` at `created_at` and `$seen` at
/// `last_seen_at`. Any other line is an event (`event`, `distinct_id`,
/// `timestamp`, `properties`), e.g. a past success event. Every imported
/// event carries `$backfill: true`. A chunk whose SHA-256 was already
/// imported is refused with 409, so re-running a backfill is safe.
async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    body: String,
) -> Response {
    use openlens_event::{CaptureBatch, RawEvent};
    use serde_json::Map;
    use sha2::Digest;

    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > IMPORT_MAX_LINES {
        return err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "at most 20,000 lines per request: split the file (the CLI does)",
        );
    }
    let sha = hex::encode(sha2::Sha256::digest(body.as_bytes()));

    let mut raws = Vec::new();
    let mut persons = Vec::new();
    let mut rejected = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(line) else {
            rejected.push(json!({ "line": i + 1, "reason": "not a JSON object" }));
            continue;
        };
        let text = |o: &Map<String, Value>, k: &str| match o.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        if o.get("type").and_then(Value::as_str) == Some("person") {
            let Some(id) = text(&o, "id") else {
                rejected.push(json!({ "line": i + 1, "reason": "person needs an id" }));
                continue;
            };
            let mut props = Map::new();
            props.insert("$backfill".into(), Value::Bool(true));
            for k in ["role", "account", "plan"] {
                if let Some(v) = text(&o, k) {
                    props.insert(format!("${k}"), Value::String(v));
                }
            }
            persons.push((
                id.clone(),
                text(&o, "role"),
                text(&o, "account"),
                text(&o, "plan"),
                text(&o, "created_at"),
            ));
            for (ev, at) in [("$identify", "created_at"), ("$seen", "last_seen_at")] {
                if let Some(ts) = text(&o, at) {
                    raws.push(RawEvent {
                        event: ev.into(),
                        distinct_id: Some(id.clone()),
                        timestamp: Some(ts),
                        properties: Some(props.clone()),
                        ..RawEvent::default()
                    });
                }
            }
        } else {
            let mut props = match o.remove("properties") {
                Some(Value::Object(m)) => m,
                _ => Map::new(),
            };
            props.insert("$backfill".into(), Value::Bool(true));
            raws.push(RawEvent {
                event: text(&o, "event").unwrap_or_default(),
                distinct_id: text(&o, "distinct_id"),
                timestamp: text(&o, "timestamp"),
                session_id: text(&o, "session_id"),
                uuid: None,
                properties: Some(props),
            });
        }
    }

    let meta = state.meta.clone();
    let pid = p.id.clone();
    let n_rows = lines.len() as i64;
    match tokio::task::spawn_blocking(move || meta.record_import(&pid, &sha, n_rows)).await {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return err(StatusCode::CONFLICT, "this chunk was already imported"),
        _ => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metadata store unavailable",
            );
        }
    }

    let now = chrono::Utc::now();
    let mut accepted = 0usize;
    let mut events = Vec::new();
    for chunk in raws.chunks(openlens_event::limits::MAX_BATCH_EVENTS) {
        let batch = CaptureBatch {
            key: None,
            sent_at: None,
            batch: chunk.to_vec(),
        };
        if let Ok(n) = openlens_event::normalize_batch(&p.id, batch, now) {
            accepted += n.events.len();
            for (i, e) in n.rejected {
                rejected.push(json!({ "event_index": i, "reason": e.to_string() }));
            }
            events.extend(n.events);
        }
    }
    let st = state.clone();
    let pid = p.id.clone();
    let n_persons = persons.len();
    let stored = tokio::task::spawn_blocking(move || -> Result<(), String> {
        for (id, role, account, plan, created) in persons {
            let at = created
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map_or(now, |t| t.with_timezone(&chrono::Utc));
            st.meta
                .identify(
                    &pid,
                    &id,
                    role.as_deref(),
                    account.as_deref(),
                    plan.as_deref(),
                    at,
                )
                .map_err(|e| e.to_string())?;
        }
        let mut w = st
            .writer
            .lock()
            .map_err(|_| "writer lock poisoned".to_owned())?;
        w.append(&events).map_err(|e| e.to_string())?;
        // Backfilled rows land in old partitions: write them now.
        w.flush().map(|_| ()).map_err(|e| e.to_string())
    })
    .await;
    match stored {
        Ok(Ok(())) => {
            Json(json!({ "accepted_events": accepted, "persons": n_persons, "rejected": rejected }))
                .into_response()
        }
        Ok(Err(e)) => err(StatusCode::SERVICE_UNAVAILABLE, &e),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "import task failed"),
    }
}

#[derive(Deserialize)]
struct ExportQuery {
    date: String,
}

/// Most rows one export request returns (one day of one project).
const EXPORT_MAX_ROWS: usize = 500_000;

/// `GET /api/projects/{id}/export?date=YYYY-MM-DD`: one day of raw events
/// as JSONL, for agents doing deep analysis. The CLI walks a date range day
/// by day, so memory stays bounded. `x-openlens-truncated: true` when the
/// day had more than 500k events.
async fn export(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<ExportQuery>,
) -> Response {
    if let Some(r) = guard(&state, &headers) {
        return r;
    }
    let p = match known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let d = q.date.as_bytes();
    let valid = d.len() == 10
        && d.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !valid {
        return err(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD");
    }
    let engine = openlens_query::QueryEngine::new(
        &state.data_dir,
        openlens_query::Limits {
            max_rows: EXPORT_MAX_ROWS,
            timeout: std::time::Duration::from_secs(120),
            ..openlens_query::Limits::default()
        },
    );
    let sql = format!(
        "SELECT uuid, event, distinct_id, session_id, CAST(timestamp AS VARCHAR) AS timestamp, url, path, feature, \
         role, account, plan, environment, release, properties FROM events WHERE date = '{}' ORDER BY timestamp",
        q.date
    );
    match engine.sql(&p.id, &sql).await {
        Ok(res) => {
            let mut body = String::new();
            for row in &res.rows {
                body.push_str(&Value::Object(row.clone()).to_string());
                body.push('\n');
            }
            (
                [
                    ("content-type", "application/x-ndjson"),
                    (
                        "x-openlens-truncated",
                        if res.truncated { "true" } else { "false" },
                    ),
                ],
                body,
            )
                .into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
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
    match openlens_engine::error_groups(&state.query, &p.id, &src, &q.filters()).await {
        Ok(g) => Json(json!({ "groups": g })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

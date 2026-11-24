//! Import and export of events. Moved out of api.rs as is (#307).

use super::*;

/// `POST /api/projects/{id}/import`: backfill from the app's own data.
///
/// Body: JSONL. A line with `"type": "person"` is a user
/// (`id`, `role`, `account`, `plan`, `created_at`, `last_seen_at`): it
/// updates the person and adds `$identify` at `created_at` and `$seen` at
/// `last_seen_at`. Any other line is an event (`event`, `distinct_id`,
/// `timestamp`, `properties`), e.g. a past success event. Every imported
/// event carries `$backfill: true`. A chunk whose SHA-256 was already
/// imported is refused with 409, so re-running a backfill is safe.
pub(super) async fn import(
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
pub(super) struct ExportQuery {
    date: String,
}

/// Most rows one export request returns (one day of one project).
pub(super) const EXPORT_MAX_ROWS: usize = 500_000;

/// `GET /api/projects/{id}/export?date=YYYY-MM-DD`: one day of raw events
/// as JSONL, for agents doing deep analysis. The CLI walks a date range day
/// by day, so memory stays bounded. `x-openlens-truncated: true` when the
/// day had more than 500k events.
pub(super) async fn export(
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
    export_day(&state, &p.id, &q.date).await
}

/// One day of a project's events as NDJSON (`date` is YYYY-MM-DD).
pub(crate) async fn export_day(state: &AppState, project: &str, date: &str) -> Response {
    let d = date.as_bytes();
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
        date
    );
    match engine.sql(project, &sql).await {
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

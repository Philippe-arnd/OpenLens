//! The host allow-list of a project (#485, D-043): screened on every
//! captured batch, previewed and saved through the API and the
//! *Environments* tab. The rules live in the engine
//! ([`openlens_engine::environments`]: `set_allowed_hosts`,
//! `preview_allowed_hosts`, [`AllowState::screen`]); this module keeps
//! what ingestion counts in memory and runs the commands.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use openlens_engine::environments::{
    self as env, AllowPreview, AllowReport, AllowState, AllowStatus,
};
use openlens_event::Event;
use serde_json::json;

use crate::{AppState, auth};

/// Screen a captured batch with the project's allow-list: while the list
/// is active, events of other hosts are dropped and counted. The first
/// batch a server sees for a project with a list reads when a listed host
/// last sent events, in the background; until then the list is pending.
pub(crate) fn screen(state: &AppState, project: &str, allow: &[String], events: &mut Vec<Event>) {
    if allow.is_empty() {
        return;
    }
    let now = chrono::Utc::now();
    let fresh = state.caches.env_allow.update(project.to_owned(), |old| {
        let fresh = old.is_none();
        let mut s = old.unwrap_or_else(|| AllowState::new(now, None));
        s.screen(allow, events, now);
        (Some(s), fresh)
    });
    if fresh == Some(true) {
        spawn_init(state, project, allow);
    }
}

/// Read when a host on the list last sent events (the last 7 days), so a
/// restart does not leave a working list pending until the next batch of
/// a listed host. A failure leaves it pending: nothing is dropped.
fn spawn_init(state: &AppState, project: &str, allow: &[String]) {
    let (st, key, allow) = (state.clone(), project.to_owned(), allow.to_vec());
    tokio::spawn(async move {
        match env::preview_allowed_hosts(&st.query, &key, &allow).await {
            Ok(Ok(p)) => {
                st.caches.env_allow.update(key, |old| {
                    let old = old.map(|mut s| {
                        s.seen_listed(p.last_listed_at);
                        s
                    });
                    (old, ())
                });
            }
            Ok(Err(err)) => tracing::warn!(project = %key, %err, "allow-list: stored list refused"),
            Err(err) => {
                tracing::warn!(project = %key, %err, "allow-list: last listed host unknown")
            }
        }
    });
}

/// The project's allow-list with what ingestion counted (engine command
/// `allowed_hosts`).
pub(crate) fn report(state: &AppState, project: &str, allow: &[String]) -> AllowReport {
    let live = state.caches.env_allow.get(&project.to_owned());
    env::allow_report(allow, live.as_ref(), chrono::Utc::now())
}

/// A list as sent: an array of hosts, or (from a form) one text with the
/// hosts separated by spaces, commas or new lines.
pub(crate) fn split_hosts(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .take(env::MAX_ALLOWED + 1)
        .map(str::to_owned)
        .collect()
}

/// Preview `hosts` as the allow-list of `project` and, when `confirm` is
/// true, store it (engine commands `preview_allowed_hosts` and
/// `set_allowed_hosts`). The preview is always computed first, and comes
/// back with the result, so nobody saves a list without its numbers.
/// Saving starts the dropped counts again.
pub(crate) async fn preview_and_save(
    state: &AppState,
    project: &str,
    hosts: &[String],
    confirm: bool,
) -> Result<(AllowPreview, bool), (StatusCode, String)> {
    let preview = match env::preview_allowed_hosts(&state.query, project, hosts).await {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return Err((StatusCode::BAD_REQUEST, e)),
        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
    };
    if !confirm {
        return Ok((preview, false));
    }
    let (st, pid, list) = (state.clone(), project.to_owned(), preview.hosts.clone());
    let saved = tokio::task::spawn_blocking(move || {
        // The settings as stored now, not as cached a minute ago.
        let mut rules = crate::environments::load(&st, &pid).rules;
        env::set_allowed_hosts(&mut rules, &list).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        crate::environments::save(&st, &pid, &rules)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the allow-list could not be saved".to_owned(),
        )
    })?;
    saved?;
    let key = project.to_owned();
    if preview.hosts.is_empty() {
        state.caches.env_allow.remove(&key);
    } else {
        let now = chrono::Utc::now();
        state
            .caches
            .env_allow
            .insert(key, AllowState::new(now, preview.last_listed_at));
    }
    Ok((preview, true))
}

/// Record a saved list with the preview it was saved after.
pub(crate) fn audit(state: &AppState, headers: &HeaderMap, project: &str, p: &AllowPreview) {
    let detail = if p.hosts.is_empty() {
        "off".to_owned()
    } else {
        let status = match p.status {
            AllowStatus::Active => "active",
            AllowStatus::Pending => "pending",
            AllowStatus::Off => "off",
        };
        format!(
            "{} ({status}; last {} h: {} events of {} other hosts would be dropped)",
            p.hosts.join(" "),
            p.hours,
            p.dropped_events,
            p.dropped.len()
        )
    };
    crate::privacy::audit(
        state,
        headers,
        Some(project),
        "environments.allow",
        &detail.chars().take(500).collect::<String>(),
    );
}

/// The body of `POST /api/projects/{id}/environments/allow`.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct AllowRequest {
    /// The list (exact hosts or `*.suffix`); empty turns it off.
    #[serde(default)]
    pub(crate) hosts: Vec<String>,
    /// Store it; without it, only the preview is returned.
    #[serde(default)]
    pub(crate) confirm: bool,
}

/// `POST /api/projects/{id}/environments/allow` with `{"hosts",
/// "confirm"}`: the preview of a host allow-list over the last 24 hours
/// and, with `confirm: true`, the list saved (audited as
/// `environments.allow`).
pub(crate) async fn api_allow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Json(req): Json<AllowRequest>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    if req.hosts.len() > env::MAX_ALLOWED * 4 {
        return crate::apierr::json(StatusCode::BAD_REQUEST, "too many hosts");
    }
    let (st, id) = (state.clone(), project.clone());
    match tokio::task::spawn_blocking(move || st.meta.project(&id)).await {
        Ok(Ok(Some(_))) => {}
        Ok(Ok(None)) => return crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        _ => {
            return crate::apierr::json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metadata store unavailable",
            );
        }
    }
    match preview_and_save(&state, &project, &req.hosts, req.confirm).await {
        Ok((preview, saved)) => {
            if saved {
                audit(&state, &headers, &project, &preview);
            }
            let list = crate::environments::current(&state, &project);
            Json(json!({
                "preview": preview,
                "saved": saved,
                "allow": report(&state, &project, &list.rules.allow),
            }))
            .into_response()
        }
        Err((status, msg)) => crate::apierr::json(status, &msg),
    }
}

/// The preview the *Environments* tab shows for `?allow=…` (admins only),
/// or why the list was refused.
pub(crate) async fn tab_preview(
    state: &AppState,
    project: &str,
    hosts: Option<&str>,
) -> Option<Result<AllowPreview, String>> {
    let hosts = split_hosts(hosts?);
    Some(
        preview_and_save(state, project, &hosts, false)
            .await
            .map(|(p, _)| p)
            .map_err(|(_, msg)| msg),
    )
}

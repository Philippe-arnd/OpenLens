//! Capacity follows the host (#606, D-088): the server measures what each
//! project's events and recordings take on the disk (at most hourly, on
//! the maintenance pass), and the engine derives from it and the data
//! volume the recordings each project keeps a day. Retention stays fixed
//! (D-015). `GET /api/capacity` is the engine's `capacity.get`.
//!
//! The route-stats window (#628, D-091) is chosen with it: after each
//! measurement the maintenance pass counts each project's route stats of
//! the last 24 hours, derives the window, and keeps it for the capture
//! response's `x-openlens-stats-window` (never computed per request).

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use openlens_engine::capacity::{self, Capacity, Inputs, ProjectUse, Usage};
use openlens_engine::commands::{Caller, Command};

use crate::{AppState, auth};

/// How often the disk is measured again, at most.
const MEASURE_EVERY: Duration = Duration::from_secs(3600);
/// Directories walked down from the data directory, at most.
const DEPTH: u32 = 12;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/capacity", get(api))
}

/// What the derivation reads now: the volume (one `statvfs`), the cached
/// measurement and the override.
pub(crate) fn inputs(state: &AppState) -> Inputs {
    Inputs {
        volume: (state.disk_space)(&state.data_dir)
            .map(|(available, total)| capacity::Volume { available, total }),
        usage: state
            .caches
            .capacity
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|(_, u)| u.clone())),
        budget_override: std::env::var(capacity::OVERRIDE_VAR)
            .ok()
            .filter(|v| !v.trim().is_empty()),
    }
}

/// The values in force now.
pub(crate) fn current(state: &AppState) -> Capacity {
    capacity::derive(&inputs(state))
}

/// What the disk holds: the data directory, the upgrade copy and each
/// project's events and recordings. Walks the data directory: blocking.
fn measure(state: &AppState) -> Usage {
    let now = chrono::Utc::now();
    let root = &state.data_dir;
    let projects = state
        .meta
        .projects()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let e = openlens_storage::disk_use(root, &p.id, now).unwrap_or_default();
            let r = openlens_replay::disk_use(root, &p.id, now).unwrap_or_default();
            ProjectUse {
                project: p.id,
                event_bytes: e.total,
                replay_bytes: r.total,
                event_bytes_per_day: e.per_day(),
                replay_bytes_per_day: r.per_day(),
                replay_bytes_per_recording: r.per_recording(),
                // Counted next, by `refresh_window` (a query, async).
                stats: None,
            }
        })
        .collect();
    Usage {
        used: openlens_storage::tree_bytes(root, DEPTH),
        upgrade_copy: openlens_storage::tree_bytes(&root.join("backups").join("upgrade"), DEPTH),
        projects,
        measured_at: now.to_rfc3339(),
    }
}

/// Measure the disk again when the last measurement is an hour old (or
/// there is none), and log the recordings a day that follow. Blocking: run from the
/// maintenance pass.
pub(crate) fn refresh(state: &AppState) {
    let due = state
        .caches
        .capacity
        .lock()
        .map(|c| {
            c.as_ref()
                .is_none_or(|(at, _)| at.elapsed() >= MEASURE_EVERY)
        })
        .unwrap_or(false);
    if !due {
        return;
    }
    let usage = measure(state);
    if let Ok(mut c) = state.caches.capacity.lock() {
        *c = Some((Instant::now(), usage));
    }
    let cap = current(state);
    tracing::info!(
        derived = cap.derived,
        budget = %cap.budget_source,
        replays_per_day = cap.replays_per_day.value,
        reason = %cap.reason,
        "capacity from the disk"
    );
}

/// The route-stats window, on every maintenance pass: after a
/// measurement, count each project's route stats of the last 24 hours
/// (once per measurement; a failed count is tried again on the next pass,
/// and meanwhile that project counts as unknown, so 10 minutes apply),
/// then derive the window and keep it for the capture response.
pub(crate) async fn refresh_window(state: &AppState) {
    let pending: Vec<String> = state
        .caches
        .capacity
        .lock()
        .ok()
        .and_then(|c| {
            c.as_ref().map(|(_, u)| {
                u.projects
                    .iter()
                    .filter(|p| p.stats.is_none())
                    .map(|p| p.project.clone())
                    .collect()
            })
        })
        .unwrap_or_default();
    let mut counted = Vec::with_capacity(pending.len());
    for id in pending {
        match capacity::stats_load(&state.query, &id).await {
            Ok(load) => counted.push((id, load)),
            Err(err) => tracing::warn!(project = %id, %err, "cannot count the route stats"),
        }
    }
    if !counted.is_empty() {
        let mut cached = state.caches.capacity.lock().ok();
        if let Some((_, u)) = cached.as_mut().and_then(|c| c.as_mut()) {
            for p in &mut u.projects {
                if let Some((_, load)) = counted.iter().find(|(id, _)| *id == p.project) {
                    p.stats = Some(*load);
                }
            }
        }
    }
    let cap = current(state);
    let w = cap.stats_window.value.unsigned_abs();
    let before = state.caches.stats_window.swap(w, Ordering::Relaxed);
    if before != w || !counted.is_empty() {
        tracing::info!(
            stats_window_s = w,
            reason = %cap.stats_window.reason,
            "route-stats window from the disk"
        );
    }
}

/// Add `x-openlens-stats-window` (the window in force, seconds) to a
/// capture response for a server SDK.
pub(crate) fn advertise_window(state: &AppState, headers: &mut HeaderMap) {
    let w = state.caches.stats_window.load(Ordering::Relaxed);
    headers.insert(
        HeaderName::from_static(capacity::window::HEADER),
        HeaderValue::from(w),
    );
}

/// `GET /api/capacity` (admin): `capacity.get`.
async fn api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = openlens_engine::commands::capacity::GetInput::default();
    match openlens_engine::commands::capacity::Get::run(&state, &Caller::default(), input).await {
        Ok(c) => Json(c).into_response(),
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

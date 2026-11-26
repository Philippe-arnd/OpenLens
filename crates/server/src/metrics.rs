//! `GET /metrics` (Prometheus text) and `GET /readyz` (#296): what an
//! operator needs to run OpenLens for months unattended.
//!
//! `/metrics` needs the admin token or a `read` API token. `/readyz` is
//! open (probes carry no token) and says only ready or not, with a reason.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::AppState;

/// Counters kept since the start.
#[derive(Debug, Default)]
pub struct Metrics {
    pub(crate) capture_requests: AtomicU64,
    pub(crate) events_accepted: AtomicU64,
    pub(crate) events_rejected: AtomicU64,
    pub(crate) rate_limited: AtomicU64,
    pub(crate) disk_refused: AtomicU64,
    pub(crate) flushes: AtomicU64,
    pub(crate) flushed_events: AtomicU64,
    pub(crate) flush_ms: AtomicU64,
    pub(crate) last_flush_unix: AtomicU64,
}

impl Metrics {
    /// Add `n` to a counter.
    pub(crate) fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Ordering::Relaxed);
    }

    /// Record a flush of `events` that took `ms`.
    pub(crate) fn flushed(&self, events: usize, ms: u128) {
        Self::add(&self.flushes, 1);
        Self::add(&self.flushed_events, events as u64);
        Self::add(&self.flush_ms, u64::try_from(ms).unwrap_or(u64::MAX));
        self.last_flush_unix.store(
            u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0),
            Ordering::Relaxed,
        );
    }
}

fn line(out: &mut String, name: &str, kind: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP openlens_{name} {help}");
    let _ = writeln!(out, "# TYPE openlens_{name} {kind}");
    let _ = writeln!(out, "openlens_{name} {value}");
}

fn wal_bytes(state: &AppState) -> u64 {
    std::fs::read_dir(state.data_dir.join("wal"))
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// `GET /metrics`.
pub(crate) async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !crate::auth::has_bearer(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "a read token is required").into_response();
    }
    let m = &state.metrics;
    let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
    let (buffered, wal_syncs) = state
        .writer
        .lock()
        .map_or((0, 0), |w| (w.buffered() as u64, w.durable().syncs()));
    let (free, total) = (state.disk_space)(&state.data_dir).unwrap_or((0, 0));
    let last = get(&m.last_flush_unix);
    let now = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0);
    let mut out = String::new();
    for (name, kind, help, v) in [
        (
            "capture_requests_total",
            "counter",
            "Ingestion requests received.",
            get(&m.capture_requests),
        ),
        (
            "events_accepted_total",
            "counter",
            "Events accepted.",
            get(&m.events_accepted),
        ),
        (
            "events_rejected_total",
            "counter",
            "Events rejected as invalid.",
            get(&m.events_rejected),
        ),
        (
            "rate_limited_total",
            "counter",
            "Requests answered 429.",
            get(&m.rate_limited),
        ),
        (
            "wal_syncs_total",
            "counter",
            "WAL fsyncs; fewer than capture requests under load (group commit).",
            wal_syncs,
        ),
        (
            "query_timeouts_total",
            "counter",
            "SQL queries stopped at their time limit.",
            state.query.timeouts(),
        ),
        (
            "disk_refused_total",
            "counter",
            "Writes refused for lack of disk space.",
            get(&m.disk_refused),
        ),
        (
            "flushes_total",
            "counter",
            "Flushes of buffered events to Parquet.",
            get(&m.flushes),
        ),
        (
            "flushed_events_total",
            "counter",
            "Events written to Parquet.",
            get(&m.flushed_events),
        ),
        (
            "flush_milliseconds_total",
            "counter",
            "Time spent writing Parquet segments.",
            get(&m.flush_ms),
        ),
        (
            "last_flush_age_seconds",
            "gauge",
            "Seconds since the last flush (0: none yet).",
            if last == 0 {
                0
            } else {
                now.saturating_sub(last)
            },
        ),
        (
            "buffered_events",
            "gauge",
            "Events in the WAL, not yet in Parquet.",
            buffered,
        ),
        (
            "wal_bytes",
            "gauge",
            "Size of the write-ahead log.",
            wal_bytes(&state),
        ),
        (
            "board_computations_total",
            "counter",
            "Boards computed (the rest came from the cache).",
            state.boards.computed(),
        ),
        (
            "board_cache_entries",
            "gauge",
            "Board results kept in memory.",
            state.boards.len() as u64,
        ),
        (
            "cold_tier_blocked",
            "gauge",
            "1 when the cold tier is on but moves nothing (bucket lifecycle too short or unreadable).",
            u64::from(crate::cold::blocked(&state.meta).is_some()),
        ),
        (
            "disk_free_bytes",
            "gauge",
            "Free space on the data volume.",
            free,
        ),
        (
            "disk_total_bytes",
            "gauge",
            "Size of the data volume.",
            total,
        ),
    ] {
        line(&mut out, name, kind, help, v);
    }
    // Entries per in-memory cache (#309), one labelled series.
    out.push_str("# HELP openlens_cache_entries Entries held by each in-memory cache.\n# TYPE openlens_cache_entries gauge\n");
    for (name, n) in state.caches.sizes() {
        out.push_str(&format!("openlens_cache_entries{{cache=\"{name}\"}} {n}\n"));
    }
    ([("content-type", "text/plain; version=0.0.4")], out).into_response()
}

/// Why the server cannot take traffic now, if it cannot.
fn not_ready(state: &AppState) -> Option<&'static str> {
    if state.meta.setting("readyz").is_err() {
        return Some("metadata store unavailable");
    }
    let probe = state.data_dir.join("wal").join(".readyz");
    if std::fs::write(&probe, b"ok").is_err() {
        return Some("data volume not writable");
    }
    let _ = std::fs::remove_file(&probe);
    if let Some((free, total)) = (state.disk_space)(&state.data_dir)
        && crate::capture::space_low(free, total)
    {
        return Some("data volume almost full");
    }
    None
}

/// `GET /readyz`: 200 `ready`, or 503 with the reason.
pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    let st = state.clone();
    match tokio::task::spawn_blocking(move || not_ready(&st)).await {
        Ok(None) => "ready".into_response(),
        Ok(Some(why)) => (StatusCode::SERVICE_UNAVAILABLE, why).into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "check failed").into_response(),
    }
}

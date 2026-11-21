//! HTTP server: capture and flag endpoints, query API, health checks and the
//! dashboard.
//!
//! Layer: L4 (see `docs/architecture.md`).

mod api;
mod auth;
mod capture;
mod web;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use openlens_meta::Meta;
use openlens_storage::Writer;
use tower_http::cors::CorsLayer;

/// Largest accepted request body (compressed), in bytes.
pub const MAX_BODY: usize = 1024 * 1024;
/// Largest accepted body after gzip decompression, in bytes.
pub const MAX_INFLATED: u64 = 8 * 1024 * 1024;

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    /// Metadata store.
    pub meta: Arc<Meta>,
    /// Event writer (WAL + Parquet).
    pub writer: Arc<Mutex<Writer>>,
    /// Data root.
    pub data_dir: PathBuf,
    /// Admin token for the dashboard and API.
    pub admin_token: Arc<str>,
    /// Query engine over the data root.
    pub query: openlens_query::QueryEngine,
}

/// Build the application router.
pub fn router(state: AppState) -> Router {
    let capture = Router::new()
        .route("/capture", post(capture::capture))
        .route("/e", post(capture::capture))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(CorsLayer::permissive());
    Router::new()
        .route("/healthz", get(healthz))
        .route("/c.js", get(sdk))
        .route("/agents.md", get(agents_guide))
        .merge(capture)
        .merge(api::routes())
        .merge(web::routes())
        .with_state(state)
}

/// The browser SDK, embedded at build time (`sdk/js/dist`, committed).
const SDK_JS: &str = include_str!("../../../sdk/js/dist/openlens.min.js");

/// `GET /c.js`: the SDK for script-tag installs. Cached for an hour so that
/// SDK updates roll out quickly without hammering the server.
async fn sdk() -> impl axum::response::IntoResponse {
    (
        [
            ("content-type", "application/javascript; charset=utf-8"),
            ("cache-control", "public, max-age=3600"),
            ("access-control-allow-origin", "*"),
            ("x-content-type-options", "nosniff"),
        ],
        SDK_JS,
    )
}

/// The agent wiring guide, served so an agent can fetch it from the server
/// it is wiring an app to.
const AGENTS_MD: &str = include_str!("../../../docs/agents/wire.md");

async fn agents_guide() -> impl axum::response::IntoResponse {
    (
        [
            ("content-type", "text/markdown; charset=utf-8"),
            ("x-content-type-options", "nosniff"),
        ],
        AGENTS_MD,
    )
}

/// Liveness probe used by the container healthcheck.
async fn healthz() -> &'static str {
    "ok"
}

/// Run the periodic jobs: flush buffered events per the flush policy and
/// compact closed hours. Runs until the process exits.
pub async fn background(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut last_compact = std::time::Instant::now();
    loop {
        tick.tick().await;
        let st = state.clone();
        let res =
            tokio::task::spawn_blocking(move || flush_and_compact(&st, &mut last_compact)).await;
        match res {
            Ok(Ok(next)) => last_compact = next,
            Ok(Err(err)) => tracing::error!(%err, "background flush failed"),
            Err(err) => tracing::error!(%err, "background task crashed"),
        }
    }
}

fn flush_and_compact(
    state: &AppState,
    last_compact: &mut std::time::Instant,
) -> Result<std::time::Instant, String> {
    {
        let mut w = state.writer.lock().map_err(|_| "writer lock poisoned")?;
        if w.should_flush() {
            let n = w.flush().map_err(|e| e.to_string())?;
            tracing::debug!(n, "flushed events to parquet");
        }
    }
    if last_compact.elapsed() >= Duration::from_secs(600) {
        for p in state.meta.projects().map_err(|e| e.to_string())? {
            if let Err(err) = openlens_storage::compact(&state.data_dir, &p.id, chrono::Utc::now())
            {
                tracing::warn!(project = %p.id, %err, "compaction failed");
            }
        }
        return Ok(std::time::Instant::now());
    }
    Ok(*last_compact)
}

/// Flush everything still buffered (used on shutdown).
pub fn flush_now(state: &AppState) -> Result<usize, String> {
    let mut w = state.writer.lock().map_err(|_| "writer lock poisoned")?;
    w.flush().map_err(|e| e.to_string())
}

/// Constant-time string comparison.
pub(crate) fn token_matches(given: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    given.len() == expected.len() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
}

/// Public base URL of this server as the client sees it.
pub(crate) fn base_url(headers: &axum::http::HeaderMap) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .filter(|h| {
            h.len() < 256
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
        })
        .unwrap_or("localhost:8686");
    let proto = match headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        Some("https") => "https",
        _ => "http",
    };
    format!("{proto}://{host}")
}

/// The project's latest valid feature map, if it has one.
pub(crate) async fn load_map(
    state: &AppState,
    project: &str,
) -> Option<(openlens_featuremap::FeatureMap, i64)> {
    let meta = state.meta.clone();
    let p = project.to_owned();
    let (version, yaml) = tokio::task::spawn_blocking(move || meta.feature_map(&p))
        .await
        .ok()?
        .ok()??;
    openlens_featuremap::parse(&yaml).map.map(|m| (m, version))
}

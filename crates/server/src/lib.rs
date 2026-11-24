//! HTTP server: capture and flag endpoints, query API, health checks and the
//! dashboard.
//!
//! Layer: L4 (see `docs/architecture.md`).

mod alerts;
mod api;
mod ask;
mod audience;
mod auth;
pub mod backup;
mod backups;
pub mod boards;
mod capture;
mod comments;
mod cookieless;
mod csrf;
mod digest;
mod explore;
mod features;
mod flags;
mod goals;
mod icon;
mod llm;
mod manage;
pub mod metrics;
mod oidc;
mod privacy;
mod replay;
mod reword;
mod s3;
mod scopes;
pub mod secrets;
mod settings;
mod share;
mod storage;
mod surveys;
mod tickets;
mod timeline;
mod tls;
mod users;
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
    /// SHA-256 (hex) of the admin token: the token itself is never kept.
    pub admin_token_hash: Arc<str>,
    /// Signs session cookies and the OIDC flow ([`secrets::session_key`]).
    pub session_key: Arc<str>,
    /// Query engine over the data root.
    pub query: openlens_query::QueryEngine,
    /// Board results shared across requests (#291).
    pub boards: boards::BoardCache,
    /// Held while a flush writes Parquet, so flushes never overlap (#293).
    pub flushing: Arc<Mutex<()>>,
    /// Counters for `/metrics` (#296).
    pub metrics: Arc<metrics::Metrics>,
}

/// Build the application router.
pub fn router(state: AppState) -> Router {
    let capture = Router::new()
        .route("/capture", post(capture::capture))
        .route("/e", post(capture::capture))
        .route("/flags", get(flags::public))
        .route("/r", post(replay::ingest))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(CorsLayer::permissive())
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ));
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(metrics::readyz))
        .route("/metrics", get(metrics::metrics))
        .route("/version", get(version))
        .route("/c.js", get(sdk))
        .route("/r.js", get(recorder))
        .route("/s.js", get(survey_widget))
        .route("/agents.md", get(agents_guide))
        .merge(capture)
        .route("/api/projects/{project}/digest", get(digest::api))
        .route("/oidc/start", get(oidc::start))
        .route("/oidc/callback", get(oidc::callback))
        .merge(api::routes())
        .merge(flags::routes())
        .merge(surveys::routes())
        .merge(replay::routes())
        .merge(web::routes())
        .layer(axum::middleware::from_fn(csrf::layer))
        // Pages, the API and Ask (a few model rounds) get two minutes.
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(120),
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            scopes::layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            web::access_layer,
        ))
        .layer(axum::middleware::from_fn(web::theme_layer))
        // One span per request, tagged with its id; the id is returned in
        // `x-request-id` (kept from the proxy when it sent one) (#296).
        .layer(
            tower_http::trace::TraceLayer::new_for_http().make_span_with(
                |req: &axum::http::Request<axum::body::Body>| {
                    let id = req
                        .headers()
                        .get("x-request-id")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    tracing::info_span!("request", method = %req.method(), path = %req.uri().path(), id)
                },
            ),
        )
        .layer(tower_http::request_id::PropagateRequestIdLayer::x_request_id())
        .layer(tower_http::request_id::SetRequestIdLayer::x_request_id(
            tower_http::request_id::MakeRequestUuid,
        ))
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

/// The replay recorder (rrweb, MIT), loaded lazily by the SDK.
const RECORDER_JS: &str = include_str!("../../../sdk/js/dist/openlens-replay.min.js");

/// `GET /r.js`: the replay recorder bundle.
async fn recorder() -> impl axum::response::IntoResponse {
    (
        [
            ("content-type", "application/javascript; charset=utf-8"),
            ("cache-control", "public, max-age=3600"),
            ("access-control-allow-origin", "*"),
            ("x-content-type-options", "nosniff"),
        ],
        RECORDER_JS,
    )
}

/// The micro-survey widget, loaded by the SDK only when a survey is due.
const SURVEY_JS: &str = include_str!("../../../sdk/js/dist/openlens-survey.min.js");

/// `GET /s.js`: the survey widget bundle.
async fn survey_widget() -> impl axum::response::IntoResponse {
    (
        [
            ("content-type", "application/javascript; charset=utf-8"),
            ("cache-control", "public, max-age=3600"),
            ("access-control-allow-origin", "*"),
            ("x-content-type-options", "nosniff"),
        ],
        SURVEY_JS,
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

/// `GET /version` (#258): the running version, public, so a release can
/// check that an instance was redeployed (`/healthz` stays a bare `ok`).
async fn version() -> impl axum::response::IntoResponse {
    (
        [
            ("cache-control", "no-store"),
            ("access-control-allow-origin", "*"),
        ],
        axum::Json(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") })),
    )
}

/// `vX.Y.Z` as numbers; pre-releases and other tags are ignored.
fn stable_version(tag: &str) -> Option<(u64, u64, u64)> {
    let mut it = tag.strip_prefix('v').unwrap_or(tag).split('.');
    let v = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    it.next().is_none().then_some(v)
}

/// The newest stable release, when it is newer than `current` (#198).
/// Sends nothing about the install: one anonymous GET to GitHub's public
/// API. `OPENLENS_UPDATE_CHECK=off` turns it off.
fn newer_release(current: &str) -> Option<String> {
    let body: serde_json::Value =
        ureq::get("https://api.github.com/repos/Philippe-arnd/OpenLens/releases/latest")
            .header("user-agent", "openlens-update-check")
            .header("accept", "application/vnd.github+json")
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
    let tag = body.get("tag_name")?.as_str()?.to_owned();
    (stable_version(&tag)? > stable_version(current)?).then_some(tag)
}

/// Check for a newer release once a day (first check a minute after start).
async fn update_check() {
    if std::env::var("OPENLENS_UPDATE_CHECK").is_ok_and(|v| v == "off") {
        return;
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let found = tokio::task::spawn_blocking(|| newer_release(env!("CARGO_PKG_VERSION")))
            .await
            .ok()
            .flatten();
        if let Some(t) = &found {
            tracing::info!(release = %t, "a newer OpenLens release is available");
        }
        openlens_ui::set_update_available(found);
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

/// Run the periodic jobs: flush buffered events per the flush policy,
/// compact closed days and apply retention (D-015), and check for a newer release
/// once a day. Runs until the process exits.
pub async fn background(state: AppState) {
    tokio::spawn(update_check());
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut last_digest_check = std::time::Instant::now();
    let mut last_backup_check = std::time::Instant::now();
    let mut last_alert_check = std::time::Instant::now();
    let mut alert_minute: u64 = 0;
    // First S3 copy a few minutes after start, then hourly.
    let mut last_s3_sync = std::time::Instant::now()
        .checked_sub(Duration::from_secs(3300))
        .unwrap_or_else(std::time::Instant::now);
    // Run the first compaction and retention pass right away.
    let mut last_compact = std::time::Instant::now()
        .checked_sub(Duration::from_secs(600))
        .unwrap_or_else(std::time::Instant::now);
    loop {
        tick.tick().await;
        let st = state.clone();
        let res =
            tokio::task::spawn_blocking(move || flush_and_compact(&st, &mut last_compact)).await;
        if last_s3_sync.elapsed() >= Duration::from_secs(3600)
            && let Some((cfg, _)) = s3::configured(&state.meta, &state.data_dir)
        {
            last_s3_sync = std::time::Instant::now();
            let (root, meta) = (state.data_dir.clone(), state.meta.clone());
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            match tokio::task::spawn_blocking(move || {
                let r = s3::sync(&cfg, &root, &meta, &today);
                s3::record(&meta, &r);
                r
            })
            .await
            {
                Ok(Ok(0)) => {}
                Ok(Ok(n)) => tracing::info!(objects = n, "copied to S3"),
                Ok(Err(err)) => tracing::warn!(%err, "S3 copy failed; will retry next hour"),
                Err(err) => tracing::warn!(%err, "S3 copy task crashed"),
            }
        }
        if last_backup_check.elapsed() >= Duration::from_secs(60) {
            last_backup_check = std::time::Instant::now();
            backups::tick(&state).await;
        }
        if last_alert_check.elapsed() >= Duration::from_secs(60) {
            last_alert_check = std::time::Instant::now();
            alert_minute += 1;
            let (st, m) = (state.clone(), alert_minute);
            // Probes can be slow: never hold up the flush loop.
            tokio::spawn(async move { alerts::tick(&st, m).await });
        }
        if last_digest_check.elapsed() >= Duration::from_secs(600) {
            last_digest_check = std::time::Instant::now();
            digest::maybe_send(&state).await;
        }
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
        // Only the hand-over runs under the writer lock: captures keep
        // appending while the segments are written (#293).
        let _flushing = state.flushing.lock().map_err(|_| "flush lock poisoned")?;
        let pending = {
            let mut w = state.writer.lock().map_err(|_| "writer lock poisoned")?;
            if w.should_flush() {
                w.take().map_err(|e| e.to_string())?
            } else {
                None
            }
        };
        if let Some(pending) = pending {
            let started = std::time::Instant::now();
            match pending.write() {
                Ok(n) => {
                    state.metrics.flushed(n, started.elapsed().as_millis());
                    tracing::debug!(n, "flushed events to parquet");
                    state.boards.bump();
                }
                Err(e) => {
                    if let Ok(mut w) = state.writer.lock() {
                        w.restore(pending);
                    }
                    return Err(e.to_string());
                }
            }
        }
    }
    if last_compact.elapsed() >= Duration::from_secs(600) {
        let _guard = privacy::MAINTENANCE
            .lock()
            .map_err(|_| "maintenance lock poisoned")?;
        for p in state.meta.projects().map_err(|e| e.to_string())? {
            let now = chrono::Utc::now();
            clean_stored_urls(state, &p.id);
            // Compaction, erasure and retention below rewrite files.
            state.boards.bump();
            if let Err(err) = openlens_storage::compact(&state.data_dir, &p.id, now) {
                tracing::warn!(project = %p.id, %err, "compaction failed");
            }
            // People forgotten lately: rewritten out again, in case a
            // restore or a late flush brought rows back (#230).
            let recent = state
                .meta
                .forgotten_since(&p.id, now.timestamp() - 2 * 86400)
                .unwrap_or_default();
            if !recent.is_empty() {
                let ids: std::collections::HashSet<String> = recent.into_iter().collect();
                if let Err(err) = openlens_storage::erase(&state.data_dir, &p.id, &ids) {
                    tracing::warn!(project = %p.id, %err, "erasure re-apply failed");
                }
            }
            match openlens_storage::prune(&state.data_dir, &p.id, now) {
                Ok(0) => {}
                Ok(n) => tracing::info!(project = %p.id, days = n, "expired old event days"),
                Err(err) => tracing::warn!(project = %p.id, %err, "event retention failed"),
            }
            match openlens_replay::prune(&state.data_dir, &p.id, now) {
                Ok(0) => {}
                Ok(n) => tracing::info!(project = %p.id, recordings = n, "expired old recordings"),
                Err(err) => tracing::warn!(project = %p.id, %err, "replay retention failed"),
            }
        }
        return Ok(std::time::Instant::now());
    }
    Ok(*last_compact)
}

/// One-off rewrite of the URLs stored before capture cleaned them (#290),
/// recorded per project so it runs once. The caller holds the maintenance
/// lock.
fn clean_stored_urls(state: &AppState, project: &str) {
    const DONE: &str = "urls_cleaned";
    if state
        .meta
        .project_setting(project, DONE)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    let run = flush_now(state).and_then(|_| {
        openlens_storage::clean_urls(&state.data_dir, project).map_err(|e| e.to_string())
    });
    match run {
        Ok(n) => {
            tracing::info!(project, n, "query strings removed from stored URLs");
            if let Err(err) = state.meta.set_project_setting(project, DONE, "1") {
                tracing::warn!(project, %err, "URL cleaning not recorded");
            }
        }
        Err(err) => tracing::warn!(project, %err, "URL cleaning failed"),
    }
}

/// Flush everything still buffered (used on shutdown).
pub fn flush_now(state: &AppState) -> Result<usize, String> {
    let _flushing = state.flushing.lock().map_err(|_| "flush lock poisoned")?;
    let mut w = state.writer.lock().map_err(|_| "writer lock poisoned")?;
    let n = w.flush().map_err(|e| e.to_string())?;
    state.boards.bump();
    Ok(n)
}

/// Whether `given` is the admin token (hashes compared in constant time).
pub(crate) fn admin_token_matches(state: &AppState, given: &str) -> bool {
    token_matches(&openlens_meta::sha256_hex(given), &state.admin_token_hash)
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

/// The original source position of each error group's latest stack, using
/// the source maps uploaded for that occurrence's release.
pub(crate) async fn resolve_frames(
    state: &AppState,
    project: &str,
    groups: &[openlens_engine::ErrorGroup],
) -> Vec<Option<openlens_errors::Original>> {
    let root = state.data_dir.clone();
    let project = project.to_owned();
    let work: Vec<(Option<String>, Option<String>)> = groups
        .iter()
        .map(|g| (g.stack.clone(), g.last_release.clone()))
        .collect();
    tokio::task::spawn_blocking(move || {
        work.into_iter()
            .map(|(stack, release)| {
                openlens_errors::resolve(&root, &project, release.as_deref()?, stack.as_deref()?)
            })
            .collect()
    })
    .await
    .unwrap_or_else(|_| vec![None; groups.len()])
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

/// Run one alert pass now (tests and tools; the server runs it every
/// minute by itself).
#[doc(hidden)]
pub async fn alert_tick(state: &AppState, minute: u64) {
    alerts::tick(state, minute).await;
}

/// A project's flags (tests and tools).
#[doc(hidden)]
pub async fn flags_for_tests(state: &AppState, project: &str) -> Vec<openlens_flags::Flag> {
    flags::load(state, project).await
}

/// A short stable id for a text (alert rule keys).
pub(crate) fn short_hash(s: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(s.as_bytes()))[..12].to_owned()
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn only_newer_stable_releases_count() {
        use super::stable_version as v;
        assert!(v("v0.3.0") > v("0.2.0"));
        assert!(v("v0.10.0") > v("v0.9.9"));
        assert_eq!(v("v0.2.0-261122-755dcee"), None);
        assert_eq!(v("nightly"), None);
    }
}

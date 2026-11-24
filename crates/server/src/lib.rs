//! HTTP server: capture and flag endpoints, query API, health checks and the
//! dashboard.
//!
//! Layer: L5 (see `docs/architecture.md`).

mod access;
mod alerts;
mod api;
mod apierr;
mod ask;
mod audience;
mod auth;
pub mod backup;
mod backups;
pub mod boards;
pub mod cache;
mod capture;
mod cold;
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
mod llm_presets;
mod manage;
pub mod metrics;
mod oidc;
mod openapi;
mod privacy;
mod replay;
mod reword;
mod s3;
mod scopes;
pub mod secrets;
mod serve;
mod settings;
mod share;
mod storage;
mod surveys;
mod tickets;
mod timeline;
mod tls;
pub mod tokens;
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
    /// Free and total bytes of the volume holding a directory: [`disk_space`]
    /// in production; tests put a fake here to fill the disk (#346).
    pub disk_space: DiskSpace,
    /// Session generation (#342): part of every cookie's signature, bumped
    /// by *Sign out everywhere*. Load it with [`tokens::session_generation`].
    pub session_gen: Arc<std::sync::atomic::AtomicU64>,
    /// In-memory caches and limiters, each with a size limit (#309).
    pub caches: Arc<cache::Caches>,
}

/// How the server reads the free space of its data volume.
pub type DiskSpace = fn(&std::path::Path) -> Option<(u64, u64)>;

pub use capture::disk_space;
pub use serve::{HEADER_READ_TIMEOUT, serve};

/// The OpenAPI document of the API (#312), for clients to check
/// themselves against.
pub fn openapi_document() -> serde_json::Value {
    openapi::document()
}

/// Build the application router.
pub fn router(state: AppState) -> Router {
    openlens_ui::set_prefs_source(web::current_prefs);
    openlens_ui::set_nav_project_source(web::current_nav_project);
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
    let app = Router::new()
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
        .route("/api/openapi.json", get(openapi::serve))
        .route("/oidc/start", get(oidc::start))
        .route("/oidc/callback", get(oidc::callback))
        .merge(api::routes())
        .merge(flags::routes())
        .merge(surveys::routes())
        .merge(replay::routes())
        .merge(web::routes())
        .layer(axum::middleware::from_fn(csrf::layer))
        .layer(axum::middleware::from_fn(trusted_headers_layer))
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
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            web::nav_layer,
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
        .with_state(state);
    // `/api/v1/...` is the versioned prefix of the same routes (#312): the
    // request goes through the whole app again as `/api/...`.
    let v1 = tower::ServiceExt::map_request(app.clone(), |mut req: axum::extract::Request| {
        let rest = req.uri().path_and_query().map_or("/", |p| p.as_str());
        if let Ok(uri) = format!("/api{rest}").parse() {
            *req.uri_mut() = uri;
        }
        req
    });
    app.nest_service("/api/v1", v1)
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
fn newer_release(current: &str) -> Option<openlens_ui::Update> {
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
    release_update(&body, current)
}

/// Read a `/releases/latest` answer: the tag when newer than `current`,
/// and whether its notes carry a `Security` heading (#416: the release
/// fixes a published advisory, so the footer says so louder).
fn release_update(body: &serde_json::Value, current: &str) -> Option<openlens_ui::Update> {
    let tag = body.get("tag_name")?.as_str()?.to_owned();
    if stable_version(&tag)? <= stable_version(current)? {
        return None;
    }
    let security = body
        .get("body")
        .and_then(|b| b.as_str())
        .is_some_and(|notes| {
            notes.lines().any(|l| {
                let l = l.trim();
                l.starts_with('#')
                    && l.trim_start_matches('#')
                        .trim()
                        .eq_ignore_ascii_case("security")
            })
        });
    Some(openlens_ui::Update { tag, security })
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
        if let Some(u) = &found {
            if u.security {
                tracing::warn!(release = %u.tag, "a newer OpenLens release fixes security advisories");
            } else {
                tracing::info!(release = %u.tag, "a newer OpenLens release is available");
            }
        }
        openlens_ui::set_update_available(found);
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

/// Run the periodic jobs: flush buffered events per the flush policy,
/// compact closed days and apply retention (D-015), and check for a newer release
/// once a day. Runs until the process exits.
pub async fn background(state: AppState) {
    run_jobs(
        state,
        |st| Box::pin(async move { digest::maybe_send(&st).await }),
        Duration::from_secs(600),
    )
    .await;
}

/// A periodic job, as a future factory (tests swap the digest for one
/// that never finishes, #311).
type Job = fn(AppState) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// One task per periodic job (#311), each on its own interval, so a slow
/// one (an SMTP server that hangs, a long backup) never holds up the
/// flush: events still reach Parquet every few seconds. Backups, the S3
/// copy and compaction take turns on one lock, as they did in one loop.
async fn run_jobs(state: AppState, digest: Job, digest_first: Duration) {
    use tokio::time::{MissedTickBehavior, interval_at};
    tokio::spawn(update_check());
    let heavy = Arc::new(tokio::sync::Mutex::new(()));
    let every = |first: Duration, period: Duration| {
        let mut t = interval_at(tokio::time::Instant::now() + first, period);
        t.set_missed_tick_behavior(MissedTickBehavior::Delay);
        t
    };
    // Flush: every 2 s, when the buffer is due.
    let st = state.clone();
    let mut t = every(Duration::ZERO, Duration::from_secs(2));
    let flusher = tokio::spawn(async move {
        loop {
            t.tick().await;
            let s = st.clone();
            match tokio::task::spawn_blocking(move || flush(&s)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::error!(%err, "background flush failed"),
                Err(err) => tracing::error!(%err, "background flush crashed"),
            }
        }
    });
    // Compaction and retention: at start, then every 10 minutes.
    let (st, lock) = (state.clone(), heavy.clone());
    let mut t = every(Duration::ZERO, Duration::from_secs(600));
    tokio::spawn(async move {
        loop {
            t.tick().await;
            let _turn = lock.lock().await;
            let s = st.clone();
            match tokio::task::spawn_blocking(move || compact(&s)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::error!(%err, "compaction failed"),
                Err(err) => tracing::error!(%err, "compaction crashed"),
            }
            roll_up(&st, chrono::Utc::now()).await;
        }
    });
    // S3 copy: 5 minutes after start, then hourly.
    let (st, lock) = (state.clone(), heavy.clone());
    let mut t = every(Duration::from_secs(300), Duration::from_secs(3600));
    tokio::spawn(async move {
        loop {
            t.tick().await;
            let Some((cfg, _)) = s3::configured(&st.meta, &st.data_dir) else {
                continue;
            };
            let _turn = lock.lock().await;
            let (root, meta) = (st.data_dir.clone(), st.meta.clone());
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            match tokio::task::spawn_blocking(move || {
                let r = s3::sync(&cfg, &root, &meta, &today);
                s3::record(&meta, &r);
                if r.is_ok() {
                    cold_tier(&cfg, &root, &meta);
                }
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
    });
    // Backups: checked every minute against the plan.
    let (st, lock) = (state.clone(), heavy);
    let mut t = every(Duration::from_secs(60), Duration::from_secs(60));
    tokio::spawn(async move {
        loop {
            t.tick().await;
            let _turn = lock.lock().await;
            backups::tick(&st).await;
        }
    });
    // Alerts: every minute; probes can be slow, so each round is its own task.
    let st = state.clone();
    let mut t = every(Duration::from_secs(60), Duration::from_secs(60));
    tokio::spawn(async move {
        let mut minute: u64 = 0;
        loop {
            t.tick().await;
            minute += 1;
            let s = st.clone();
            tokio::spawn(async move { alerts::tick(&s, minute).await });
        }
    });
    // Weekly digest: checked every 10 minutes.
    let st = state;
    let mut t = every(digest_first, Duration::from_secs(600));
    tokio::spawn(async move {
        loop {
            t.tick().await;
            digest(st.clone()).await;
        }
    });
    // The flush task runs for the life of the process.
    let _ = flusher.await;
}

fn flush(state: &AppState) -> Result<(), String> {
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
                    for project in pending.projects() {
                        state.boards.bump(&project);
                    }
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
    Ok(())
}

/// Compaction, erasure re-apply and retention for every project.
fn compact(state: &AppState) -> Result<(), String> {
    let _guard = privacy::MAINTENANCE
        .lock()
        .map_err(|_| "maintenance lock poisoned")?;
    for p in state.meta.projects().map_err(|e| e.to_string())? {
        let now = chrono::Utc::now();
        clean_stored_urls(state, &p.id);
        // Compaction, erasure and retention below rewrite files.
        state.boards.bump(&p.id);
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
        // Click labels and positions go after 30 days, once rolled up (#324).
        match openlens_storage::trim_clicks(&state.data_dir, &p.id, now) {
            Ok(0) => {}
            Ok(n) => tracing::info!(project = %p.id, clicks = n, "trimmed old click detail"),
            Err(err) => tracing::warn!(project = %p.id, %err, "click trimming failed"),
        }
        match openlens_replay::prune(&state.data_dir, &p.id, now) {
            Ok(0) => {}
            Ok(n) => tracing::info!(project = %p.id, recordings = n, "expired old recordings"),
            Err(err) => tracing::warn!(project = %p.id, %err, "replay retention failed"),
        }
    }
    Ok(())
}

/// The cold tier, after a good S3 copy (#326, D-031): people forgotten
/// since the last pass are erased from cold days too, then old days move
/// to S3 and cold days past retention go.
fn cold_tier(cfg: &s3::Config, root: &std::path::Path, meta: &openlens_meta::Meta) {
    const DONE: &str = "cold_erased_at";
    let Ok(_guard) = privacy::MAINTENANCE.lock() else {
        return;
    };
    let now = chrono::Utc::now();
    for p in meta.projects().unwrap_or_default() {
        let since = meta
            .project_setting(&p.id, DONE)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let ids: std::collections::HashSet<String> = meta
            .forgotten_since(&p.id, since)
            .unwrap_or_default()
            .into_iter()
            .collect();
        if !ids.is_empty() {
            match cold::erase(cfg, root, &p.id, &ids) {
                Ok(n) => tracing::info!(project = %p.id, events = n, "erased from cold days"),
                Err(err) => {
                    tracing::warn!(project = %p.id, %err, "cold erasure failed; retried next hour");
                    continue;
                }
            }
        }
        let _ = meta.set_project_setting(&p.id, DONE, &now.timestamp().to_string());
    }
    match cold::offload(cfg, root, now.date_naive()) {
        Ok(0) => {}
        Ok(n) => tracing::info!(days = n, "moved old event days to S3"),
        Err(err) => tracing::warn!(%err, "cold tier: move to S3 failed"),
    }
    match cold::expire(cfg, root, now.date_naive()) {
        Ok(0) => {}
        Ok(n) => tracing::info!(days = n, "expired cold days on S3"),
        Err(err) => tracing::warn!(%err, "cold tier: expiry failed"),
    }
}

/// Closed days rolled up per project and pass, so a first pass over a
/// long history does not hold the heavy lock for long (#322).
const ROLLUPS_PER_PASS: usize = 7;

/// Roll up the closed days that have no rollup yet (#322): every day at
/// the first start after an upgrade, a few per pass, then each day once it
/// is closed. A day erased meanwhile is skipped and done on the next pass.
async fn roll_up(state: &AppState, now: chrono::DateTime<chrono::Utc>) -> usize {
    let projects = match state.meta.projects() {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(%err, "rollups: cannot list projects");
            return 0;
        }
    };
    let mut written = 0;
    for p in projects {
        let days = match openlens_storage::rollup::missing(&state.data_dir, &p.id, now) {
            Ok(d) => d,
            Err(err) => {
                tracing::warn!(project = %p.id, %err, "rollups: cannot list days");
                continue;
            }
        };
        for day in days.into_iter().take(ROLLUPS_PER_PASS) {
            let generation = openlens_storage::rollup::generation();
            let label = day.format("%Y-%m-%d").to_string();
            let batches = match state.query.rollup_day(&p.id, &label).await {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(project = %p.id, day = %label, %err, "rollup failed");
                    continue;
                }
            };
            let Ok(_guard) = privacy::MAINTENANCE.lock() else {
                return written;
            };
            match openlens_storage::rollup::write(&state.data_dir, &p.id, day, generation, &batches)
            {
                Ok(true) => written += 1,
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(project = %p.id, day = %label, %err, "rollup not written")
                }
            }
        }
    }
    written
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
    let projects = w.buffered_projects();
    let n = w.flush().map_err(|e| e.to_string())?;
    for project in projects {
        state.boards.bump(&project);
    }
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

/// Public base URL of this server as the client sees it: the fixed
/// `OPENLENS_PUBLIC_URL` (or `SERVICE_URL_OPENLENS`) when one is set,
/// otherwise the request's `Host`, with `X-Forwarded-Host` and
/// `X-Forwarded-Proto` honoured only because [`trusted_headers_layer`] has
/// already stripped both from any connection that is not a trusted proxy
/// (SEC-06).
pub(crate) fn public_url(headers: &axum::http::HeaderMap) -> String {
    if let Ok(fixed) =
        std::env::var("OPENLENS_PUBLIC_URL").or_else(|_| std::env::var("SERVICE_URL_OPENLENS"))
    {
        let fixed = fixed.trim().trim_end_matches('/');
        if !fixed.is_empty() {
            return fixed.to_owned();
        }
    }
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

/// SEC-06: strip `X-Forwarded-Host` and `X-Forwarded-Proto` from any
/// connection that is not [`capture::trusted_proxy`], before anything else
/// in the app (the CSRF same-origin check, [`public_url`], the session
/// cookie, the OIDC redirect) can read them. A request with no
/// `ConnectInfo` (a direct local call, as in most tests) is treated as
/// trusted, the same as everywhere else this server decides who to trust.
///
/// Also sets the response headers that follow from the same public URL:
/// HSTS when it is HTTPS, and, on `/api`, `nosniff` and `no-store` since
/// those responses carry session-authenticated JSON.
pub(crate) async fn trusted_headers_layer(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{HeaderValue, header};

    let trusted = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_none_or(|c| capture::trusted_proxy(c.0.ip()));
    if !trusted {
        req.headers_mut().remove("x-forwarded-host");
        req.headers_mut().remove("x-forwarded-proto");
    }
    let https = public_url(req.headers()).starts_with("https://");
    let api = req.uri().path().starts_with("/api");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    if https {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    if api {
        h.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
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

    #[test]
    fn a_security_heading_makes_a_security_update() {
        use super::release_update as r;
        let plain = serde_json::json!({"tag_name": "v0.5.0", "body": "## Added\n- things"});
        let sec =
            serde_json::json!({"tag_name": "v0.5.0", "body": "## Added\n\n### Security\n- GHSA-x"});
        assert!(!r(&plain, "0.4.0").unwrap().security);
        assert!(r(&sec, "0.4.0").unwrap().security);
        assert_eq!(r(&sec, "0.4.0").unwrap().tag, "v0.5.0");
        assert!(r(&sec, "0.5.0").is_none(), "same version");
        assert!(
            r(&serde_json::json!({"body": "x"}), "0.4.0").is_none(),
            "no tag"
        );
        let no_body = serde_json::json!({"tag_name": "v0.5.0"});
        assert!(!r(&no_body, "0.4.0").unwrap().security);
        let mention = serde_json::json!({"tag_name": "v0.5.0", "body": "Security is great"});
        assert!(
            !r(&mention, "0.4.0").unwrap().security,
            "only a heading counts"
        );
    }
}

#[cfg(test)]
mod jobs_tests {
    #![allow(clippy::unwrap_used)]
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[tokio::test]
    async fn closed_days_are_rolled_up_a_few_per_pass() {
        let dir = tempfile::tempdir().unwrap();
        let meta = openlens_meta::Meta::in_memory().unwrap();
        let project = meta.create_project("Rolled", None).unwrap();
        let (mut writer, _) =
            openlens_storage::Writer::open(dir.path(), openlens_storage::FlushPolicy::default())
                .unwrap();
        let now: chrono::DateTime<chrono::Utc> = "2026-03-20T12:00:00Z".parse().unwrap();
        let evs: Vec<_> = (0..10)
            .map(|d| {
                let t = now - chrono::Duration::days(d);
                openlens_event::Event {
                    uuid: format!("0191f0c0-0000-7000-8000-{d:012}"),
                    project: project.id.clone(),
                    event: "$pageview".into(),
                    distinct_id: "u1".into(),
                    session_id: None,
                    timestamp: t,
                    received_at: t,
                    url: None,
                    path: None,
                    feature: None,
                    role: None,
                    account: None,
                    plan: None,
                    environment: "production".into(),
                    release: None,
                    flags: None,
                    properties: "{}".into(),
                }
            })
            .collect();
        writer.append(&evs).unwrap();
        writer.flush().unwrap();
        let state = crate::AppState {
            meta: Arc::new(meta),
            writer: Arc::new(Mutex::new(writer)),
            query: openlens_query::QueryEngine::new(dir.path(), openlens_query::Limits::default()),
            data_dir: dir.path().to_path_buf(),
            admin_token_hash: "x".into(),
            session_key: "k".into(),
            boards: Default::default(),
            flushing: Default::default(),
            metrics: Default::default(),
            disk_space: crate::disk_space,
            session_gen: Default::default(),
            caches: Default::default(),
        };
        // Ten days of events: today and yesterday are not closed yet.
        // A test erasing elsewhere in this process can make a pass skip a
        // day (it is redone next pass): count over passes.
        let first = super::roll_up(&state, now).await;
        assert!(first <= super::ROLLUPS_PER_PASS);
        let mut total = first;
        for _ in 0..5 {
            total += super::roll_up(&state, now).await;
        }
        assert_eq!(total, 8);
        assert_eq!(super::roll_up(&state, now).await, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hung_digest_never_holds_up_the_flush() {
        let dir = tempfile::tempdir().unwrap();
        let meta = openlens_meta::Meta::in_memory().unwrap();
        let policy = openlens_storage::FlushPolicy {
            max_events: 1,
            max_age: Duration::from_millis(100),
        };
        let (writer, _) = openlens_storage::Writer::open(dir.path(), policy).unwrap();
        let state = crate::AppState {
            meta: Arc::new(meta),
            writer: Arc::new(Mutex::new(writer)),
            query: openlens_query::QueryEngine::new(dir.path(), openlens_query::Limits::default()),
            data_dir: dir.path().to_path_buf(),
            admin_token_hash: "x".into(),
            session_key: "k".into(),
            boards: Default::default(),
            flushing: Default::default(),
            metrics: Default::default(),
            disk_space: crate::disk_space,
            session_gen: Default::default(),
            caches: Default::default(),
        };
        // The digest starts at once and never finishes, like an SMTP
        // server that accepts the connection and never answers.
        tokio::spawn(super::run_jobs(
            state.clone(),
            |_| Box::pin(std::future::pending()),
            Duration::ZERO,
        ));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let ev = openlens_event::Event {
            uuid: "0191f0c0-0000-7000-8000-000000000001".into(),
            project: "p1".into(),
            event: "$pageview".into(),
            distinct_id: "u1".into(),
            session_id: None,
            timestamp: chrono::Utc::now(),
            received_at: chrono::Utc::now(),
            url: None,
            path: None,
            feature: None,
            role: None,
            account: None,
            plan: None,
            environment: "production".into(),
            release: None,
            flags: None,
            properties: "{}".into(),
        };
        state.writer.lock().unwrap().append(&[ev]).unwrap();
        // The flush job ticks every 2 s: within 5 s the event is in Parquet.
        let mut flushed = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if state.writer.lock().unwrap().buffered() == 0 {
                flushed = true;
                break;
            }
        }
        assert!(flushed, "the flush waited for the digest");
    }
}

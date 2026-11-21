//! HTTP server: capture and flag endpoints, query API, health checks and the
//! embedded UI.
//!
//! Layer: L4 (see `docs/architecture.md`). Only `/healthz` exists today; the
//! capture API lands in milestone M1 (`ROADMAP.md`).

use axum::{Router, routing::get};

/// Build the application router.
pub fn router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

/// Liveness probe used by the container healthcheck.
async fn healthz() -> &'static str {
    "ok"
}

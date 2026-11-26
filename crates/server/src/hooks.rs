//! `POST /hooks/deploy/{secret}` (#504, part 3 of #237, D-053): deploy
//! notifications from Coolify, Vercel, Netlify, Render, Fly, GitHub
//! Deployments, Argo CD, Flux, or a plain JSON form. The engine reads the
//! format ([`openlens_engine::deploys`]) and records the deploy
//! (`deploys.record`).
//!
//! The secret is the only credential, so:
//! - a wrong secret answers 404, like a route that does not exist, so a
//!   scan cannot tell a hook from nothing;
//! - only its hash is stored; the request log shows the path as
//!   `/hooks/deploy/<secret>` ([`log_path`]);
//! - requests per client address are limited before the secret is looked
//!   up, and deploys per project after, with the capture limiter's token
//!   buckets; the body is at most [`MAX_HOOK_BODY`].

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use openlens_engine::commands::deploys::{Record, RecordInput};
use openlens_engine::commands::{Caller, Command};
use openlens_engine::deploys::{self, Hints, Parsed, Status};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::capture::{Peer, client_ip, error};

/// Largest hook body: a GitHub `deployment_status` carries the repository
/// and the sender, an Argo CD template may carry the whole Application.
pub(crate) const MAX_HOOK_BODY: usize = 256 * 1024;

/// Hook requests a second per client address, and the burst allowed.
const IP_RATE: f64 = 1.0;
const IP_BURST: f64 = 30.0;
/// Deploys a second per project (one every 10 s), and the burst allowed.
const PROJECT_RATE: f64 = 0.1;
const PROJECT_BURST: f64 = 30.0;

/// The path prefix whose last segment is a secret.
const PREFIX: &str = "/hooks/deploy/";

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/hooks/deploy/{secret}", post(deploy))
        .layer(DefaultBodyLimit::max(MAX_HOOK_BODY))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
}

/// The path as logged: the secret of a deploy hook is never written.
pub(crate) fn log_path(path: &str) -> &str {
    if path.starts_with(PREFIX) {
        "/hooks/deploy/<secret>"
    } else {
        path
    }
}

/// The hook's URL for a secret, on this server.
pub(crate) fn url(headers: &HeaderMap, secret: &str) -> String {
    format!("{}{PREFIX}{secret}", crate::public_url(headers))
}

#[derive(Deserialize, Default)]
pub(crate) struct HookQuery {
    /// The environment, for platforms whose notification does not say it
    /// (Render, Fly): wins over the notification's.
    #[serde(default)]
    env: Option<String>,
}

fn limited(state: &AppState, key: &str, rate: f64, burst: f64) -> bool {
    !state
        .caches
        .hook_buckets
        .lock()
        .is_ok_and(|mut b| b.take(key, 1.0, rate, burst))
}

fn too_many() -> Response {
    let mut r = error(
        StatusCode::TOO_MANY_REQUESTS,
        "too many deploy notifications; retry in a minute",
    );
    r.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("60"),
    );
    r
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// `POST /hooks/deploy/{secret}[?env=]`.
pub(crate) async fn deploy(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Path(secret): Path<String>,
    Query(q): Query<HookQuery>,
    body: Bytes,
) -> Response {
    if let Some(ip) = client_ip(&peer, &headers)
        && limited(&state, &format!("ip:{ip}"), IP_RATE, IP_BURST)
    {
        return too_many();
    }
    let meta = state.meta.clone();
    let found = tokio::task::spawn_blocking(move || meta.deploy_hook_project(&secret)).await;
    let project = match found {
        Ok(Ok(Some(p))) => p,
        Ok(Ok(None)) => return StatusCode::NOT_FOUND.into_response(),
        _ => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "metadata store unavailable",
            );
        }
    };
    if limited(
        &state,
        &format!("project:{project}"),
        PROJECT_RATE,
        PROJECT_BURST,
    ) {
        tracing::warn!(project = %project, "deploy hook rate limited");
        return too_many();
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return error(StatusCode::BAD_REQUEST, "the body must be JSON");
    };
    let hints = Hints {
        github_event: header(&headers, "x-github-event"),
        flux_component: header(&headers, "gotk-component"),
    };
    let notice = match deploys::parse(hints, &value) {
        Ok(Parsed::Deploy(n)) => n,
        Ok(Parsed::Ignored(why)) => {
            return (StatusCode::ACCEPTED, Json(json!({ "ignored": why }))).into_response();
        }
        Err(why) => return error(StatusCode::BAD_REQUEST, why),
    };
    let now = chrono::Utc::now().timestamp();
    let d = match deploys::finish(notice, q.env.as_deref(), now) {
        Ok(d) => d,
        Err(why) => return error(StatusCode::BAD_REQUEST, why),
    };
    let input = RecordInput {
        project: project.clone(),
        environment: d.environment.clone(),
        release: d.release.clone(),
        commit: d.commit.clone(),
        status: Status::parse(&d.status).unwrap_or(Status::Started),
        at: d.at,
        source: d.source.clone(),
    };
    match Record::run(&state, &Caller::default(), input).await {
        Ok(()) => Json(json!({
            "recorded": true,
            "source": d.source,
            "environment": d.environment,
            "release": d.release,
            "commit": d.commit,
            "status": d.status,
            "at": d.at,
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!(project = %project, %e, "deploy not recorded");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "the deploy could not be recorded",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_secret_is_never_in_the_logged_path() {
        let p = super::log_path("/hooks/deploy/dh_abcdef");
        assert!(!p.contains("dh_abcdef"));
        assert_eq!(super::log_path("/p/demo"), "/p/demo");
    }
}

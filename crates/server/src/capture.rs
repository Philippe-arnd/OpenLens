//! `POST /capture`: batched event ingestion.
//!
//! Accepts JSON, optionally gzip-compressed (by `Content-Encoding: gzip` or
//! the gzip magic bytes, since `navigator.sendBeacon` cannot set headers).
//! The project key comes from the `X-OpenLens-Key` header, the `key` query
//! parameter or the body, in that order.

use std::collections::HashMap;
use std::io::Read;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use openlens_event::{CaptureBatch, names, normalize_batch};
use serde_json::json;

use crate::{AppState, MAX_INFLATED};

pub(crate) fn error(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// The request body, gunzipped when needed (by header or magic bytes) and
/// bounded by [`MAX_INFLATED`].
pub(crate) fn inflate(headers: &HeaderMap, body: &Bytes) -> Result<Vec<u8>, Box<Response>> {
    let gzip = headers
        .get("content-encoding")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"gzip"))
        || body.starts_with(&[0x1f, 0x8b]);
    if !gzip {
        return Ok(body.to_vec());
    }
    let mut out = Vec::new();
    let mut dec = flate2::read::GzDecoder::new(&body[..]).take(MAX_INFLATED + 1);
    if dec.read_to_end(&mut out).is_err() {
        return Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "invalid gzip body",
        )));
    }
    if out.len() as u64 > MAX_INFLATED {
        return Err(Box::new(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body too large once decompressed",
        )));
    }
    Ok(out)
}

/// The project a capture key names, or the error response to send.
pub(crate) async fn project_for_key(
    state: &AppState,
    key: Option<String>,
) -> Result<openlens_meta::Project, Box<Response>> {
    let Some(key) = key else {
        return Err(Box::new(error(
            StatusCode::UNAUTHORIZED,
            "missing project key",
        )));
    };
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.project_by_key(&key)).await {
        Ok(Ok(Some(p))) => Ok(p),
        Ok(Ok(None)) => Err(Box::new(error(
            StatusCode::UNAUTHORIZED,
            "unknown project key",
        ))),
        _ => Err(Box::new(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata store unavailable",
        ))),
    }
}

/// Environment given to events sent by robots.
pub(crate) const BOT_ENVIRONMENT: &str = "bot";

/// A user agent that names itself a crawler, a preview fetcher or an uptime
/// monitor. Server SDKs, curl and headless browsers used for tests are not.
pub(crate) fn is_bot(ua: &str) -> bool {
    let ua = ua.to_ascii_lowercase();
    ua.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w.ends_with("bot"))
        || [
            "crawler",
            "spider",
            "slurp",
            "facebookexternalhit",
            "bingpreview",
            "lighthouse",
            "pingdom",
            "uptimerobot",
            "statuscake",
        ]
        .iter()
        .any(|k| ua.contains(k))
}

pub(crate) async fn capture(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let raw = match inflate(&headers, &body) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };

    let batch: CaptureBatch = match serde_json::from_slice(&raw) {
        Ok(b) => b,
        Err(_) => return error(StatusCode::BAD_REQUEST, "body is not a valid capture batch"),
    };

    let key = headers
        .get("x-openlens-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| query.get("key").cloned())
        .or_else(|| batch.key.clone());
    let project = match project_for_key(&state, key).await {
        Ok(p) => p,
        Err(resp) => return *resp,
    };

    let mut normalized = match normalize_batch(&project.id, batch, Utc::now()) {
        Ok(n) => n,
        Err(e) => return error(StatusCode::PAYLOAD_TOO_LARGE, &e.to_string()),
    };
    // Crawlers and monitors posting directly (the SDK already skips them in
    // browsers): kept, but in their own environment, out of every number.
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if is_bot(ua) {
        for ev in &mut normalized.events {
            ev.environment = BOT_ENVIRONMENT.to_owned();
        }
    }
    let accepted = normalized.events.len();
    let rejected: Vec<_> = normalized
        .rejected
        .iter()
        .map(|(i, e)| json!({ "index": i, "reason": e.to_string() }))
        .collect();

    let st = state.clone();
    let events = normalized.events;
    let stored = tokio::task::spawn_blocking(move || -> Result<(), String> {
        for ev in events.iter().filter(|e| e.event == names::IDENTIFY) {
            st.meta
                .identify(
                    &ev.project,
                    &ev.distinct_id,
                    ev.role.as_deref(),
                    ev.account.as_deref(),
                    ev.plan.as_deref(),
                    ev.timestamp,
                )
                .map_err(|e| e.to_string())?;
        }
        let mut w = st
            .writer
            .lock()
            .map_err(|_| "writer lock poisoned".to_owned())?;
        w.append(&events).map_err(|e| e.to_string())
    })
    .await;
    match stored {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({ "accepted": accepted, "rejected": rejected })),
        )
            .into_response(),
        Ok(Err(err)) => {
            tracing::error!(%err, "cannot store events");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "cannot store events right now",
            )
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "storage task failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::is_bot;

    #[test]
    fn robots_are_told_apart_from_people_and_server_sdks() {
        for bot in [
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
            "Mozilla/5.0 (compatible; bingbot/2.0)",
            "facebookexternalhit/1.1",
            "Mozilla/5.0 (compatible; AhrefsBot/7.0)",
            "UptimeRobot/2.0",
            "Mozilla/5.0 (Linux; Android 11; moto g power (2022)) AppleWebKit/537.36 Chrome-Lighthouse",
        ] {
            assert!(is_bot(bot), "{bot}");
        }
        for person in [
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 Version/17.5 Safari/605.1.15",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/131.0 Safari/537.36",
            "openlens-node/0.1.0",
            "python-urllib/3.12",
            "curl/8.5.0",
            "",
        ] {
            assert!(!is_bot(person), "{person}");
        }
    }
}

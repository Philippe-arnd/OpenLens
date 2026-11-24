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

/// Token buckets: `rate` tokens a second, up to `burst`.
#[derive(Default)]
pub(crate) struct Buckets {
    map: HashMap<String, (f64, std::time::Instant)>,
}

/// Keys a [`Buckets`] holds at most (then idle ones go, then the oldest).
const MAX_BUCKETS: usize = 50_000;

impl Buckets {
    /// Keys held now.
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    /// Take `cost` tokens from `key`'s bucket; false when it is empty.
    fn take(&mut self, key: &str, cost: f64, rate: f64, burst: f64) -> bool {
        let now = std::time::Instant::now();
        if self.map.len() > MAX_BUCKETS {
            // Forget idle keys (a full bucket is the same as no entry).
            self.map.retain(|_, (tokens, at)| {
                *tokens + now.duration_since(*at).as_secs_f64() * rate < burst
            });
            // Still too many busy keys (a flood from many addresses): keep
            // the most recently seen, so memory stays bounded (#309).
            if self.map.len() > MAX_BUCKETS {
                let mut seen: Vec<std::time::Instant> =
                    self.map.values().map(|(_, at)| *at).collect();
                seen.sort_unstable();
                let cut = seen[seen.len() - MAX_BUCKETS / 2];
                self.map.retain(|_, (_, at)| *at >= cut);
            }
        }
        let (tokens, at) = self.map.entry(key.to_owned()).or_insert((burst, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * rate).min(burst);
        *at = now;
        if *tokens < cost {
            return false;
        }
        *tokens -= cost;
        true
    }
}

/// Requests a second per client address, and events a second per project
/// key, for traffic that comes through a proxy: generous for real apps (a busy browser sends a batch every few
/// seconds), but a runaway client or a hostile script cannot take the node.
/// No setting (D-014). Clients get a 429 and the SDK retries later.
pub(crate) const IP_RATE: f64 = 50.0;
const IP_BURST: f64 = 500.0;
pub(crate) const KEY_RATE: f64 = 20_000.0;
const KEY_BURST: f64 = 200_000.0;

/// The connection's peer address, when the server knows it (always in
/// production; tests that call the router directly have none).
pub(crate) type Peer = Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>;

/// A proxy we take `X-Forwarded-For` from: the host itself or a private
/// network (Docker, Compose, a cluster), never the internet.
fn trusted_proxy(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        std::net::IpAddr::V6(v) => {
            v.is_loopback() || v.is_unique_local() || v.is_unicast_link_local()
        }
    }
}

/// Who to rate-limit: the connection's address, or, behind a trusted
/// proxy, the client it names (the last `X-Forwarded-For` entry is the one
/// our proxy added; earlier ones come from the client). `None` for direct
/// local traffic (server SDKs on the host, benchmarks, health checks).
pub(crate) fn client_ip(peer: &Peer, headers: &HeaderMap) -> Option<String> {
    let peer = peer.as_ref().map(|axum::Extension(c)| c.0.ip());
    if let Some(ip) = peer
        && !trusted_proxy(ip)
    {
        return Some(ip.to_string());
    }
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    match (forwarded, peer) {
        (Some(f), _) => Some(f),
        (None, Some(ip)) if !ip.is_loopback() => Some(ip.to_string()),
        _ => None,
    }
}

/// Whether this client sent too many requests (checked before any parsing).
pub(crate) fn ip_limited(state: &AppState, peer: &Peer, headers: &HeaderMap) -> bool {
    let Some(ip) = client_ip(peer, headers) else {
        return false;
    };
    !state
        .caches
        .ip_buckets
        .lock()
        .is_ok_and(|mut b| b.take(&ip, 1.0, IP_RATE, IP_BURST))
}

fn key_limited(state: &AppState, key: &str, events: usize) -> bool {
    !state
        .caches
        .key_buckets
        .lock()
        .is_ok_and(|mut b| b.take(key, events as f64, KEY_RATE, KEY_BURST))
}

/// 429 with a hint for the SDK.
pub(crate) fn too_many() -> Response {
    let mut r = error(
        StatusCode::TOO_MANY_REQUESTS,
        "too many events; retry in a few seconds",
    );
    r.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("5"),
    );
    r
}

/// Writes stop below this share of free space on the data volume, so the
/// WAL, compaction and backups still have room. Fixed (D-014).
const MIN_FREE_SHARE: f64 = 0.05;

/// Whether `available` of `total` bytes is too little to accept writes.
pub(crate) fn space_low(available: u64, total: u64) -> bool {
    total > 0 && (available as f64) < (total as f64) * MIN_FREE_SHARE
}

/// Free and total bytes of the volume holding `dir`.
#[cfg(unix)]
pub fn disk_space(dir: &std::path::Path) -> Option<(u64, u64)> {
    let s = rustix::fs::statvfs(dir).ok()?;
    Some((
        s.f_bavail.saturating_mul(s.f_frsize),
        s.f_blocks.saturating_mul(s.f_frsize),
    ))
}

/// Free and total bytes of the volume holding `dir` (unknown here).
#[cfg(not(unix))]
pub fn disk_space(_dir: &std::path::Path) -> Option<(u64, u64)> {
    None
}

/// 503 when the data volume is nearly full; logged once a minute at most.
pub(crate) fn disk_full(state: &AppState) -> Option<Response> {
    let (available, total) = (state.disk_space)(&state.data_dir)?;
    if !space_low(available, total) {
        return None;
    }
    if let Ok(mut last) = state.caches.disk_logged.lock()
        && last.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(60))
    {
        *last = Some(std::time::Instant::now());
        tracing::error!(available, total, "data volume almost full: writes refused");
    }
    let mut r = error(
        StatusCode::SERVICE_UNAVAILABLE,
        "the server is out of disk space; retry later",
    );
    r.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("60"),
    );
    Some(r)
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
    peer: Peer,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    use crate::metrics::Metrics;
    let m = state.metrics.clone();
    Metrics::add(&m.capture_requests, 1);
    if ip_limited(&state, &peer, &headers) {
        Metrics::add(&m.rate_limited, 1);
        return too_many();
    }
    if let Some(r) = disk_full(&state) {
        Metrics::add(&m.disk_refused, 1);
        return r;
    }
    // Up to 8 MB to gunzip and parse: off the async threads (#295).
    let h = headers.clone();
    let parsed = tokio::task::spawn_blocking(move || -> Result<CaptureBatch, Box<Response>> {
        let raw = inflate(&h, &body)?;
        serde_json::from_slice(&raw).map_err(|_| {
            Box::new(error(
                StatusCode::BAD_REQUEST,
                "body is not a valid capture batch",
            ))
        })
    })
    .await;
    let batch = match parsed {
        Ok(Ok(b)) => b,
        Ok(Err(resp)) => return *resp,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "capture task failed"),
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

    let pid = project.id.clone();
    let normalized =
        tokio::task::spawn_blocking(move || normalize_batch(&pid, batch, Utc::now())).await;
    let mut normalized = match normalized {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => return error(StatusCode::PAYLOAD_TOO_LARGE, &e.to_string()),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "capture task failed"),
    };
    // Direct local traffic (server SDKs on the host, benchmarks) has no
    // per-key budget either.
    if client_ip(&peer, &headers).is_some()
        && key_limited(&state, &project.api_key, normalized.events.len())
    {
        tracing::warn!(project = %project.id, "capture rate limited");
        Metrics::add(&m.rate_limited, 1);
        return too_many();
    }
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
    crate::cookieless::resolve(&state, &mut normalized.events, &peer, &headers);
    // Forgotten people stay forgotten (#230).
    normalized.events.retain(|e| {
        !state
            .meta
            .is_forgotten(&e.project, &e.distinct_id)
            .unwrap_or(false)
    });
    crate::audience::init(&state.data_dir);
    crate::audience::enrich(&mut normalized.events, &peer, &headers);
    let accepted = normalized.events.len();
    Metrics::add(&m.events_accepted, accepted as u64);
    Metrics::add(&m.events_rejected, normalized.rejected.len() as u64);
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
        // Write under the lock, wait for the disk outside it, so one fsync
        // serves every request that arrives meanwhile (#333).
        let (ticket, durable) = {
            let mut w = st
                .writer
                .lock()
                .map_err(|_| "writer lock poisoned".to_owned())?;
            (w.write(&events).map_err(|e| e.to_string())?, w.durable())
        };
        durable.wait(ticket).map_err(|e| e.to_string())
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
    fn writes_stop_when_the_volume_is_nearly_full() {
        assert!(super::space_low(1, 100));
        assert!(!super::space_low(50, 100));
        assert!(!super::space_low(0, 0), "unknown size: keep accepting");
    }

    #[test]
    fn buckets_refill_and_cap() {
        let mut b = super::Buckets::default();
        assert!(b.take("k", 10.0, 1.0, 10.0));
        assert!(!b.take("k", 1.0, 1.0, 10.0), "empty after the burst");
        assert!(b.take("other", 10.0, 1.0, 10.0), "buckets are per key");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(b.take("k", 1.0, 1.0, 10.0), "refilled at the rate");
    }

    #[test]
    fn buckets_stay_bounded_under_a_flood_of_addresses() {
        let mut b = super::Buckets::default();
        // Every key spends tokens, so none is idle and the first sweep
        // alone would keep them all.
        for i in 0..(super::MAX_BUCKETS + 5_000) {
            b.take(&format!("ip-{i}"), 5.0, 1.0, 10.0);
            assert!(b.len() <= super::MAX_BUCKETS + 1);
        }
    }

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

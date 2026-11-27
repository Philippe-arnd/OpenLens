//! `POST /v1/metrics` (#605, D-085): OpenTelemetry metrics over OTLP/HTTP,
//! so an app already instrumented with OpenTelemetry sends its route
//! stats with no OpenLens server SDK. Metrics only, no spans.
//!
//! - **Encodings:** protobuf (`application/x-protobuf`) and JSON
//!   (`application/json`); the answer uses the request's. Gzip as on
//!   `/capture`. gRPC is out of scope.
//! - **Read:** the `http.server.request.duration` explicit-bucket
//!   histogram with `http.request.method`, `http.route` and
//!   `http.response.status_code`; resource `service.version` is the
//!   release and `deployment.environment.name` the environment. Every
//!   other metric and data point is counted in
//!   `partialSuccess.rejectedDataPoints`.
//! - **Stored:** each data point becomes one `$request_stats` event with
//!   the fields the Node SDK sends (D-084), so the routes, the verdict and
//!   the alerts read it unchanged. Durations become seconds; other bucket
//!   bounds are re-bucketed into OpenLens's by upper bound, so p95 stays
//!   an upper bound.
//! - **Temporality:** delta points are stored as they are. For cumulative
//!   ones the last point of each series (project, resource, attributes,
//!   start time) is kept in memory ([`crate::cache::Caches::otlp_series`])
//!   and the difference is stored. A new start time is a new series; a
//!   count that went down starts over. A series first seen that may have
//!   been counted before (it started before the server or before the last
//!   series it forgot) only sets the baseline: never a double count.
//! - **Safety:** the project key (`x-openlens-key` or `Authorization:
//!   Bearer`), the per-client and per-key limits of `/capture` (a data
//!   point costs one event), the disk check, a 4 MB body before and after
//!   gzip, 10,000 data points, and a bound on decoded structs. Hostile
//!   input gets a 400.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use chrono::{DateTime, Utc};
use openlens_engine::routes::{BOUNDS_MS, BUCKETS};
use openlens_event::{CaptureBatch, RawEvent, limits, names, normalize_batch};
use prost::Message;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::cache::Caches;
use crate::capture::{self, Peer};
use crate::metrics::Metrics;

mod wire;

/// The metric read.
pub(crate) const METRIC: &str = "http.server.request.duration";
/// Largest body, before and after gzip (D-085).
pub(crate) const MAX_BODY: usize = 4 * 1024 * 1024;
/// Data points per request, of every metric (D-085).
pub(crate) const MAX_POINTS: usize = 10_000;
/// Repeated messages (JSON objects) per request: what bounds the decoded
/// structs. A full request of 10,000 points with ten attributes each is
/// about 210,000.
pub(crate) const MAX_ITEMS: usize = 250_000;
/// Explicit bounds per data point.
const MAX_BOUNDS: usize = 64;
/// Largest count of one data point (cumulative counts grow for months).
const MAX_COUNT: u64 = 1_000_000_000_000_000;
/// Cumulative series kept (D-085); the least recently updated go first.
pub(crate) const SERIES_CAP: usize = 50_000;
/// A cumulative series not updated for this long is forgotten.
pub(crate) const SERIES_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Longest route kept, in characters (as the Node SDK).
const MAX_ROUTE: usize = 300;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/metrics", post(ingest))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
}

/// Now, in Unix nanoseconds.
pub(crate) fn now_ns() -> u64 {
    Utc::now()
        .timestamp_nanos_opt()
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Enc {
    Protobuf,
    Json,
}

fn encoding(headers: &HeaderMap) -> Option<Enc> {
    let ct = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let mime = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "application/x-protobuf" | "application/protobuf" => Some(Enc::Protobuf),
        "application/json" => Some(Enc::Json),
        _ => None,
    }
}

fn reply(enc: Enc, status: StatusCode, proto: Vec<u8>, json: &Value) -> Response {
    let (ct, bytes) = match enc {
        Enc::Protobuf => ("application/x-protobuf", proto),
        Enc::Json => ("application/json", json.to_string().into_bytes()),
    };
    (status, [(header::CONTENT_TYPE, ct)], bytes).into_response()
}

/// An OTLP error: a `Status` with its message, in the request's encoding.
fn fail(enc: Enc, status: StatusCode, message: &str) -> Response {
    let proto = wire::Status {
        message: message.to_owned(),
    }
    .encode_to_vec();
    reply(enc, status, proto, &json!({ "message": message }))
}

/// A refusal of a check shared with `/capture`, sent again as an OTLP
/// `Status` with the same code and `Retry-After`.
fn rewrap(enc: Enc, r: &Response) -> Response {
    let status = r.status();
    let message = match status {
        StatusCode::UNAUTHORIZED => "missing or unknown project key",
        StatusCode::TOO_MANY_REQUESTS => "too many data points; retry in a few seconds",
        StatusCode::SERVICE_UNAVAILABLE => "the server cannot store data right now; retry later",
        StatusCode::PAYLOAD_TOO_LARGE => "body too large",
        StatusCode::BAD_REQUEST => "invalid body",
        _ => "the request failed",
    };
    let mut out = fail(enc, status, message);
    if let Some(v) = r.headers().get(header::RETRY_AFTER) {
        out.headers_mut().insert(header::RETRY_AFTER, v.clone());
    }
    out
}

/// The project key: `x-openlens-key`, else `Authorization: Bearer …`.
fn key_of(headers: &HeaderMap) -> Option<String> {
    if let Some(k) = headers.get("x-openlens-key").and_then(|v| v.to_str().ok()) {
        return Some(k.trim().to_owned());
    }
    let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = auth.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_owned())
}

/// A request refused as a whole: 400 with this message.
#[derive(Debug)]
struct Bad(&'static str);

/// One accepted data point, durations in seconds.
#[derive(Clone)]
struct Sample {
    /// The series key of a cumulative point (`None`: delta).
    series: Option<String>,
    start: u64,
    time: u64,
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    /// Counts per source bucket (empty: no buckets).
    buckets: Vec<u64>,
    /// Upper bounds of the source buckets, seconds.
    bounds: Vec<f64>,
    /// A fingerprint of the source bounds and unit.
    layout: u64,
    method: String,
    route: String,
    status: i64,
    release: Option<String>,
    environment: Option<String>,
}

/// What a request holds once read.
#[derive(Default)]
struct Parsed {
    samples: Vec<Sample>,
    /// Data points accepted (including those with no recorded value).
    accepted: usize,
    rejected: u64,
    reasons: Vec<&'static str>,
}

impl Parsed {
    fn reject(&mut self, n: usize, reason: &'static str) {
        if n == 0 {
            return;
        }
        self.rejected += n as u64;
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }
}

fn refused(r: wire::Refused) -> Bad {
    match r {
        wire::Refused::Malformed => Bad("body is not a valid OTLP metrics request"),
        wire::Refused::TooBig => Bad("body holds too many messages"),
        wire::Refused::TooManyPoints => Bad("more than 10000 data points in one request"),
    }
}

/// Seconds per unit of the metric; `None` for a unit not read.
fn unit_factor(unit: &str) -> Option<f64> {
    match unit.trim() {
        "" | "s" => Some(1.0),
        "ms" => Some(1e-3),
        "us" => Some(1e-6),
        "ns" => Some(1e-9),
        _ => None,
    }
}

fn attr(attrs: &[wire::KeyValue], key: &str) -> Option<String> {
    attrs
        .iter()
        .find(|kv| kv.key == key)
        .and_then(|kv| kv.value.as_ref())
        .and_then(wire::AnyValue::text)
}

/// At most `max` bytes of `s`, cut on a character boundary.
fn clip_bytes(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default().to_owned()
}

/// `GET`, `POST`…: what the routes read (`[A-Z]{1,10}`); `OTHER` else
/// (OpenTelemetry says `_OTHER` for an unknown method).
fn method_of(raw: Option<String>) -> String {
    let m = raw.unwrap_or_default().trim().to_ascii_uppercase();
    if (1..=10).contains(&m.len()) && m.bytes().all(|b| b.is_ascii_uppercase()) {
        m
    } else {
        "OTHER".to_owned()
    }
}

/// Attributes in a fixed order, each part length-prefixed.
fn canon(attrs: &[wire::KeyValue]) -> Vec<u8> {
    let mut kv: Vec<(&str, String)> = attrs
        .iter()
        .map(|a| {
            (
                a.key.as_str(),
                a.value
                    .as_ref()
                    .and_then(wire::AnyValue::text)
                    .unwrap_or_default(),
            )
        })
        .collect();
    kv.sort();
    let mut out = Vec::new();
    for (k, v) in kv {
        for part in [k.as_bytes(), v.as_bytes()] {
            out.extend_from_slice(&(part.len() as u64).to_le_bytes());
            out.extend_from_slice(part);
        }
    }
    out
}

/// A cumulative series: a hash of project, resource, attributes and start
/// time, so a key's size never depends on the input.
fn series_key(project: &str, resource: &[u8], attrs: &[wire::KeyValue], start: u64) -> String {
    let mut h = Sha256::new();
    for part in [project.as_bytes(), resource, &canon(attrs)] {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part);
    }
    h.update(start.to_le_bytes());
    hex::encode(h.finalize())
}

fn layout_of(bounds: &[f64], factor: f64) -> u64 {
    let mut h = DefaultHasher::new();
    factor.to_bits().hash(&mut h);
    for b in bounds {
        b.to_bits().hash(&mut h);
    }
    h.finish()
}

/// Refuse a data point no exporter sends: the whole request gets a 400.
fn check(p: &wire::HistogramDataPoint) -> Result<(), Bad> {
    if p.count > MAX_COUNT {
        return Err(Bad("a data point's count is too large"));
    }
    if p.explicit_bounds.len() > MAX_BOUNDS {
        return Err(Bad("a data point has more than 64 bucket bounds"));
    }
    if p.explicit_bounds.iter().any(|b| !b.is_finite())
        || p.explicit_bounds.windows(2).any(|w| w[0] >= w[1])
    {
        return Err(Bad("bucket bounds must be finite and increasing"));
    }
    if !p.bucket_counts.is_empty() {
        if p.bucket_counts.len() != p.explicit_bounds.len() + 1 {
            return Err(Bad("bucket counts do not match the bucket bounds"));
        }
        let total = p
            .bucket_counts
            .iter()
            .try_fold(0u64, |a, n| a.checked_add(*n));
        if total != Some(p.count) {
            return Err(Bad("bucket counts do not add up to the count"));
        }
    }
    if [p.sum, p.min, p.max]
        .into_iter()
        .flatten()
        .any(|v| !v.is_finite() || v < 0.0)
    {
        return Err(Bad("sum, min and max must be finite and not negative"));
    }
    if p.time_unix_nano == 0 || p.start_time_unix_nano > p.time_unix_nano {
        return Err(Bad("a data point's times are missing or reversed"));
    }
    Ok(())
}

/// The largest duration when the point does not say: the upper bound of
/// its highest non-empty bucket (the last bound for the overflow bucket).
fn estimated_max(buckets: &[u64], bounds: &[f64]) -> f64 {
    let fallback = bounds
        .last()
        .copied()
        .unwrap_or_else(|| BOUNDS_MS.last().copied().unwrap_or(0.0) / 1000.0);
    buckets
        .iter()
        .rposition(|n| *n > 0)
        .and_then(|i| bounds.get(i).copied())
        .unwrap_or(fallback)
}

/// Decode, bound and check a body; keep the points OpenLens reads.
fn read(enc: Enc, raw: &[u8], project: &str) -> Result<Parsed, Bad> {
    let req: wire::ExportRequest = match enc {
        Enc::Protobuf => {
            wire::bound_protobuf(raw).map_err(refused)?;
            wire::ExportRequest::decode(raw).map_err(|_| refused(wire::Refused::Malformed))?
        }
        Enc::Json => {
            wire::bound_json(raw).map_err(refused)?;
            serde_json::from_slice(raw).map_err(|_| refused(wire::Refused::Malformed))?
        }
    };
    let total: usize = req
        .resource_metrics
        .iter()
        .flat_map(|r| &r.scope_metrics)
        .flat_map(|s| &s.metrics)
        .map(|m| m.other_points() + m.histogram.as_ref().map_or(0, |h| h.data_points.len()))
        .sum();
    if total > MAX_POINTS {
        return Err(refused(wire::Refused::TooManyPoints));
    }
    let mut out = Parsed::default();
    for rm in &req.resource_metrics {
        let res: &[wire::KeyValue] = rm
            .resource
            .as_ref()
            .map_or(&[][..], |r| r.attributes.as_slice());
        let release = attr(res, "service.version")
            .filter(|v| !v.trim().is_empty())
            .map(|v| clip_bytes(&v, limits::MAX_DIMENSION));
        // The stable name, else the deprecated one older SDKs still send.
        let environment = attr(res, "deployment.environment.name")
            .or_else(|| attr(res, "deployment.environment"))
            .filter(|v| !v.trim().is_empty())
            .map(|v| clip_bytes(&v, limits::MAX_DIMENSION));
        let res_canon = canon(res);
        for m in rm.scope_metrics.iter().flat_map(|s| &s.metrics) {
            let histogram_points = m.histogram.as_ref().map_or(0, |h| h.data_points.len());
            if m.name != METRIC {
                out.reject(
                    m.other_points() + histogram_points,
                    "only http.server.request.duration is read",
                );
                continue;
            }
            out.reject(
                m.other_points(),
                "http.server.request.duration must be an explicit-bucket histogram",
            );
            let Some(h) = &m.histogram else {
                continue;
            };
            let Some(factor) = unit_factor(&m.unit) else {
                out.reject(
                    histogram_points,
                    "the unit of http.server.request.duration must be s or ms",
                );
                continue;
            };
            let cumulative = match h.aggregation_temporality {
                wire::DELTA => false,
                wire::CUMULATIVE => true,
                _ => {
                    out.reject(
                        histogram_points,
                        "the aggregation temporality must be delta or cumulative",
                    );
                    continue;
                }
            };
            for p in &h.data_points {
                check(p)?;
                if p.flags & wire::NO_RECORDED_VALUE != 0 {
                    out.accepted += 1;
                    continue;
                }
                let Some(route) =
                    attr(&p.attributes, "http.route").filter(|r| !r.trim().is_empty())
                else {
                    out.reject(1, "a data point has no http.route");
                    continue;
                };
                out.accepted += 1;
                let bounds: Vec<f64> = p.explicit_bounds.iter().map(|b| b * factor).collect();
                let max = p
                    .max
                    .map_or_else(|| estimated_max(&p.bucket_counts, &bounds), |m| m * factor);
                out.samples.push(Sample {
                    series: cumulative.then(|| {
                        series_key(project, &res_canon, &p.attributes, p.start_time_unix_nano)
                    }),
                    start: p.start_time_unix_nano,
                    time: p.time_unix_nano,
                    count: p.count,
                    sum: p.sum.map_or(0.0, |s| s * factor),
                    min: p.min.map_or(0.0, |m| m * factor),
                    max,
                    buckets: p.bucket_counts.clone(),
                    layout: layout_of(&p.explicit_bounds, factor),
                    bounds,
                    method: method_of(attr(&p.attributes, "http.request.method")),
                    route: route.chars().take(MAX_ROUTE).collect(),
                    status: attr(&p.attributes, "http.response.status_code")
                        .and_then(|s| s.trim().parse::<i64>().ok())
                        .filter(|s| (100..600).contains(s))
                        .unwrap_or(0),
                    release: release.clone(),
                    environment: environment.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// The last point of a cumulative series (D-085).
#[derive(Clone)]
pub(crate) struct Last {
    time: u64,
    count: u64,
    sum: f64,
    buckets: Vec<u64>,
    layout: u64,
}

impl Last {
    fn of(s: &Sample) -> Self {
        Self {
            time: s.time,
            count: s.count,
            sum: s.sum,
            buckets: s.buckets.clone(),
            layout: s.layout,
        }
    }
}

/// What a cumulative point adds.
enum Step {
    /// Not newer than the last point (a retry, or out of order): nothing.
    Stale,
    /// Only the baseline: the series may have been counted before.
    Prime,
    /// All of it, since this time (a new series or a reset).
    Whole(u64),
    /// The difference with the last point.
    Since(Last),
}

fn step(old: Option<&Last>, s: &Sample, horizon: u64) -> Step {
    match old {
        Some(prev) if s.time <= prev.time => Step::Stale,
        Some(prev) if prev.layout != s.layout || prev.buckets.len() != s.buckets.len() => {
            Step::Prime
        }
        Some(prev) => {
            let went_down = s.count < prev.count
                || s.sum < prev.sum
                || s.buckets.iter().zip(&prev.buckets).any(|(a, b)| a < b);
            if went_down {
                Step::Whole(prev.time)
            } else {
                Step::Since(prev.clone())
            }
        }
        None if s.start > horizon => Step::Whole(s.start),
        None => Step::Prime,
    }
}

/// The difference between a cumulative point and the one before; `None`
/// when nothing happened in between.
fn since(s: &Sample, prev: &Last) -> Option<Sample> {
    let count = s.count.checked_sub(prev.count).filter(|c| *c > 0)?;
    Some(Sample {
        start: prev.time,
        count,
        sum: (s.sum - prev.sum).max(0.0),
        buckets: s
            .buckets
            .iter()
            .zip(&prev.buckets)
            .map(|(a, b)| a.saturating_sub(*b))
            .collect(),
        ..s.clone()
    })
}

/// How to put a series back as it was if the write fails.
struct Undo {
    key: String,
    prev: Option<Last>,
    time: u64,
}

/// The windows to store: delta points as they are, cumulative ones as
/// differences, updating the series in memory.
fn windows(caches: &Caches, samples: Vec<Sample>, now: u64) -> (Vec<Sample>, Vec<Undo>) {
    let ttl = u64::try_from(SERIES_TTL.as_nanos()).unwrap_or(u64::MAX);
    let horizon = caches
        .otlp_forgot
        .load(Ordering::Relaxed)
        .max(now.saturating_sub(ttl));
    let mut out = Vec::new();
    let mut undo = Vec::new();
    for s in samples {
        let Some(key) = s.series.clone() else {
            out.push(s);
            continue;
        };
        let full = caches.otlp_series.len() >= SERIES_CAP;
        let last = Last::of(&s);
        let r = caches.otlp_series.update(key.clone(), |old| {
            let st = step(old.as_ref(), &s, horizon);
            if matches!(st, Step::Stale) {
                return (old, (st, None, false));
            }
            let fresh = old.is_none();
            (Some(last), (st, old, fresh))
        });
        let Some((st, prev, fresh)) = r else {
            continue;
        };
        if fresh && full {
            // Another series was forgotten to make room: from now on a
            // series first seen may be that one.
            caches.otlp_forgot.store(now, Ordering::Relaxed);
        }
        if matches!(st, Step::Stale) {
            continue;
        }
        undo.push(Undo {
            key,
            prev,
            time: s.time,
        });
        match st {
            Step::Stale | Step::Prime => {}
            Step::Whole(from) => out.push(Sample { start: from, ..s }),
            Step::Since(p) => out.extend(since(&s, &p)),
        }
    }
    (out, undo)
}

/// Put the series back as they were before a write that failed, unless a
/// later point moved them since.
fn rollback(caches: &Caches, undo: Vec<Undo>) {
    for u in undo {
        let time = u.time;
        let prev = u.prev;
        caches.otlp_series.update(u.key, move |cur| match cur {
            Some(c) if c.time == time => (prev, ()),
            other => (other, ()),
        });
    }
}

/// Counts in OpenLens's buckets ([`BOUNDS_MS`]): each source bucket goes
/// to the first one whose upper bound is at least its own, so a quantile
/// read from them is never below the true one.
fn rebucket(buckets: &[u64], bounds: &[f64], count: u64) -> Vec<u64> {
    let last = BUCKETS - 1;
    let mut out = vec![0u64; BUCKETS];
    let mut add = |j: usize, n: u64| {
        if let Some(slot) = out.get_mut(j) {
            *slot = slot.saturating_add(n);
        }
    };
    if buckets.is_empty() {
        add(last, count);
    }
    for (i, n) in buckets.iter().enumerate() {
        let j = bounds.get(i).map_or(last, |upper| {
            let ms = upper * 1000.0;
            BOUNDS_MS
                .iter()
                .position(|t| ms <= t * (1.0 + 1e-9))
                .unwrap_or(last)
        });
        add(j, *n);
    }
    out
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// The `$request_stats` event of a window, as the Node SDK sends it.
fn raw_event(w: &Sample) -> RawEvent {
    let from = if w.start > 0 { w.start } else { w.time };
    let timestamp = i64::try_from(from / 1_000_000_000)
        .ok()
        .and_then(|secs| {
            DateTime::<Utc>::from_timestamp(secs, u32::try_from(from % 1_000_000_000).unwrap_or(0))
        })
        .map(|t| t.to_rfc3339());
    let window_s = if w.start > 0 {
        w.time.saturating_sub(w.start) / 1_000_000_000
    } else {
        0
    };
    let mut p = Map::new();
    p.insert("$lib".into(), json!("otlp"));
    if let Some(e) = &w.environment {
        p.insert("$environment".into(), json!(e));
    }
    if let Some(r) = &w.release {
        p.insert("$release".into(), json!(r));
    }
    p.insert("$metric".into(), json!(METRIC));
    p.insert("$unit".into(), json!("s"));
    p.insert("http.request.method".into(), json!(w.method));
    p.insert("http.route".into(), json!(w.route));
    p.insert("http.response.status_code".into(), json!(w.status));
    p.insert("count".into(), json!(w.count));
    p.insert("sum".into(), json!(round6(w.sum)));
    p.insert("min".into(), json!(round6(w.min)));
    p.insert("max".into(), json!(round6(w.max)));
    p.insert(
        "bucket_counts".into(),
        json!(rebucket(&w.buckets, &w.bounds, w.count)),
    );
    p.insert("$window_s".into(), json!(window_s));
    RawEvent {
        event: names::REQUEST_STATS.to_owned(),
        distinct_id: Some(names::SERVER_ID.to_owned()),
        timestamp,
        properties: Some(p),
        ..RawEvent::default()
    }
}

/// Events through the same checks as `/capture`, in batches of its size.
fn normalized(project: &str, windows: &[Sample]) -> (Vec<openlens_event::Event>, usize) {
    let now = Utc::now();
    let mut batch: Vec<RawEvent> = windows.iter().map(raw_event).collect();
    let mut events = Vec::with_capacity(batch.len());
    let mut invalid = 0;
    while !batch.is_empty() {
        let rest = batch.split_off(batch.len().min(limits::MAX_BATCH_EVENTS));
        let b = CaptureBatch {
            batch,
            ..CaptureBatch::default()
        };
        if let Ok(n) = normalize_batch(project, b, now) {
            invalid += n.rejected.len();
            events.extend(n.events);
        }
        batch = rest;
    }
    (events, invalid)
}

/// `POST /v1/metrics`.
pub(crate) async fn ingest(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(enc) = encoding(&headers) else {
        return fail(
            Enc::Json,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content-type must be application/x-protobuf or application/json",
        );
    };
    let m = state.metrics.clone();
    Metrics::add(&m.capture_requests, 1);
    if capture::ip_limited(&state, &peer, &headers) {
        Metrics::add(&m.rate_limited, 1);
        return rewrap(enc, &capture::too_many());
    }
    if let Some(r) = capture::disk_full(&state) {
        Metrics::add(&m.disk_refused, 1);
        return rewrap(enc, &r);
    }
    let project = match capture::project_for_key(&state, key_of(&headers)).await {
        Ok(p) => p,
        Err(r) => return rewrap(enc, &r),
    };

    // Up to 8 MB to gunzip and decode: off the async threads.
    let (h, pid) = (headers.clone(), project.id.clone());
    let parsed = tokio::task::spawn_blocking(move || -> Result<Parsed, Response> {
        let raw = capture::inflate(&h, &body).map_err(|r| rewrap(enc, &r))?;
        if raw.len() > MAX_BODY {
            return Err(fail(
                enc,
                StatusCode::PAYLOAD_TOO_LARGE,
                "body too large once decompressed",
            ));
        }
        read(enc, &raw, &pid).map_err(|Bad(msg)| fail(enc, StatusCode::BAD_REQUEST, msg))
    })
    .await;
    let parsed = match parsed {
        Ok(Ok(p)) => p,
        Ok(Err(r)) => return r,
        Err(_) => return fail(enc, StatusCode::INTERNAL_SERVER_ERROR, "decode task failed"),
    };
    // A data point costs one event of the key's budget; direct local
    // traffic has none, as on `/capture`.
    if capture::client_ip(&peer, &headers).is_some()
        && capture::key_limited(&state, &project.api_key, parsed.accepted)
    {
        tracing::warn!(project = %project.id, "otlp rate limited");
        Metrics::add(&m.rate_limited, 1);
        return rewrap(enc, &capture::too_many());
    }

    let Parsed {
        samples,
        rejected,
        mut reasons,
        ..
    } = parsed;
    let (st, pid) = (state.clone(), project.id.clone());
    let built = tokio::task::spawn_blocking(move || {
        let (windows, undo) = windows(&st.caches, samples, now_ns());
        let (events, invalid) = normalized(&pid, &windows);
        (events, invalid, undo)
    })
    .await;
    let Ok((mut events, invalid, undo)) = built else {
        return fail(
            enc,
            StatusCode::INTERNAL_SERVER_ERROR,
            "ingestion task failed",
        );
    };
    // Host rules and aliases of the project (D-034), as on `/capture`.
    crate::environments::apply(&state, &project.id, &mut events);
    let stored_n = events.len();
    let st = state.clone();
    let stored = tokio::task::spawn_blocking(move || capture::store(&st, &events)).await;
    match stored {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::error!(%err, "cannot store otlp data points");
            rollback(&state.caches, undo);
            return fail(
                enc,
                StatusCode::SERVICE_UNAVAILABLE,
                "cannot store data points right now",
            );
        }
        Err(_) => {
            rollback(&state.caches, undo);
            return fail(
                enc,
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage task failed",
            );
        }
    }

    let rejected = rejected + invalid as u64;
    if invalid > 0 {
        reasons.push("a data point failed the ingestion checks");
    }
    Metrics::add(&m.events_accepted, stored_n as u64);
    Metrics::add(&m.events_rejected, rejected);
    if rejected == 0 {
        return reply(
            enc,
            StatusCode::OK,
            wire::ExportResponse::default().encode_to_vec(),
            &json!({}),
        );
    }
    let message = format!("{rejected} data points not read: {}", reasons.join("; "));
    let proto = wire::ExportResponse {
        partial_success: Some(wire::PartialSuccess {
            rejected_data_points: i64::try_from(rejected).unwrap_or(i64::MAX),
            error_message: message.clone(),
        }),
    }
    .encode_to_vec();
    reply(
        enc,
        StatusCode::OK,
        proto,
        &json!({ "partialSuccess": {
            "rejectedDataPoints": rejected.to_string(),
            "errorMessage": message,
        }}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(start: u64, time: u64, count: u64, buckets: Vec<u64>) -> Sample {
        Sample {
            series: Some("k".into()),
            start,
            time,
            count,
            sum: count as f64 * 0.01,
            min: 0.0,
            max: 1.0,
            buckets,
            bounds: vec![0.1],
            layout: 7,
            method: "GET".into(),
            route: "/a".into(),
            status: 200,
            release: None,
            environment: None,
        }
    }

    #[test]
    fn a_series_first_seen_is_counted_only_when_it_started_after_the_horizon() {
        let s = sample(100, 200, 5, vec![5, 0]);
        assert!(matches!(step(None, &s, 50), Step::Whole(100)));
        assert!(matches!(step(None, &s, 150), Step::Prime));
    }

    #[test]
    fn a_retry_adds_nothing_and_a_counter_that_went_down_starts_over() {
        let prev = Last::of(&sample(100, 200, 10, vec![6, 4]));
        assert!(matches!(
            step(Some(&prev), &sample(100, 200, 10, vec![6, 4]), 0),
            Step::Stale
        ));
        assert!(matches!(
            step(Some(&prev), &sample(100, 300, 3, vec![3, 0]), 0),
            Step::Whole(200)
        ));
        let next = sample(100, 300, 15, vec![9, 6]);
        let Step::Since(p) = step(Some(&prev), &next, 0) else {
            panic!("a later point adds its difference");
        };
        let d = since(&next, &p).unwrap();
        assert_eq!(d.count, next.count - prev.count);
        assert_eq!(d.buckets.iter().sum::<u64>(), d.count);
    }

    #[test]
    fn rebucketing_never_moves_a_count_below_its_upper_bound() {
        // Source buckets: (.., 30 ms], (30, 300 ms], (300 ms, ..).
        let out = rebucket(&[1, 2, 3], &[0.03, 0.3], 6);
        assert_eq!(out.iter().sum::<u64>(), 6);
        for (i, n) in out.iter().enumerate() {
            if *n > 0 && i < BOUNDS_MS.len() {
                assert!(BOUNDS_MS[i] >= 30.0);
            }
        }
        assert_eq!(out[BUCKETS - 1], 3, "the open bucket stays open");
        // OpenTelemetry's default bounds map one to one.
        let same: Vec<f64> = BOUNDS_MS.iter().map(|b| b / 1000.0).collect();
        let counts: Vec<u64> = (1..=BUCKETS as u64).collect();
        assert_eq!(rebucket(&counts, &same, counts.iter().sum()), counts);
    }
}

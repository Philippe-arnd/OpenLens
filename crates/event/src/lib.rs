//! Event schema, identifiers and input validation for captured events.
//!
//! Everything that arrives on `/capture` is hostile until it has passed
//! through [`normalize_batch`]: sizes, nesting depth and counts are bounded,
//! client clocks are corrected, and the result is a flat [`Event`] with the
//! dimensions OpenLens slices by (role, account, plan, feature, page)
//! promoted to their own fields.
//!
//! Layer: L0 (see `docs/architecture.md`).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub mod facts;
pub mod vital;
pub use facts::{Facts, facts};
pub use vital::{Vital, vital};

/// Limits applied to every incoming batch. They are deliberately generous for
/// real SDK traffic and small enough that one request cannot exhaust memory.
pub mod limits {
    /// Maximum events in one batch.
    pub const MAX_BATCH_EVENTS: usize = 1_000;
    /// Maximum length of an event name, in bytes.
    pub const MAX_EVENT_NAME: usize = 200;
    /// Maximum length of a distinct id, session id or uuid, in bytes.
    pub const MAX_ID: usize = 200;
    /// Maximum length of a promoted string dimension (url, role, ...), in bytes.
    pub const MAX_DIMENSION: usize = 2_048;
    /// Maximum serialized size of one event's properties, in bytes.
    pub const MAX_PROPERTIES_BYTES: usize = 64 * 1024;
    /// Maximum JSON nesting depth inside properties.
    pub const MAX_DEPTH: usize = 10;
    /// Client timestamps further than this in the future are not trusted.
    pub const MAX_FUTURE_SKEW_SECS: i64 = 3_600;
}

/// Event names with a meaning to OpenLens. Apps may send any other name.
pub mod names {
    /// A page view (also emitted on client-side route changes).
    pub const PAGEVIEW: &str = "$pageview";
    /// A click on an interactive element.
    pub const CLICK: &str = "$click";
    /// A form submission.
    pub const SUBMIT: &str = "$submit";
    /// The user was identified; `$set` carries role/account/plan.
    pub const IDENTIFY: &str = "$identify";
    /// A feature was used (task start).
    pub const FEATURE: &str = "$feature";
    /// An uncaught error or unhandled rejection.
    pub const ERROR: &str = "$error";
    /// A Web Vitals measurement (`$name`, `$value`, `$rating`).
    pub const VITAL: &str = "$vital";
    /// A failed network request (`$path`, `$status`).
    pub const REQUEST_FAILED: &str = "$request_failed";
    /// Server request stats of one route in one 10-minute window, sent by
    /// the server SDKs (#593): `$method`, `$route`, `$status_class`,
    /// `$count`, `$errors`, `$sum_ms`, `$max_ms`, `$buckets`. Not user
    /// activity: the `events` view leaves them out.
    pub const REQUEST_STATS: &str = "$request_stats";
    /// Distinct id of server events tied to no user (#593): left out of
    /// release shares.
    pub const SERVER_ID: &str = "$server";
}

/// One event as sent by an SDK.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawEvent {
    /// Event name.
    pub event: String,
    /// Stable id of the user (or anonymous visitor).
    #[serde(default)]
    pub distinct_id: Option<String>,
    /// Client timestamp, RFC 3339.
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Session id assigned by the SDK.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Client-generated id, used for de-duplication.
    #[serde(default)]
    pub uuid: Option<String>,
    /// Free-form properties.
    #[serde(default)]
    pub properties: Option<Map<String, Value>>,
}

/// A batch as posted to `/capture`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CaptureBatch {
    /// Project API key (may also come from a header or the query string).
    #[serde(default)]
    pub key: Option<String>,
    /// Client time when the batch was sent, RFC 3339; used to correct skew.
    #[serde(default)]
    pub sent_at: Option<String>,
    /// The events.
    #[serde(default)]
    pub batch: Vec<RawEvent>,
}

/// A validated, normalized event, ready for storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Unique id (client-supplied when valid, otherwise a v7 UUID).
    pub uuid: String,
    /// Project the event belongs to.
    pub project: String,
    /// Event name.
    pub event: String,
    /// User or visitor id.
    pub distinct_id: String,
    /// Session id, when the SDK sent one.
    pub session_id: Option<String>,
    /// When the event happened (skew-corrected).
    pub timestamp: DateTime<Utc>,
    /// When the server received it.
    pub received_at: DateTime<Utc>,
    /// Full page URL (`$current_url`).
    pub url: Option<String>,
    /// Page path (`$pathname`).
    pub path: Option<String>,
    /// Feature key (`$feature`).
    pub feature: Option<String>,
    /// Raw role value as the app sent it (`$role`); mapped to a canonical
    /// role at query time using the feature map.
    pub role: Option<String>,
    /// Account / organisation id (`$account`).
    pub account: Option<String>,
    /// Plan (`$plan`).
    pub plan: Option<String>,
    /// Deployment environment (`$environment`): `production` when unset.
    #[serde(default = "default_environment")]
    pub environment: String,
    /// Release (`$release`): version or commit of the app that sent it.
    #[serde(default)]
    pub release: Option<String>,
    /// Feature flags on for the user, comma-separated and sorted (`$flags`).
    #[serde(default)]
    pub flags: Option<String>,
    /// Remaining properties, serialized JSON object.
    pub properties: String,
}

/// Environment assumed when the SDK sends none.
pub const DEFAULT_ENVIRONMENT: &str = "production";

fn default_environment() -> String {
    DEFAULT_ENVIRONMENT.to_owned()
}

/// Normalize an environment name: lowercase, common aliases folded
/// (`prod` → `production`, `dev`/`local` → `development`, `stage` →
/// `staging`), anything else kept as sent.
pub fn normalize_environment(raw: &str) -> String {
    let e = raw.trim().to_ascii_lowercase();
    match e.as_str() {
        "" | "prod" | "live" => DEFAULT_ENVIRONMENT.to_owned(),
        "dev" | "local" | "localhost" => "development".to_owned(),
        "stage" | "stg" => "staging".to_owned(),
        "preview" | "review" => "preview".to_owned(),
        _ => e,
    }
}

/// Environment of an event whose host no rule and no hostname pattern
/// classifies (D-034): kept apart, counted in no production number.
pub const UNKNOWN_ENVIRONMENT: &str = "unknown";

/// The host of a page URL, lowercase, without port: `https://App.x:8080/a`
/// gives `app.x`. `None` when the URL has no `scheme://host`.
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = if authority.starts_with('[') {
        // IPv6 literal: keep the brackets, drop the port.
        authority.split_once(']').map(|(h, _)| format!("{h}]"))?
    } else {
        authority.split(':').next().unwrap_or_default().to_owned()
    };
    let host = host.trim().to_ascii_lowercase();
    (!host.is_empty() && host.len() <= 253).then_some(host)
}

/// The environment a hostname pattern gives `host`, as the browser SDK
/// detects it (`sdk/js/src/env.ts`, D-012), or `None` when no pattern
/// matches (the SDK then says `production`).
#[must_use]
pub fn detected_environment(host: &str) -> Option<&'static str> {
    let h = host.to_ascii_lowercase();
    if h == "localhost"
        || h == "0.0.0.0"
        || h == "[::1]"
        || h.starts_with("127.")
        || h.ends_with(".local")
        || h.ends_with(".localhost")
        || h.ends_with(".test")
    {
        return Some("development");
    }
    // Labels split on `.` and `-`, as the SDK's `(^|[.-])word([.-]|$)`.
    let labels: Vec<&str> = h.split(['.', '-']).collect();
    if labels
        .iter()
        .any(|l| matches!(*l, "staging" | "stage" | "stg"))
    {
        return Some("staging");
    }
    if labels.iter().any(|l| matches!(*l, "preview" | "review"))
        || pull_request_label(&h)
        || h.ends_with(".vercel.app")
        || h.ends_with(".netlify.app")
    {
        return Some("preview");
    }
    None
}

/// `pr-<digits>` between separators (`.`, `-`) or the ends, as the SDK's
/// `(^|[.-])pr-\d+([.-]|$)`.
fn pull_request_label(h: &str) -> bool {
    let b = h.as_bytes();
    h.match_indices("pr-").any(|(i, _)| {
        let starts = i == 0 || matches!(b[i - 1], b'.' | b'-');
        let digits = b[i + 3..].iter().take_while(|c| c.is_ascii_digit()).count();
        let end = i + 3 + digits;
        starts && digits > 0 && (end == b.len() || matches!(b[end], b'.' | b'-'))
    })
}

/// What the browser SDK sends for `host` when the app sets no environment:
/// the pattern's environment, or `production` (D-012).
#[must_use]
pub fn sdk_guess(host: &str) -> &'static str {
    detected_environment(host).unwrap_or(DEFAULT_ENVIRONMENT)
}

/// Why an event or batch was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EventError {
    /// The batch holds more events than [`limits::MAX_BATCH_EVENTS`].
    #[error("batch has {0} events, the limit is {max}", max = limits::MAX_BATCH_EVENTS)]
    BatchTooLarge(usize),
    /// The event name is empty.
    #[error("event name is empty")]
    EmptyName,
    /// A field exceeds its size limit.
    #[error("{0} is too long")]
    TooLong(&'static str),
    /// No distinct id was given.
    #[error("distinct_id is missing")]
    MissingDistinctId,
    /// Properties nest deeper than [`limits::MAX_DEPTH`].
    #[error("properties nest too deeply")]
    TooDeep,
}

/// Outcome of normalizing a batch: accepted events plus per-event rejections
/// (index in the batch and reason). One bad event never drops the batch.
#[derive(Debug, Default)]
pub struct Normalized {
    /// Events that passed validation.
    pub events: Vec<Event>,
    /// Rejected events, by index in the incoming batch.
    pub rejected: Vec<(usize, EventError)>,
}

/// Validate and normalize a batch for `project`, received at `received_at`.
///
/// Returns `Err` only when the batch as a whole is unacceptable.
pub fn normalize_batch(
    project: &str,
    batch: CaptureBatch,
    received_at: DateTime<Utc>,
) -> Result<Normalized, EventError> {
    if batch.batch.len() > limits::MAX_BATCH_EVENTS {
        return Err(EventError::BatchTooLarge(batch.batch.len()));
    }
    let skew = batch
        .sent_at
        .as_deref()
        .and_then(parse_ts)
        .map(|sent| received_at - sent);

    let mut out = Normalized::default();
    for (i, raw) in batch.batch.into_iter().enumerate() {
        match normalize_event(project, raw, received_at, skew) {
            Ok(ev) => out.events.push(ev),
            Err(e) => out.rejected.push((i, e)),
        }
    }
    Ok(out)
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn normalize_event(
    project: &str,
    raw: RawEvent,
    received_at: DateTime<Utc>,
    skew: Option<Duration>,
) -> Result<Event, EventError> {
    let name = raw.event.trim();
    if name.is_empty() {
        return Err(EventError::EmptyName);
    }
    bounded(name, limits::MAX_EVENT_NAME, "event")?;

    let distinct_id = raw
        .distinct_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(EventError::MissingDistinctId)?;
    bounded(distinct_id, limits::MAX_ID, "distinct_id")?;

    let session_id = clean_opt(raw.session_id.as_deref(), limits::MAX_ID, "session_id")?;

    let uuid = match clean_opt(raw.uuid.as_deref(), limits::MAX_ID, "uuid")? {
        Some(u) if uuid::Uuid::parse_str(&u).is_ok() => u,
        _ => uuid::Uuid::now_v7().to_string(),
    };

    let timestamp = correct_timestamp(raw.timestamp.as_deref(), received_at, skew);

    let mut props = raw.properties.unwrap_or_default();
    for v in props.values() {
        if depth(v, 1) > limits::MAX_DEPTH {
            return Err(EventError::TooDeep);
        }
    }
    let url = take_dim(&mut props, "$current_url")?.map(|u| clean_url(&u));
    let path = take_dim(&mut props, "$pathname")?;
    let feature = take_dim(&mut props, "$feature")?;
    let role = take_dim(&mut props, "$role")?;
    let account = take_dim(&mut props, "$account")?;
    let plan = take_dim(&mut props, "$plan")?;
    let environment = take_dim(&mut props, "$environment")?
        .map_or_else(default_environment, |e| normalize_environment(&e));
    let release = take_dim(&mut props, "$release")?;
    let flags = take_dim(&mut props, "$flags")?;

    let properties = Value::Object(props).to_string();
    if properties.len() > limits::MAX_PROPERTIES_BYTES {
        return Err(EventError::TooLong("properties"));
    }

    Ok(Event {
        uuid,
        project: project.to_owned(),
        event: name.to_owned(),
        distinct_id: distinct_id.to_owned(),
        session_id,
        timestamp,
        received_at,
        url,
        path,
        feature,
        role,
        account,
        plan,
        environment,
        release,
        flags,
        properties,
    })
}

/// Use the client timestamp shifted by the measured clock skew; fall back to
/// the receive time when it is missing, unparsable or in the future.
fn correct_timestamp(
    ts: Option<&str>,
    received_at: DateTime<Utc>,
    skew: Option<Duration>,
) -> DateTime<Utc> {
    let Some(client) = ts.and_then(parse_ts) else {
        return received_at;
    };
    let corrected = match skew {
        Some(s) => client.checked_add_signed(s).unwrap_or(received_at),
        None => client,
    };
    if corrected > received_at + Duration::seconds(limits::MAX_FUTURE_SKEW_SECS) {
        received_at
    } else {
        corrected
    }
}

fn bounded(s: &str, max: usize, field: &'static str) -> Result<(), EventError> {
    if s.len() > max {
        Err(EventError::TooLong(field))
    } else {
        Ok(())
    }
}

fn clean_opt(
    s: Option<&str>,
    max: usize,
    field: &'static str,
) -> Result<Option<String>, EventError> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => bounded(v, max, field).map(|()| Some(v.to_owned())),
        None => Ok(None),
    }
}

/// Query parameters kept in a stored page URL: campaign tags only.
const KEPT_PARAMS: [&str; 2] = ["ref", "gclid"];

/// A page URL as stored: no fragment, and no query parameter except
/// campaign tags (`utm_*`, `ref`, `gclid`). Query strings carry reset tokens,
/// magic links and e-mail addresses, which are never stored (docs/privacy.md).
pub fn clean_url(url: &str) -> String {
    let url = url.split('#').next().unwrap_or_default();
    let Some((base, query)) = url.split_once('?') else {
        return url.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            let key = kv
                .split('=')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            key.starts_with("utm_") || KEPT_PARAMS.contains(&key.as_str())
        })
        .collect();
    if kept.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// Remove a promoted dimension from the properties. Non-string scalars are
/// stringified; objects and arrays are not valid dimensions and stay put.
fn take_dim(
    props: &mut Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, EventError> {
    let value = match props.get(key) {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => return Ok(None),
    };
    props.remove(key);
    if value.is_empty() {
        return Ok(None);
    }
    bounded(&value, limits::MAX_DIMENSION, key)?;
    Ok(Some(value))
}

/// Nesting depth, iterative so hostile input cannot overflow the stack.
fn depth(v: &Value, start: usize) -> usize {
    let mut max = start;
    let mut stack = vec![(v, start)];
    while let Some((v, d)) = stack.pop() {
        max = max.max(d);
        if d > limits::MAX_DEPTH {
            return d;
        }
        match v {
            Value::Array(a) => stack.extend(a.iter().map(|x| (x, d + 1))),
            Value::Object(o) => stack.extend(o.values().map(|x| (x, d + 1))),
            _ => {}
        }
    }
    max
}

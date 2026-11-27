//! Exemplars of OTLP data points (#607, D-089): the slowest and the first
//! 5xx of each point, with the trace id and the session, never the user.

use openlens_engine::routes::{MAX_EXEMPLAR_S, valid_session, valid_trace_id};
use serde_json::{Map, Value, json};

use super::{attr, rfc3339, round6, wire};

/// One exemplar kept (#607, D-089): nothing else of the request, and never
/// the user (`$request_stats` belongs to `$server`, so erasing a person
/// would not reach a user id kept here; the session is erased with them).
#[derive(Clone, PartialEq)]
pub(super) struct Ex {
    /// Seconds.
    pub(super) secs: f64,
    /// Unix nanoseconds.
    pub(super) time: u64,
    pub(super) trace_id: Option<String>,
    pub(super) session: Option<String>,
}

/// The exemplars of a point kept (#607): its slowest and, when the point
/// was answered 5xx, its first; two at most. `factor` turns the metric's
/// unit into seconds.
pub(super) fn exemplars(p: &wire::HistogramDataPoint, factor: f64, status: i64) -> Vec<Ex> {
    let mut slowest: Option<Ex> = None;
    let mut first: Option<Ex> = None;
    for e in &p.exemplars {
        let Some(v) = e.as_double.or(e.as_int.map(|i| i as f64)) else {
            continue;
        };
        let secs = v * factor;
        if !secs.is_finite() || !(0.0..=MAX_EXEMPLAR_S).contains(&secs) {
            continue;
        }
        let trace_id = (e.trace_id.len() == 16)
            .then(|| hex::encode(&e.trace_id))
            .filter(|t| valid_trace_id(t));
        let ex = Ex {
            secs,
            time: if e.time_unix_nano > 0 {
                e.time_unix_nano
            } else {
                p.time_unix_nano
            },
            trace_id,
            session: attr(&e.filtered_attributes, "session.id").filter(|s| valid_session(s)),
        };
        if slowest.as_ref().is_none_or(|s| ex.secs > s.secs) {
            slowest = Some(ex.clone());
        }
        if status >= 500 && first.as_ref().is_none_or(|f| ex.time < f.time) {
            first = Some(ex);
        }
    }
    let mut out: Vec<Ex> = slowest.into_iter().collect();
    if let Some(f) = first
        && out.first() != Some(&f)
    {
        out.push(f);
    }
    out
}

/// The `exemplars` field (#607), in the Node SDK's shape.
pub(super) fn exemplars_json(list: &[Ex]) -> Vec<Value> {
    list.iter()
        .filter_map(|e| {
            let mut x = Map::new();
            x.insert("value".into(), json!(round6(e.secs)));
            x.insert("time".into(), json!(rfc3339(e.time)?));
            if let Some(t) = &e.trace_id {
                x.insert("trace_id".into(), json!(t));
            }
            let mut a = Map::new();
            if let Some(s) = &e.session {
                a.insert("session.id".into(), json!(s));
            }
            if !a.is_empty() {
                x.insert("filtered_attributes".into(), Value::Object(a));
            }
            Some(Value::Object(x))
        })
        .collect()
}

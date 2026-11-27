//! The OTLP metrics messages `/v1/metrics` reads (#605, D-085), written by
//! hand from the public opentelemetry-proto files (v1.5.0, Apache-2.0):
//! only the fields OpenLens uses. One set of structs reads both encodings,
//! protobuf through `prost` and JSON through `serde`, with the OTLP JSON
//! rules: lowerCamelCase names, 64-bit integers as strings or numbers,
//! enums as integers, unknown fields ignored.
//!
//! A `oneof` is a set of optional fields on the wire, so each case is its
//! own optional field here. Fields left out (the scope, span ids, nested
//! array and map values) are skipped by both decoders without being
//! built, so they cost no memory and no recursion. Exemplars (#607) are
//! read: their value, time, trace id and filtered attributes.
//!
//! Before decoding, [`bound_protobuf`] and [`bound_json`] count what the
//! decoders would allocate, so a 4 MB body of empty messages cannot become
//! hundreds of megabytes of structs.

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

/// `ExportMetricsServiceRequest`.
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ExportRequest {
    #[prost(message, repeated, tag = "1")]
    pub(crate) resource_metrics: Vec<ResourceMetrics>,
}

/// `ResourceMetrics`.
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ResourceMetrics {
    #[prost(message, optional, tag = "1")]
    pub(crate) resource: Option<Resource>,
    #[prost(message, repeated, tag = "2")]
    pub(crate) scope_metrics: Vec<ScopeMetrics>,
}

/// `Resource`.
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Resource {
    #[prost(message, repeated, tag = "1")]
    pub(crate) attributes: Vec<KeyValue>,
}

/// `ScopeMetrics` (the scope itself is not read).
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ScopeMetrics {
    #[prost(message, repeated, tag = "2")]
    pub(crate) metrics: Vec<Metric>,
}

/// `Metric`: one of the data cases is set.
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Metric {
    #[prost(string, tag = "1")]
    pub(crate) name: String,
    #[prost(string, tag = "3")]
    pub(crate) unit: String,
    #[prost(message, optional, tag = "5")]
    pub(crate) gauge: Option<Points>,
    #[prost(message, optional, tag = "7")]
    pub(crate) sum: Option<Points>,
    #[prost(message, optional, tag = "9")]
    pub(crate) histogram: Option<Histogram>,
    #[prost(message, optional, tag = "10")]
    pub(crate) exponential_histogram: Option<Points>,
    #[prost(message, optional, tag = "11")]
    pub(crate) summary: Option<Points>,
}

impl Metric {
    /// Data points of the cases OpenLens does not read.
    pub(crate) fn other_points(&self) -> usize {
        let cases: [&Option<Points>; 4] = [
            &self.gauge,
            &self.sum,
            &self.exponential_histogram,
            &self.summary,
        ];
        cases
            .into_iter()
            .map(|p| p.as_ref().map_or(0, |p| p.data_points.len()))
            .sum()
    }
}

/// The data points of a case that is only counted (gauge, sum,
/// exponential histogram, summary).
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Points {
    #[prost(message, repeated, tag = "1")]
    pub(crate) data_points: Vec<Opaque>,
}

/// A message whose content is skipped: it takes no memory.
#[derive(prost::Message, Deserialize)]
pub(crate) struct Opaque {}

/// `Histogram` (explicit buckets).
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Histogram {
    #[prost(message, repeated, tag = "1")]
    pub(crate) data_points: Vec<HistogramDataPoint>,
    /// 1 delta, 2 cumulative (0 unspecified).
    #[prost(int32, tag = "2")]
    pub(crate) aggregation_temporality: i32,
}

/// `AGGREGATION_TEMPORALITY_DELTA`.
pub(crate) const DELTA: i32 = 1;
/// `AGGREGATION_TEMPORALITY_CUMULATIVE`.
pub(crate) const CUMULATIVE: i32 = 2;
/// `DATA_POINT_FLAGS_NO_RECORDED_VALUE_MASK`.
pub(crate) const NO_RECORDED_VALUE: u32 = 1;

/// `HistogramDataPoint`.
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct HistogramDataPoint {
    #[prost(message, repeated, tag = "9")]
    pub(crate) attributes: Vec<KeyValue>,
    #[prost(message, repeated, tag = "8")]
    pub(crate) exemplars: Vec<Exemplar>,
    #[prost(fixed64, tag = "2")]
    #[serde(deserialize_with = "u64_any")]
    pub(crate) start_time_unix_nano: u64,
    #[prost(fixed64, tag = "3")]
    #[serde(deserialize_with = "u64_any")]
    pub(crate) time_unix_nano: u64,
    #[prost(fixed64, tag = "4")]
    #[serde(deserialize_with = "u64_any")]
    pub(crate) count: u64,
    #[prost(double, optional, tag = "5")]
    #[serde(deserialize_with = "opt_f64")]
    pub(crate) sum: Option<f64>,
    #[prost(fixed64, repeated, tag = "6")]
    #[serde(deserialize_with = "vec_u64")]
    pub(crate) bucket_counts: Vec<u64>,
    #[prost(double, repeated, tag = "7")]
    #[serde(deserialize_with = "vec_f64")]
    pub(crate) explicit_bounds: Vec<f64>,
    #[prost(uint32, tag = "10")]
    pub(crate) flags: u32,
    #[prost(double, optional, tag = "11")]
    #[serde(deserialize_with = "opt_f64")]
    pub(crate) min: Option<f64>,
    #[prost(double, optional, tag = "12")]
    #[serde(deserialize_with = "opt_f64")]
    pub(crate) max: Option<f64>,
}

/// `Exemplar` (#607): one real measurement of a data point. The span id
/// is not read.
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Exemplar {
    #[prost(message, repeated, tag = "7")]
    pub(crate) filtered_attributes: Vec<KeyValue>,
    #[prost(fixed64, tag = "2")]
    #[serde(deserialize_with = "u64_any")]
    pub(crate) time_unix_nano: u64,
    #[prost(double, optional, tag = "3")]
    #[serde(deserialize_with = "opt_f64")]
    pub(crate) as_double: Option<f64>,
    #[prost(sfixed64, optional, tag = "6")]
    #[serde(deserialize_with = "opt_i64")]
    pub(crate) as_int: Option<i64>,
    /// 16 bytes; hex in JSON (OTLP's exception to base64). Anything else
    /// reads as empty: no trace id, never a refusal.
    #[prost(bytes = "vec", tag = "5")]
    #[serde(deserialize_with = "hex_id")]
    pub(crate) trace_id: Vec<u8>,
}

/// `KeyValue`.
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct KeyValue {
    #[prost(string, tag = "1")]
    pub(crate) key: String,
    #[prost(message, optional, tag = "2")]
    pub(crate) value: Option<AnyValue>,
}

/// `AnyValue`, its scalar cases (arrays, maps and bytes are skipped).
#[derive(prost::Message, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct AnyValue {
    #[prost(string, optional, tag = "1")]
    pub(crate) string_value: Option<String>,
    #[prost(bool, optional, tag = "2")]
    pub(crate) bool_value: Option<bool>,
    #[prost(int64, optional, tag = "3")]
    #[serde(deserialize_with = "opt_i64")]
    pub(crate) int_value: Option<i64>,
    #[prost(double, optional, tag = "4")]
    #[serde(deserialize_with = "opt_f64")]
    pub(crate) double_value: Option<f64>,
}

impl AnyValue {
    /// The value as text; `None` for an empty or non-scalar value.
    pub(crate) fn text(&self) -> Option<String> {
        if let Some(s) = &self.string_value {
            return Some(s.clone());
        }
        if let Some(i) = self.int_value {
            return Some(i.to_string());
        }
        if let Some(d) = self.double_value.filter(|d| d.is_finite()) {
            return Some(d.to_string());
        }
        self.bool_value.map(|b| b.to_string())
    }
}

/// `ExportMetricsServiceResponse`.
#[derive(prost::Message)]
pub(crate) struct ExportResponse {
    #[prost(message, optional, tag = "1")]
    pub(crate) partial_success: Option<PartialSuccess>,
}

/// `ExportMetricsPartialSuccess`.
#[derive(prost::Message)]
pub(crate) struct PartialSuccess {
    #[prost(int64, tag = "1")]
    pub(crate) rejected_data_points: i64,
    #[prost(string, tag = "2")]
    pub(crate) error_message: String,
}

/// `google.rpc.Status`, the body of every OTLP error (its `code` is not
/// used by OTLP and left out).
#[derive(prost::Message)]
pub(crate) struct Status {
    #[prost(string, tag = "2")]
    pub(crate) message: String,
}

// --- JSON numbers -------------------------------------------------------

struct U64V;

impl Visitor<'_> for U64V {
    type Value = u64;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an unsigned 64-bit integer, as a number or a decimal string")
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
        Ok(v)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
        u64::try_from(v).map_err(|_| E::custom("negative integer"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
        v.parse().map_err(|_| E::custom("not an unsigned integer"))
    }
    fn visit_unit<E: de::Error>(self) -> Result<u64, E> {
        Ok(0)
    }
}

struct I64V;

impl Visitor<'_> for I64V {
    type Value = i64;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a 64-bit integer, as a number or a decimal string")
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
        Ok(v)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
        i64::try_from(v).map_err(|_| E::custom("integer too large"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
        v.parse().map_err(|_| E::custom("not an integer"))
    }
}

struct F64V;

impl Visitor<'_> for F64V {
    type Value = f64;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a number, or NaN, Infinity or -Infinity as a string")
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<f64, E> {
        Ok(v)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<f64, E> {
        Ok(v as f64)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<f64, E> {
        Ok(v as f64)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<f64, E> {
        match v {
            "NaN" => Ok(f64::NAN),
            "Infinity" => Ok(f64::INFINITY),
            "-Infinity" => Ok(f64::NEG_INFINITY),
            _ => v.parse().map_err(|_| E::custom("not a number")),
        }
    }
}

struct U64(u64);
impl<'de> Deserialize<'de> for U64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(U64V).map(U64)
    }
}

struct I64(i64);
impl<'de> Deserialize<'de> for I64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(I64V).map(I64)
    }
}

struct F64(f64);
impl<'de> Deserialize<'de> for F64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(F64V).map(F64)
    }
}

fn u64_any<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    d.deserialize_any(U64V)
}

fn opt_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(Option::<I64>::deserialize(d)?.map(|I64(v)| v))
}

fn opt_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(Option::<F64>::deserialize(d)?.map(|F64(v)| v))
}

/// A trace id in OTLP JSON: 32 hex digits, else empty.
fn hex_id<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = Option::<String>::deserialize(d)?.unwrap_or_default();
    if s.len() != 32 {
        return Ok(Vec::new());
    }
    Ok(hex::decode(s).unwrap_or_default())
}

fn vec_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    Ok(Vec::<U64>::deserialize(d)?
        .into_iter()
        .map(|U64(v)| v)
        .collect())
}

fn vec_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    Ok(Vec::<F64>::deserialize(d)?
        .into_iter()
        .map(|F64(v)| v)
        .collect())
}

// --- Bounds before decoding ---------------------------------------------

/// Why a body is refused before it is decoded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Not valid protobuf (or JSON) for the messages above.
    Malformed,
    /// More repeated messages than [`super::MAX_ITEMS`].
    TooBig,
    /// More data points than [`super::MAX_POINTS`].
    TooManyPoints,
}

/// The messages a length-delimited field is decoded into.
#[derive(Clone, Copy)]
enum Level {
    Request,
    ResourceMetrics,
    Resource,
    ScopeMetrics,
    Metric,
    Histogram,
    Points,
    Point,
    Exemplar,
    KeyValue,
}

/// What a length-delimited `field` of a `level` message becomes.
enum Child {
    /// An element of a repeated message field, decoded at that level.
    Each(Level),
    /// A singular message, decoded at that level.
    One(Level),
    /// A data point of a histogram (decoded) or of a case only counted.
    HistogramPoint,
    OtherPoint,
    /// A string, bytes, packed numbers or a skipped field: no structs.
    Flat,
}

fn child(level: Level, field: u64) -> Child {
    match (level, field) {
        (Level::Request, 1) => Child::Each(Level::ResourceMetrics),
        (Level::ResourceMetrics, 1) => Child::One(Level::Resource),
        (Level::ResourceMetrics, 2) => Child::Each(Level::ScopeMetrics),
        (Level::Resource, 1) | (Level::Point, 9) | (Level::Exemplar, 7) => {
            Child::Each(Level::KeyValue)
        }
        (Level::Point, 8) => Child::Each(Level::Exemplar),
        (Level::ScopeMetrics, 2) => Child::Each(Level::Metric),
        (Level::Metric, 9) => Child::One(Level::Histogram),
        (Level::Metric, 5 | 7 | 10 | 11) => Child::One(Level::Points),
        (Level::Histogram, 1) => Child::HistogramPoint,
        (Level::Points, 1) => Child::OtherPoint,
        _ => Child::Flat,
    }
}

/// Repeated messages and data points seen so far.
#[derive(Default)]
struct Budget {
    items: usize,
    points: usize,
}

impl Budget {
    fn item(&mut self) -> Result<(), Refused> {
        self.items += 1;
        if self.items > super::MAX_ITEMS {
            return Err(Refused::TooBig);
        }
        Ok(())
    }
    fn point(&mut self) -> Result<(), Refused> {
        self.points += 1;
        if self.points > super::MAX_POINTS {
            return Err(Refused::TooManyPoints);
        }
        Ok(())
    }
}

fn varint(buf: &[u8], pos: &mut usize) -> Result<u64, Refused> {
    let mut v: u64 = 0;
    for i in 0..10 {
        let b = *buf.get(*pos).ok_or(Refused::Malformed)?;
        *pos += 1;
        v |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(Refused::Malformed)
}

fn skip(buf: &[u8], pos: &mut usize, n: usize) -> Result<(), Refused> {
    *pos = pos
        .checked_add(n)
        .filter(|p| *p <= buf.len())
        .ok_or(Refused::Malformed)?;
    Ok(())
}

/// Walk one message of `level`. The depth is the schema's (seven levels
/// at most), never the input's.
fn walk(buf: &[u8], level: Level, budget: &mut Budget) -> Result<(), Refused> {
    let mut pos = 0;
    while pos < buf.len() {
        let key = varint(buf, &mut pos)?;
        match key & 7 {
            0 => {
                varint(buf, &mut pos)?;
            }
            1 => skip(buf, &mut pos, 8)?,
            5 => skip(buf, &mut pos, 4)?,
            2 => {
                let len =
                    usize::try_from(varint(buf, &mut pos)?).map_err(|_| Refused::Malformed)?;
                let start = pos;
                skip(buf, &mut pos, len)?;
                let inner = buf.get(start..pos).ok_or(Refused::Malformed)?;
                match child(level, key >> 3) {
                    Child::Each(l) => {
                        budget.item()?;
                        walk(inner, l, budget)?;
                    }
                    Child::One(l) => walk(inner, l, budget)?,
                    Child::HistogramPoint => {
                        budget.item()?;
                        budget.point()?;
                        walk(inner, Level::Point, budget)?;
                    }
                    Child::OtherPoint => budget.point()?,
                    Child::Flat => {}
                }
            }
            // Groups (3, 4) are not used by OTLP; 6 and 7 do not exist.
            _ => return Err(Refused::Malformed),
        }
    }
    Ok(())
}

/// Refuse a protobuf body that is malformed or would decode into too many
/// structs or data points.
pub(crate) fn bound_protobuf(buf: &[u8]) -> Result<(), Refused> {
    walk(buf, Level::Request, &mut Budget::default())
}

/// Refuse a JSON body with more objects than [`super::MAX_ITEMS`]: every
/// decoded struct is an object (braces inside strings count too, which
/// only makes the bound stricter).
pub(crate) fn bound_json(buf: &[u8]) -> Result<(), Refused> {
    if buf.iter().filter(|b| **b == b'{').count() > super::MAX_ITEMS {
        return Err(Refused::TooBig);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::*;

    #[test]
    fn protobuf_and_json_read_the_same_point() {
        let point = HistogramDataPoint {
            attributes: vec![KeyValue {
                key: "http.route".into(),
                value: Some(AnyValue {
                    string_value: Some("/a".into()),
                    ..AnyValue::default()
                }),
            }],
            start_time_unix_nano: 1,
            time_unix_nano: 2,
            count: 3,
            sum: Some(0.5),
            bucket_counts: vec![1, 2],
            explicit_bounds: vec![0.1],
            ..HistogramDataPoint::default()
        };
        let bytes = point.encode_to_vec();
        assert!(walk(&bytes, Level::Point, &mut Budget::default()).is_ok());
        let back = HistogramDataPoint::decode(&bytes[..]).unwrap();
        let json: HistogramDataPoint = serde_json::from_str(
            r#"{"attributes":[{"key":"http.route","value":{"stringValue":"/a"}}],
                "startTimeUnixNano":"1","timeUnixNano":2,"count":"3","sum":0.5,
                "bucketCounts":["1",2],"explicitBounds":[0.1],"exemplars":[{"spanId":"ab"}]}"#,
        )
        .unwrap();
        for p in [&back, &json] {
            assert_eq!(p.count, point.count);
            assert_eq!(p.bucket_counts, point.bucket_counts);
            assert_eq!(p.explicit_bounds, point.explicit_bounds);
            assert_eq!(p.time_unix_nano, point.time_unix_nano);
            assert_eq!(
                p.attributes[0].value.as_ref().and_then(AnyValue::text),
                Some("/a".into())
            );
        }
    }

    #[test]
    fn a_body_of_empty_messages_is_refused_before_it_is_decoded() {
        // Each `0a 00` is one empty ResourceMetrics: two bytes for a struct.
        let flood: Vec<u8> = [0x0a, 0x00].repeat(super::super::MAX_ITEMS + 1);
        assert_eq!(bound_protobuf(&flood), Err(Refused::TooBig));
        let json = format!(
            "{{\"resourceMetrics\":[{}]}}",
            vec!["{}"; super::super::MAX_ITEMS].join(",")
        );
        assert_eq!(bound_json(json.as_bytes()), Err(Refused::TooBig));
    }
}

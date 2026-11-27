//! OTLP/HTTP metrics bodies for `/v1/metrics` (#605): a small protobuf
//! encoder for the messages the server reads, written from the public
//! opentelemetry-proto field numbers, and the hostile-input corpus
//! (exemplars too, #607).

/// An attribute value.
#[derive(Debug, Clone)]
pub enum Value {
    /// `stringValue`.
    Str(String),
    /// `intValue`.
    Int(i64),
}

/// One `HistogramDataPoint`.
#[derive(Debug, Clone, Default)]
pub struct Point {
    /// Attributes.
    pub attrs: Vec<(String, Value)>,
    /// `startTimeUnixNano`.
    pub start: u64,
    /// `timeUnixNano`.
    pub time: u64,
    /// `count`.
    pub count: u64,
    /// `sum`.
    pub sum: Option<f64>,
    /// `min`.
    pub min: Option<f64>,
    /// `max`.
    pub max: Option<f64>,
    /// `bucketCounts`.
    pub buckets: Vec<u64>,
    /// `explicitBounds`.
    pub bounds: Vec<f64>,
    /// Add one exemplar (a trace id and a value).
    pub exemplar: bool,
    /// More exemplars (#607).
    pub exemplars: Vec<Exemplar>,
}

/// One `Exemplar` (#607).
#[derive(Debug, Clone, Default)]
pub struct Exemplar {
    /// `timeUnixNano`.
    pub time: u64,
    /// `asDouble`, in the metric's unit.
    pub value: f64,
    /// `traceId` (16 bytes when valid).
    pub trace_id: Vec<u8>,
    /// `filteredAttributes`.
    pub attrs: Vec<(String, Value)>,
}

fn exemplar(e: &Exemplar) -> Vec<u8> {
    let mut ex = Vec::new();
    for (k, v) in &e.attrs {
        bytes(&mut ex, 7, &key_value(k, v));
    }
    fixed64(&mut ex, 2, e.time);
    double(&mut ex, 3, e.value);
    if !e.trace_id.is_empty() {
        bytes(&mut ex, 5, &e.trace_id);
    }
    ex
}

impl Point {
    /// A point of `http.server.request.duration` for `method route status`.
    pub fn route(method: &str, route: &str, status: i64) -> Self {
        Self {
            attrs: vec![
                ("http.request.method".into(), Value::Str(method.into())),
                ("http.route".into(), Value::Str(route.into())),
                ("http.response.status_code".into(), Value::Int(status)),
            ],
            ..Self::default()
        }
    }
}

/// The data of a metric.
#[derive(Debug, Clone)]
pub enum Data {
    /// An explicit-bucket histogram: temporality (1 delta, 2 cumulative)
    /// and points.
    Histogram(i32, Vec<Point>),
    /// A gauge with this many points.
    Gauge(usize),
}

/// One `Metric`.
#[derive(Debug, Clone)]
pub struct Metric {
    /// `name`.
    pub name: String,
    /// `unit`.
    pub unit: String,
    /// The data.
    pub data: Data,
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn key(out: &mut Vec<u8>, field: u64, wire: u64) {
    varint(out, (field << 3) | wire);
}

fn bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
    key(out, field, 2);
    varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

fn fixed64(out: &mut Vec<u8>, field: u64, v: u64) {
    key(out, field, 1);
    out.extend_from_slice(&v.to_le_bytes());
}

fn double(out: &mut Vec<u8>, field: u64, v: f64) {
    fixed64(out, field, v.to_bits());
}

fn key_value(k: &str, v: &Value) -> Vec<u8> {
    let mut any = Vec::new();
    match v {
        Value::Str(s) => bytes(&mut any, 1, s.as_bytes()),
        Value::Int(i) => {
            key(&mut any, 3, 0);
            varint(&mut any, *i as u64);
        }
    }
    let mut kv = Vec::new();
    bytes(&mut kv, 1, k.as_bytes());
    bytes(&mut kv, 2, &any);
    kv
}

fn point(p: &Point) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in &p.attrs {
        bytes(&mut out, 9, &key_value(k, v));
    }
    fixed64(&mut out, 2, p.start);
    fixed64(&mut out, 3, p.time);
    fixed64(&mut out, 4, p.count);
    if let Some(s) = p.sum {
        double(&mut out, 5, s);
    }
    if !p.buckets.is_empty() {
        let packed: Vec<u8> = p.buckets.iter().flat_map(|b| b.to_le_bytes()).collect();
        bytes(&mut out, 6, &packed);
    }
    if !p.bounds.is_empty() {
        let packed: Vec<u8> = p
            .bounds
            .iter()
            .flat_map(|b| b.to_bits().to_le_bytes())
            .collect();
        bytes(&mut out, 7, &packed);
    }
    if p.exemplar {
        let mut ex = Vec::new();
        fixed64(&mut ex, 2, p.time);
        double(&mut ex, 3, 0.01);
        bytes(&mut ex, 5, &[0xab; 16]);
        bytes(&mut out, 8, &ex);
    }
    for e in &p.exemplars {
        bytes(&mut out, 8, &exemplar(e));
    }
    if let Some(m) = p.min {
        double(&mut out, 11, m);
    }
    if let Some(m) = p.max {
        double(&mut out, 12, m);
    }
    out
}

/// A metric whose histogram points each get `extra` appended (raw fields).
fn metric_with(m: &Metric, extra: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    bytes(&mut out, 1, m.name.as_bytes());
    bytes(&mut out, 3, m.unit.as_bytes());
    match &m.data {
        Data::Histogram(temporality, points) => {
            let mut h = Vec::new();
            for p in points {
                let mut raw = point(p);
                raw.extend_from_slice(extra);
                bytes(&mut h, 1, &raw);
            }
            key(&mut h, 2, 0);
            varint(&mut h, *temporality as u64);
            bytes(&mut out, 9, &h);
        }
        Data::Gauge(n) => {
            let mut g = Vec::new();
            let mut p = Vec::new();
            fixed64(&mut p, 3, 1);
            double(&mut p, 4, 1.0);
            for _ in 0..*n {
                bytes(&mut g, 1, &p);
            }
            bytes(&mut out, 5, &g);
        }
    }
    out
}

/// An `ExportMetricsServiceRequest` with one resource (its string
/// attributes) and one scope holding `metrics`, protobuf-encoded.
pub fn request(resource: &[(&str, &str)], metrics: &[Metric]) -> Vec<u8> {
    request_with(resource, metrics, &[])
}

fn request_with(resource: &[(&str, &str)], metrics: &[Metric], extra: &[u8]) -> Vec<u8> {
    let mut res = Vec::new();
    for (k, v) in resource {
        bytes(&mut res, 1, &key_value(k, &Value::Str((*v).into())));
    }
    let mut scope = Vec::new();
    let mut name = Vec::new();
    bytes(&mut name, 1, b"openlens-test");
    bytes(&mut scope, 1, &name);
    for m in metrics {
        bytes(&mut scope, 2, &metric_with(m, extra));
    }
    let mut rm = Vec::new();
    bytes(&mut rm, 1, &res);
    bytes(&mut rm, 2, &scope);
    let mut out = Vec::new();
    bytes(&mut out, 1, &rm);
    out
}

fn duration(points: Vec<Point>) -> Vec<u8> {
    request(
        &[("service.name", "api")],
        &[Metric {
            name: "http.server.request.duration".into(),
            unit: "s".into(),
            data: Data::Histogram(1, points),
        }],
    )
}

fn good() -> Point {
    Point {
        start: 1_700_000_000_000_000_000,
        time: 1_700_000_060_000_000_000,
        count: 3,
        sum: Some(0.3),
        buckets: vec![1, 2],
        bounds: vec![0.1],
        ..Point::route("GET", "/a", 200)
    }
}

const JSON_POINT: &str = r#""attributes":[{"key":"http.route","value":{"stringValue":"/a"}}],"startTimeUnixNano":"1700000000000000000","timeUnixNano":"1700000060000000000""#;

fn json_body(point: &str) -> Vec<u8> {
    format!(
        r#"{{"resourceMetrics":[{{"scopeMetrics":[{{"metrics":[{{"name":"http.server.request.duration","unit":"s","histogram":{{"aggregationTemporality":1,"dataPoints":[{{{JSON_POINT},{point}}}]}}}}]}}]}}]}}"#
    )
    .into_bytes()
}

/// Hostile `/v1/metrics` bodies, each with its content type: every one
/// must get a 400, never a crash. Every crash fix adds its input here.
pub fn hostile_otlp_bodies() -> Vec<(&'static str, Vec<u8>)> {
    const PB: &str = "application/x-protobuf";
    const JS: &str = "application/json";
    let huge = Point {
        count: u64::MAX,
        buckets: vec![u64::MAX, 0],
        ..good()
    };
    let mismatch = Point {
        buckets: vec![1, 1, 1],
        ..good()
    };
    let not_adding_up = Point { count: 4, ..good() };
    let nan = Point {
        sum: Some(f64::NAN),
        ..good()
    };
    let negative = Point {
        sum: Some(-1.0),
        ..good()
    };
    let infinite_bound = Point {
        bounds: vec![f64::INFINITY],
        ..good()
    };
    let reversed = Point {
        start: 2,
        time: 1,
        ..good()
    };
    let too_many = request(
        &[],
        &[Metric {
            name: "process.cpu.time".into(),
            unit: "s".into(),
            data: Data::Gauge(10_001),
        }],
    );
    // A length that runs past the end of the body.
    let mut truncated = duration(vec![good()]);
    truncated.truncate(truncated.len() / 2);
    // A start-group field (wire type 3), which OTLP never uses.
    let group = vec![0x0b, 0x0c];
    // Two bytes per empty ResourceMetrics: a flood of structs.
    let flood = [0x0a, 0x00].repeat(300_000);
    let json_flood = format!(
        r#"{{"resourceMetrics":[{}]}}"#,
        vec!["{}"; 300_000].join(",")
    );
    let json_deep = format!(
        r#"{{"resourceMetrics":{}1{}}}"#,
        "[".repeat(5_000),
        "]".repeat(5_000)
    );
    // 300,000 empty exemplars (`42 00`) on one point: a flood of structs
    // since exemplars are decoded (#607).
    let exemplar_flood = request_with(
        &[],
        &[Metric {
            name: "http.server.request.duration".into(),
            unit: "s".into(),
            data: Data::Histogram(1, vec![good()]),
        }],
        &[0x42, 0x00].repeat(300_000),
    );
    let json_too_many = format!(
        r#"{{"resourceMetrics":[{{"scopeMetrics":[{{"metrics":[{{"name":"x","gauge":{{"dataPoints":[{}]}}}}]}}]}}]}}"#,
        vec!["{}"; 10_001].join(",")
    );
    vec![
        (PB, vec![0xff; 16]),
        (PB, truncated),
        (PB, group),
        (PB, flood),
        (PB, too_many),
        (PB, duration(vec![huge])),
        (PB, duration(vec![mismatch])),
        (PB, duration(vec![not_adding_up])),
        (PB, duration(vec![nan])),
        (PB, duration(vec![negative])),
        (PB, duration(vec![infinite_bound])),
        (PB, duration(vec![reversed])),
        (PB, exemplar_flood),
        (JS, b"not json".to_vec()),
        (JS, b"null".to_vec()),
        (JS, json_deep.into_bytes()),
        (JS, json_flood.into_bytes()),
        (JS, json_too_many.into_bytes()),
        (
            JS,
            json_body(r#""count":"-5","bucketCounts":[],"explicitBounds":[]"#),
        ),
        (
            JS,
            json_body(r#""count":"18446744073709551616","bucketCounts":[],"explicitBounds":[]"#),
        ),
        (JS, json_body(r#""count":1e30"#)),
        (
            JS,
            json_body(r#""count":"3","sum":"NaN","bucketCounts":["3"]"#),
        ),
        (
            JS,
            json_body(r#""count":"3","sum":-2,"bucketCounts":["3"]"#),
        ),
        (
            JS,
            json_body(r#""count":"3","bucketCounts":["1","2"],"explicitBounds":[]"#),
        ),
        (
            JS,
            json_body(r#""count":"3","bucketCounts":["1","1","1"],"explicitBounds":[0.5,0.1]"#),
        ),
    ]
}

/// Hostile exemplars (#607): each body is a valid export whose exemplars
/// are not. Every one must be accepted (exemplars never refuse a point),
/// with what is stored bounded: no huge value, no attribute but the
/// session (the `user.id` sent must be dropped), no invalid trace id.
pub fn hostile_exemplar_bodies() -> Vec<(&'static str, Vec<u8>)> {
    const PB: &str = "application/x-protobuf";
    const JS: &str = "application/json";
    let huge = "x".repeat(1_000_000);
    let mut attrs: Vec<(String, Value)> = (0..1_000)
        .map(|i| {
            (
                format!("http.request.header.{i}"),
                Value::Str("secret".into()),
            )
        })
        .collect();
    attrs.push(("session.id".into(), Value::Str(huge.clone())));
    attrs.push(("user.id".into(), Value::Str(huge.clone())));
    let point = |exemplars: Vec<Exemplar>| Point {
        exemplars,
        ..Point::route("GET", "/hostile", 503)
    };
    let at = good().time;
    let bad = vec![
        Exemplar {
            time: at,
            value: 0.5,
            trace_id: vec![0xab; 5],
            attrs,
        },
        Exemplar {
            time: at,
            value: f64::NAN,
            trace_id: vec![0; 16],
            ..Exemplar::default()
        },
        Exemplar {
            time: 0,
            value: 1e300,
            ..Exemplar::default()
        },
        Exemplar {
            time: u64::MAX,
            value: -1.0,
            ..Exemplar::default()
        },
    ];
    let pb = duration(vec![Point {
        start: good().start,
        time: good().time,
        count: 3,
        sum: Some(0.3),
        buckets: vec![1, 2],
        bounds: vec![0.1],
        ..point(bad)
    }]);
    let json = |exemplars: &str| {
        json_body(&format!(
            r#""count":"3","bucketCounts":["1","2"],"explicitBounds":[0.1],"exemplars":[{exemplars}]"#
        ))
    };
    vec![
        (PB, pb),
        (
            JS,
            json(
                r#"{"timeUnixNano":"1700000030000000000","asDouble":0.2,"traceId":"zz8efff798038103d269b633813fc6zz"}"#,
            ),
        ),
        (
            JS,
            json(&format!(
                r#"{{"timeUnixNano":"1700000030000000000","asDouble":0.2,"traceId":"{huge}"}}"#
            )),
        ),
        (
            JS,
            json(
                r#"{"timeUnixNano":"1700000030000000000","asInt":"9223372036854775807","traceId":"00000000000000000000000000000000"}"#,
            ),
        ),
        (
            JS,
            json(&format!(
                r#"{{"asDouble":"Infinity","filteredAttributes":[{{"key":"user.id","value":{{"stringValue":"{huge}"}}}}]}}"#
            )),
        ),
    ]
}

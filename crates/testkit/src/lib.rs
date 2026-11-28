//! Test helpers: synthetic event generators and hostile-input corpora.
//!
//! Only dev-dependencies may use this crate (`cargo xtask layers`).

pub mod otlp;

/// Hostile JSON bodies for `/capture`. Every crash fix adds its input here.
pub fn hostile_capture_bodies() -> Vec<Vec<u8>> {
    let deep = format!(
        "{{\"batch\":[{{\"event\":\"x\",\"distinct_id\":\"u\",\"properties\":{{\"a\":{}1{}}}}}]}}",
        "[".repeat(5_000),
        "]".repeat(5_000)
    );
    let wide = format!(
        "{{\"batch\":[{}]}}",
        vec!["{\"event\":\"x\",\"distinct_id\":\"u\"}"; 1_001].join(",")
    );
    vec![
        Vec::new(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{\"batch\":null}".to_vec(),
        b"{\"batch\":[null]}".to_vec(),
        b"{\"batch\":[{\"event\":123}]}".to_vec(),
        b"{\"batch\":[{\"event\":\"\",\"distinct_id\":\"\"}]}".to_vec(),
        "{\"batch\":[{\"event\":\"\u{0000}\u{FFFF}\",\"distinct_id\":\"\u{1F600}\"}]}"
            .as_bytes()
            .to_vec(),
        format!("{{\"batch\":[{{\"event\":\"{}\",\"distinct_id\":\"u\"}}]}}", "e".repeat(100_000))
            .into_bytes(),
        b"{\"batch\":[{\"event\":\"x\",\"distinct_id\":\"u\",\"timestamp\":\"+275760-09-13T00:00:00Z\"}],\"sent_at\":\"-262143-01-01T00:00:00Z\"}".to_vec(),
        deep.into_bytes(),
        wide.into_bytes(),
        // Typed facts (#482): escapes, wrong types, lone surrogates, a
        // pattern hidden in a string or a nested object.
        br#"{"batch":[{"event":"$error","distinct_id":"u","properties":{"$message":"\"\\\u0000\u2028","$source":{"a":1},"$stack":["x"]}}]}"#.to_vec(),
        br#"{"batch":[{"event":"$error","distinct_id":"u","properties":{"$message":"\ud800"}}]}"#.to_vec(),
        format!(
            r#"{{"batch":[{{"event":"$pageview","distinct_id":"u","properties":{{"$channel":"{}","$utm_source":"\"\"\"","$ref_host":null,"$utm_campaign":"{}"}}}}]}}"#,
            "\\\"".repeat(5_000),
            "é".repeat(300)
        )
        .into_bytes(),
        br#"{"batch":[{"event":"$click","distinct_id":"u","properties":{"$x":"\"$rage\":true","a":{"$rage":true}}}]}"#.to_vec(),
        vec![0xff, 0xfe, 0x00, 0x7b],
    ]
}

/// Hostile plain texts a person may post (announcement titles and bodies,
/// #562): markup and scripts that must be shown as text, quotes that
/// would close an attribute, control characters, and lengths at and past
/// the limits. Every page that shows posted text keeps these inert.
pub fn hostile_texts() -> Vec<String> {
    vec![
        "<script>alert('x')</script>".into(),
        "<img src=x onerror=alert(1)>".into(),
        "\"><svg onload=alert(1)>".into(),
        "<a href=\"javascript:alert(1)\">link</a>".into(),
        "</textarea><script>alert(1)</script>".into(),
        "&lt;b&gt; already escaped &amp;".into(),
        "line\u{0000}null".into(),
        "bell\u{0007} and escape\u{001b}[31m".into(),
        "\u{202e}reversed".into(),
        "é".repeat(4_000),
        "x".repeat(4_001),
        String::new(),
    ]
}

/// Hostile slugs for the public status page (#561): what a scan or a
/// careless admin sends as `/status/<slug>` or as the slug to save. Path
/// tricks, encoded separators, a route suffix, a project id shape,
/// Unicode look-alikes, control characters, and lengths around the
/// limits. None is a valid slug as typed; a page lookup never reaches the
/// store with one.
pub fn hostile_slugs() -> Vec<String> {
    vec![
        String::new(),
        "ab".into(),
        "-lead".into(),
        "trail-".into(),
        "../etc/passwd".into(),
        "..%2F..%2Fadmin".into(),
        "a%00b".into(),
        "slug.json".into(),
        "slug/badge.svg".into(),
        "slug?window=24h".into(),
        "slug#x".into(),
        "UPPER-case".into(),
        "with space".into(),
        "tab\there".into(),
        "nul\u{0000}byte".into(),
        "ſtatus".into(),
        "stаtus".into(),
        "<script>".into(),
        "' OR 1=1 --".into(),
        "a".repeat(41),
        "b".repeat(10_000),
    ]
}

/// Hostile job slugs for job check-ins (#609): what a broken script or a
/// scan sends as `/check-in/<key>/<slug>`. Path tricks, encoded
/// separators, a colon (which would split the incident rule), uppercase,
/// Unicode look-alikes, control characters, markup, and lengths past the
/// 64 allowed. None is a valid slug; none reaches the store.
pub fn hostile_job_slugs() -> Vec<String> {
    vec![
        "../etc/passwd".into(),
        "..".into(),
        "a/b".into(),
        "a%2Fb".into(),
        "a%00b".into(),
        "nul\u{0000}byte".into(),
        "job:missed".into(),
        "nightly.export".into(),
        "Nightly-Export".into(),
        "with space".into(),
        "tab\there".into(),
        "ſlug".into(),
        "jоb".into(),
        "é".into(),
        "<script>".into(),
        "' OR 1=1 --".into(),
        "a".repeat(65),
        "b".repeat(10_000),
    ]
}

/// Hostile query strings for a job check-in (#609): a status that is
/// neither `ok` nor `error`, a repeated field, negative, fractional, huge
/// and non-numeric durations and gaps, and gaps out of bounds. Each one
/// is refused and records nothing.
pub fn hostile_check_in_queries() -> Vec<String> {
    vec![
        "status=maybe".into(),
        "status=OK".into(),
        "status=ok&status=error".into(),
        "status=%00".into(),
        "ms=-1".into(),
        "ms=abc".into(),
        "ms=1.5".into(),
        "ms=99999999999999999999".into(),
        "ms=604800001".into(),
        "expectEvery=5".into(),
        "expectEvery=-60".into(),
        "expectEvery=1e9".into(),
        "expectEvery=2678401".into(),
        format!("status={}", "x".repeat(10_000)),
    ]
}

use chrono::{DateTime, Duration, Utc};
use openlens_event::Event;

mod plans;
mod realistic;
pub use plans::{CONVERSION_DRIVER, UNMATCHED_PLAN, plans_dataset, plans_feature_map};
pub use realistic::{EVENTS_PER_USER, realistic_feature_map, realistic_user};

/// Deterministic xorshift PRNG (no dependency, reproducible datasets).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 10_000) as f64 / 10_000.0 < p
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo).max(1) as u64) as i64
    }
}

/// What [`planted_dataset`] hides in its data, for tests to find again.
pub mod planted {
    /// Feature whose early use drives retention.
    pub const DRIVER: &str = "export";
    /// Where churned users are last seen.
    pub const CHURN_POINT: &str = "billing";
    /// Role that never uses [`BLIND_FEATURE`].
    pub const BLIND_ROLE: &str = "admin";
    /// Feature members use and admins never do.
    pub const BLIND_FEATURE: &str = "reports";
    /// Release with an error spike.
    pub const BAD_RELEASE: &str = "v2";
    /// Churn window used by the dataset.
    pub const CHURN_DAYS: i64 = 14;
}

#[allow(clippy::too_many_arguments)]
fn event(
    project: &str,
    user: &str,
    role: &str,
    session: &str,
    name: &str,
    at: DateTime<Utc>,
    feature: Option<&str>,
    path: &str,
    release: &str,
    props: &str,
    n: &mut u64,
) -> Event {
    *n += 1;
    Event {
        uuid: format!("00000000-0000-7000-8000-{:012x}", *n),
        project: project.into(),
        event: name.into(),
        distinct_id: user.into(),
        session_id: Some(session.into()),
        timestamp: at,
        received_at: at,
        url: None,
        path: Some(path.into()),
        feature: feature.map(Into::into),
        role: Some(role.into()),
        account: None,
        plan: None,
        environment: "production".into(),
        release: Some(release.into()),
        flags: None,
        properties: props.into(),
    }
}

/// Events `start..start + len` of a large synthetic app for scale
/// benchmarks (#321): `total` events spread evenly over the 90 days before
/// `now`, from `users` people in 4 roles, on 20 features, with a release
/// every 15 days, sessions of about 15 events, a page view in four and
/// one `$identify` per person per day. Deterministic: event `i` is the
/// same whatever the chunking, so a run can be written in pieces.
pub fn scale_chunk(
    project: &str,
    total: u64,
    users: u64,
    start: u64,
    len: u64,
    now: DateTime<Utc>,
) -> Vec<Event> {
    const ROLES: [&str; 4] = ["member", "admin", "viewer", "billing"];
    const DAY_MS: i64 = 86_400_000;
    let span = 90 * DAY_MS;
    let first = now - Duration::days(90);
    let mix = |i: u64| {
        // splitmix64: a well-spread hash of the index.
        let mut z = i.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    for i in start..(start + len).min(total) {
        let h = mix(i);
        // Even spread over the window, in order.
        let at_ms = i128::from(span) * i128::from(i) / i128::from(total.max(1));
        let at = first + Duration::milliseconds(i64::try_from(at_ms).unwrap_or(0));
        let day = i64::try_from(at_ms).unwrap_or(0) / DAY_MS;
        let user = h % users.max(1);
        let role = ROLES[(user % 4) as usize];
        let session = format!("u{user}-d{day}-s{}", (h >> 20) % 3);
        let release = format!("v{}", 1 + day / 15);
        let feature = format!("f{}", (h >> 8) % 20);
        let (name, feature, path) = match (h >> 4) % 16 {
            0 => ("$identify", None, "/".to_owned()),
            1..=4 => ("$pageview", None, format!("/{feature}")),
            5 => ("$error", None, format!("/{feature}")),
            6 => ("$vital", None, format!("/{feature}")),
            7..=10 => ("$click", Some(feature.as_str()), format!("/{feature}")),
            11 => ("$submit", Some(feature.as_str()), format!("/{feature}")),
            _ => ("$feature", Some(feature.as_str()), format!("/{feature}")),
        };
        let props = match name {
            "$vital" => r#"{"$name":"LCP","$value":2380.5}"#,
            "$error" => r#"{"$message":"boom","$source":"https://app.example.com/a.js:1:2"}"#,
            _ => "{}",
        };
        out.push(Event {
            uuid: format!(
                "{:08x}-0000-7000-8000-{:012x}",
                (h >> 32) as u32,
                i & 0xffff_ffff_ffff
            ),
            project: project.into(),
            event: name.into(),
            distinct_id: format!("u{user}"),
            session_id: Some(session),
            timestamp: at,
            received_at: at,
            url: None,
            path: Some(path),
            feature: feature.map(Into::into),
            role: Some(role.into()),
            account: None,
            plan: None,
            environment: "production".into(),
            release: Some(release),
            flags: None,
            properties: props.into(),
        });
    }
    out
}

/// A synthetic app with planted patterns (see [`planted`]): 300 users, two
/// roles, two releases, ~90 days of history ending now.
pub fn planted_dataset(project: &str) -> Vec<Event> {
    use planted::*;
    let now = Utc::now();
    let mut rng = Rng(0x5eed_1234_abcd_9876);
    let mut out = Vec::new();
    let mut n = 0u64;
    for u in 0..300 {
        let user = format!("u{u}");
        let role = if u % 5 == 0 { BLIND_ROLE } else { "member" };
        let first = now - Duration::days(rng.range(30, 85)) - Duration::hours(rng.range(0, 20));
        let early_driver = rng.chance(0.5);
        let retained = rng.chance(if early_driver { 0.85 } else { 0.20 });
        let last = if retained {
            now - Duration::hours(rng.range(2, 72))
        } else {
            (first + Duration::days(rng.range(9, 20))).min(now - Duration::days(CHURN_DAYS + 3))
        };
        let mut day = first;
        let mut s = 0;
        while day <= last {
            s += 1;
            let session = format!("{user}-s{s}");
            let release = if day > now - Duration::days(10) {
                BAD_RELEASE
            } else {
                "v1"
            };
            let mut t = day;
            // Like a real app, the first visit starts anonymous: one page
            // view before login, then `identify` links it to the user.
            let anon = format!("anon-{user}");
            if s == 1 {
                out.push(event(
                    project,
                    &anon,
                    role,
                    &session,
                    "$pageview",
                    t - Duration::seconds(5),
                    None,
                    "/login",
                    release,
                    "{}",
                    &mut n,
                ));
            }
            let identify = if s == 1 {
                format!(r#"{{"$anon_distinct_id":"{anon}"}}"#)
            } else {
                "{}".to_owned()
            };
            let mut ev =
                |name: &str, feature: Option<&str>, path: &str, props: &str, t: DateTime<Utc>| {
                    out.push(event(
                        project, &user, role, &session, name, t, feature, path, release, props,
                        &mut n,
                    ));
                };
            ev("$identify", None, "/", &identify, t);
            ev("$pageview", None, "/home", "{}", t);
            t += Duration::minutes(rng.range(1, 6));
            ev("$click", Some("search"), "/search", "{}", t);
            if early_driver && day <= first + Duration::days(7) {
                for _ in 0..rng.range(1, 4) {
                    t += Duration::minutes(2);
                    ev("$click", Some(DRIVER), "/export", "{}", t);
                }
            }
            if role != BLIND_ROLE && rng.chance(0.6) {
                t += Duration::minutes(2);
                ev("$pageview", Some(BLIND_FEATURE), "/reports", "{}", t);
            }
            if rng.chance(if release == BAD_RELEASE { 0.35 } else { 0.03 }) {
                ev("$error", None, "/home", r#"{"$message":"boom"}"#, t);
            }
            if rng.chance(0.3) {
                ev(
                    "$vital",
                    None,
                    "/home",
                    r#"{"$name":"LCP","$value":4800,"$rating":"poor"}"#,
                    t,
                );
            }
            day += Duration::days(rng.range(1, 4));
        }
        if !retained {
            // The last thing churned users touched.
            let t = last + Duration::minutes(30);
            let (f, p) = if rng.chance(0.7) {
                (CHURN_POINT, "/billing")
            } else {
                ("search", "/search")
            };
            out.push(event(
                project,
                &user,
                role,
                &format!("{user}-last"),
                "$pageview",
                t,
                Some(f),
                p,
                "v1",
                "{}",
                &mut n,
            ));
        }
    }
    out
}

#[cfg(test)]
mod scale_tests {
    #[test]
    fn scale_events_do_not_depend_on_the_chunking() {
        let now = chrono::Utc::now();
        let whole = super::scale_chunk("p", 1_000, 50, 0, 1_000, now);
        let mut parts = super::scale_chunk("p", 1_000, 50, 0, 300, now);
        parts.extend(super::scale_chunk("p", 1_000, 50, 300, 700, now));
        assert_eq!(whole.len(), 1_000);
        assert!(
            whole
                .iter()
                .zip(&parts)
                .all(|(a, b)| a.uuid == b.uuid && a.timestamp == b.timestamp)
        );
        assert!(
            whole.windows(2).all(|w| w[0].timestamp <= w[1].timestamp),
            "in time order"
        );
    }
}

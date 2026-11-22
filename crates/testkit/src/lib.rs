//! Test helpers: synthetic event generators and hostile-input corpora.
//!
//! Only dev-dependencies may use this crate (`cargo xtask layers`).

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
        vec![0xff, 0xfe, 0x00, 0x7b],
    ]
}

use chrono::{DateTime, Duration, Utc};
use openlens_event::Event;

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

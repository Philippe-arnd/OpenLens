//! The realistic scale profile (ARCH-10, #443): what a real product's 90
//! days look like, so a fix to the board can show it moved the failure
//! point. Users are independent and deterministic: user `i` is the same
//! whatever the order they are generated in, so a run can be written in
//! pieces.

use chrono::{DateTime, Duration, Utc};
use openlens_event::Event;

use super::Rng;

/// Features of the profile (`feat1` to `feat24`, route `/app/featN/:id`).
pub const FEATURES: u64 = 24;
/// Events a user sends over 90 days, on average: how many users a run of
/// `events` needs.
pub const EVENTS_PER_USER: u64 = 160;
/// Raw roles as an app sends them, case variants included.
const RAW_ROLES: [&str; 6] = [
    "Admin",
    "owner",
    "member",
    "Member",
    "viewer",
    "billing-admin",
];
const OTHER_PATHS: [&str; 4] = ["/", "/settings", "/billing", "/login"];
const VITALS: [(&str, f64); 5] = [
    ("LCP", 2380.5),
    ("INP", 180.0),
    ("CLS", 0.05),
    ("FCP", 1200.0),
    ("TTFB", 340.0),
];
const PARETO_ALPHA: f64 = 1.6;
const DAYS: i64 = 90;

impl Rng {
    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Poisson-distributed count (Knuth), for small means.
    fn poisson(&mut self, mean: f64) -> u32 {
        let limit = (-mean).exp();
        let mut k = 0;
        let mut p = self.unit();
        while p > limit && k < 1_000 {
            k += 1;
            p *= self.unit();
        }
        k
    }
}

/// The feature map to push before measuring: 24 features and 3 roles that
/// the six raw roles fold into.
pub fn realistic_feature_map() -> String {
    let mut y = String::from(
        "version: 1\nroles:\n  \
         - key: admin\n    match: [admin, owner, billing-admin]\n  \
         - key: member\n    match: [member]\n  \
         - key: viewer\n    match: [viewer]\nfeatures:\n",
    );
    for n in 1..=FEATURES {
        y.push_str(&format!(
            "  - key: feat{n}\n    name: Feature {n}\n    entry:\n      routes: [\"/app/feat{n}/:id\"]\n    success: feat{n}_done\n"
        ));
    }
    y
}

/// All events of user `index` over the 90 days before `now`, in time order.
///
/// Activity propensity follows a Pareto law (α = 1.6) scaled to 20 % of
/// days on average and clamped to 1–95 %. An active day is one session, or
/// two in 30 % of cases, of Poisson(9) events 25 s apart on average. The
/// user's first event is an `$identify` carrying `$anon_distinct_id`.
pub fn realistic_user(project: &str, index: u64, now: DateTime<Utc>) -> Vec<Event> {
    let mut rng = Rng(index.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5eed_0443_cafe_f00d | 1);
    for _ in 0..4 {
        rng.next(); // spread the seed
    }
    let user = format!("u{index}");
    let role = RAW_ROLES[(rng.next() % RAW_ROLES.len() as u64) as usize];
    // Pareto minimum 1, mean α/(α-1): scale so the mean is 20 %.
    let pareto = (1.0 - rng.unit()).powf(-1.0 / PARETO_ALPHA);
    let propensity = (0.2 * pareto / (PARETO_ALPHA / (PARETO_ALPHA - 1.0))).clamp(0.01, 0.95);
    let first = now - Duration::days(DAYS);
    let mut out = Vec::with_capacity(EVENTS_PER_USER as usize);
    let mut seq = 0u64;
    for day in 0..DAYS {
        if !rng.chance(propensity) {
            continue;
        }
        let sessions = if rng.chance(0.3) { 2 } else { 1 };
        for s in 0..sessions {
            let mut at = first + Duration::days(day) + Duration::seconds(rng.range(0, 86_000));
            let feat = 1 + rng.next() % FEATURES;
            let session = format!("{user}-d{day}-s{s}");
            let release = format!("v{}", 1 + day / 15);
            let count = rng.poisson(9.0).max(1);
            for _ in 0..count {
                // Exponential gaps, mean 25 s.
                at += Duration::milliseconds((-25_000.0 * (1.0 - rng.unit()).ln()) as i64);
                if at > now {
                    break;
                }
                let in_feature = rng.chance(0.8);
                let path = if in_feature {
                    format!("/app/feat{feat}/{}", rng.next() % 1_000)
                } else {
                    OTHER_PATHS[(rng.next() % 4) as usize].to_owned()
                };
                let pick = rng.next() % 100;
                let (name, props): (String, String) = match pick {
                    0..=39 => ("$pageview".into(), "{}".into()),
                    40..=61 => ("$click".into(), "{}".into()),
                    62..=69 => ("$feature".into(), "{}".into()),
                    70..=74 => ("$submit".into(), "{}".into()),
                    75..=82 => {
                        let (n, v) = VITALS[(rng.next() % 5) as usize];
                        (
                            "$vital".into(),
                            format!(r#"{{"$name":"{n}","$value":{v}}}"#),
                        )
                    }
                    83..=84 => (
                        "$error".into(),
                        r#"{"$message":"boom","$source":"https://app.example.com/a.js:1:2"}"#
                            .into(),
                    ),
                    85..=94 => (
                        "$scroll".into(),
                        format!(r#"{{"$depth":{}}}"#, 10 * (1 + rng.next() % 10)),
                    ),
                    _ => (format!("feat{feat}_done"), "{}".into()),
                };
                let feature = (in_feature
                    && matches!(name.as_str(), "$click" | "$feature" | "$submit")
                    || name.ends_with("_done"))
                .then(|| format!("feat{feat}"));
                push(
                    &mut out, project, &user, role, &session, &release, at, name, feature, path,
                    props, &mut seq, index,
                );
            }
        }
    }
    out.sort_by_key(|e| e.timestamp);
    // The first event of a user links their anonymous visits.
    if let Some(head) = out.first().cloned() {
        let mut identify = head;
        identify.event = "$identify".into();
        identify.path = Some("/".into());
        identify.feature = None;
        identify.properties = format!(r#"{{"$anon_distinct_id":"anon-{user}"}}"#);
        identify.uuid = uuid(index, u64::MAX >> 16);
        identify.timestamp -= Duration::seconds(1);
        identify.received_at = identify.timestamp;
        out.insert(0, identify);
    }
    out
}

fn uuid(user: u64, seq: u64) -> String {
    format!(
        "{:08x}-0000-7000-8000-{:012x}",
        user & 0xffff_ffff,
        seq & 0xffff_ffff_ffff
    )
}

#[allow(clippy::too_many_arguments)]
fn push(
    out: &mut Vec<Event>,
    project: &str,
    user: &str,
    role: &str,
    session: &str,
    release: &str,
    at: DateTime<Utc>,
    name: String,
    feature: Option<String>,
    path: String,
    props: String,
    seq: &mut u64,
    index: u64,
) {
    *seq += 1;
    out.push(Event {
        uuid: uuid(index, *seq),
        project: project.into(),
        event: name,
        distinct_id: user.into(),
        session_id: Some(session.into()),
        timestamp: at,
        received_at: at,
        url: None,
        path: Some(path),
        feature,
        role: Some(role.into()),
        account: None,
        plan: None,
        environment: "production".into(),
        release: Some(release.into()),
        flags: None,
        properties: props,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn population(n: u64) -> Vec<Event> {
        let now = Utc::now();
        (0..n).flat_map(|i| realistic_user("p", i, now)).collect()
    }

    #[test]
    fn a_user_is_the_same_whenever_it_is_generated() {
        let now = Utc::now();
        let a = realistic_user("p", 7, now);
        let b = realistic_user("p", 7, now);
        assert!(!a.is_empty());
        assert!(
            a.iter()
                .zip(&b)
                .all(|(x, y)| x.uuid == y.uuid && x.timestamp == y.timestamp)
        );
    }

    #[test]
    fn identify_comes_first_and_events_stay_in_the_window() {
        let now = Utc::now();
        for i in 0..50 {
            let evs = realistic_user("p", i, now);
            if let Some(first) = evs.first() {
                assert_eq!(first.event, "$identify");
                assert!(first.properties.contains("$anon_distinct_id"));
            }
            assert!(evs.windows(2).all(|w| w[0].timestamp <= w[1].timestamp));
            assert!(
                evs.iter()
                    .all(|e| e.timestamp <= now && e.timestamp >= now - Duration::days(91))
            );
            assert_eq!(
                evs.iter().filter(|e| e.event == "$identify").count(),
                usize::from(!evs.is_empty())
            );
        }
    }

    #[test]
    fn the_profile_has_the_expected_shape() {
        let evs = population(400);
        let per_user = evs.len() as f64 / 400.0;
        assert!(
            (60.0..400.0).contains(&per_user),
            "events per user: {per_user}"
        );
        let share = |n: &str| evs.iter().filter(|e| e.event == n).count() as f64 / evs.len() as f64;
        assert!(share("$pageview") > share("$click"));
        assert!(share("$click") > share("$vital"));
        assert!(share("$vital") > share("$error"));
        let roles: std::collections::HashSet<_> =
            evs.iter().filter_map(|e| e.role.as_deref()).collect();
        assert_eq!(roles.len(), RAW_ROLES.len());
        let in_feature = evs
            .iter()
            .filter(|e| {
                e.path
                    .as_deref()
                    .is_some_and(|p| p.starts_with("/app/feat"))
            })
            .count() as f64
            / evs.len() as f64;
        assert!(in_feature > 0.6, "most paths are in the session's feature");
        // Heavy tail: the busiest tenth sends more than a quarter of the events.
        let mut counts: Vec<usize> = (0..400)
            .map(|i| realistic_user("p", i, Utc::now()).len())
            .collect();
        counts.sort_unstable_by(|a, b| b.cmp(a));
        let top: usize = counts[..40].iter().sum();
        assert!(top * 4 > evs.len());
    }

    #[test]
    fn the_feature_map_is_valid_and_folds_every_raw_role() {
        let parsed = openlens_featuremap::parse(&realistic_feature_map());
        let map = parsed.map.expect("the map parses");
        assert!(
            parsed.issues.iter().all(|i| i.level != "error"),
            "{:?}",
            parsed.issues
        );
        assert_eq!(map.features.len() as u64, FEATURES);
        for raw in RAW_ROLES {
            assert!(
                map.resolve_role(raw).is_some(),
                "{raw} has no canonical role"
            );
        }
    }
}

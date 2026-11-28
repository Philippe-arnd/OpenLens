//! The persona profile (#706): the app of the target persona, a product
//! with about 30,000 daily active users, as it is stored **after** the
//! event budget (D-097), over the 90 days before `now`.
//!
//! - 120,000 known users, activity drawn from a Pareto law (α = 1.6) so
//!   that a user is active on about a quarter of the days: about 30,000
//!   active users a day. Six raw roles (as the realistic profile), plans
//!   free (80 %) and pro (20 %).
//! - About 25 stored browser events per active user-day: one `$identify`
//!   (at most one per user-day), one or two sessions each opened by a page
//!   view, then clicks (≈ 25 % of the stored events), page views (≈ 12 %),
//!   success and custom events (≈ 15 %), `$submit` and `$request_failed`
//!   (≈ 5 %), feature uses and a few errors for the rest.
//! - Distribution-only events (`$vital`, `$scroll`, `$field`) only on the
//!   user-days drawn by the budget: about 2,000 a day, each event carrying
//!   the share as `$sample` (left out when the share is 1, as the SDK
//!   does). Clicks carry their position only on those days.
//! - Anonymous visitors on a landing page: one known user in three is
//!   first seen anonymous (an `$identify` with `$anon_distinct_id` links
//!   that visit), plus visitors who never sign up, three times as many as
//!   those signups.
//! - Server telemetry of one app server: `$request_stats` for 30 routes per
//!   10-minute window and `$runtime_stats` every 10 minutes, with a
//!   process start at every release (every 15 days).
//!
//! Deterministic: unit `k` (a known user, a visitor, or a server day) is
//! the same whatever the order units are generated in, so a run can be
//! written in pieces. The matching feature map is the realistic profile's
//! ([`super::realistic_feature_map`]): same 24 features, same roles.

use chrono::{DateTime, Duration, NaiveTime, Utc};
use openlens_event::Event;

use super::Rng;
use super::realistic::{FEATURES, OTHER_PATHS, RAW_ROLES, VITALS};

/// Known users of the full persona.
pub const PERSONA_KNOWN_USERS: u64 = 120_000;
/// Events the full persona writes over 90 days, about (the exact count is
/// what a run reports): what `bench-gen --events` scales the population
/// against.
pub const PERSONA_NOMINAL_EVENTS: u64 = 68_000_000;
/// Sessions a day the event budget measures in full (D-097).
pub const PERSONA_SAMPLED_PER_DAY: f64 = 2_000.0;

const DAYS: i64 = 90;
const PARETO_ALPHA: f64 = 1.6;
/// Pareto scale of a user's share of active days: with the clamp at 95 %,
/// the mean share is about [`MEAN_ACTIVITY`].
const PROPENSITY_SCALE: f64 = 0.1137;
/// Mean share of days a known user is active on.
const MEAN_ACTIVITY: f64 = 0.25;
/// Browser events per active user-day besides `$identify` and the
/// sampled distribution events: with those, about 25.
const BASE_MEAN: f64 = 23.0;
const PRO_SHARE: f64 = 0.2;
const LANDING: [&str; 3] = ["/", "/pricing", "/signup"];
const CUSTOM: [&str; 4] = ["invite_sent", "export_csv", "comment_added", "filter_saved"];
const FIELDS: [&str; 4] = ["name", "email", "title", "amount"];
/// Server routes measured per window.
const ROUTES: u64 = 30;
const OTHER_ROUTES: [(&str, &str); 6] = [
    ("POST", "/api/login"),
    ("GET", "/api/me"),
    ("GET", "/api/billing"),
    ("PUT", "/api/settings"),
    ("GET", "/api/search"),
    ("GET", "/health"),
];
const WINDOWS: i64 = 144;
const WINDOW_S: i64 = 600;
/// OpenTelemetry's default bounds of `http.server.request.duration`, in
/// seconds (the server's `routes::BOUNDS_MS`).
const BOUNDS_S: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];
const KNOWN: u16 = 1;
const VISITOR: u16 = 2;
const SERVER: u16 = 3;

/// The persona app, full size or scaled down.
#[derive(Debug, Clone, Copy)]
pub struct Persona {
    known: u64,
}

impl Persona {
    /// The persona at full size: 120,000 known users.
    pub fn full() -> Self {
        Self {
            known: PERSONA_KNOWN_USERS,
        }
    }

    /// The persona with its users scaled so that it writes about `events`
    /// (the server telemetry stays one server's). At least 3 known users.
    pub fn scaled(events: u64) -> Self {
        let known = u128::from(PERSONA_KNOWN_USERS) * u128::from(events)
            / u128::from(PERSONA_NOMINAL_EVENTS);
        Self {
            known: u64::try_from(known).unwrap_or(u64::MAX).max(3),
        }
    }

    /// Known users (`u0`, `u1`, …).
    pub fn known_users(&self) -> u64 {
        self.known
    }

    /// Known users first seen anonymous: one in three.
    pub fn signups(&self) -> u64 {
        self.known.div_ceil(3)
    }

    /// Visitors who never sign up: three times the signups.
    pub fn visitors(&self) -> u64 {
        3 * self.signups()
    }

    /// Daily active users expected.
    pub fn daily_users(&self) -> f64 {
        self.known as f64 * MEAN_ACTIVITY
    }

    /// The share of user-days that carry distribution-only events:
    /// `min(1, 2000 / daily active users)`, as the budget computes it.
    pub fn sample_rate(&self) -> f64 {
        (PERSONA_SAMPLED_PER_DAY / self.daily_users().max(1.0)).min(1.0)
    }

    /// Units to generate: every known user, every visitor, then each of
    /// the 90 days of server telemetry.
    pub fn units(&self) -> u64 {
        self.known + self.visitors() + DAYS as u64
    }

    /// The events of unit `k` (see [`Persona::units`]), in time order.
    pub fn unit(&self, project: &str, k: u64, now: DateTime<Utc>) -> Vec<Event> {
        let mut out = if k < self.known {
            self.known_user(project, k, now)
        } else if k < self.known + self.visitors() {
            visitor(project, k - self.known, now)
        } else {
            self.server_day(project, k - self.known - self.visitors(), now)
        };
        out.sort_by_key(|e| e.timestamp);
        out
    }

    fn known_user(&self, project: &str, index: u64, now: DateTime<Utc>) -> Vec<Event> {
        let mut rng = seeded(index, 0x7e25_0706_a11c_e5ed);
        let user = format!("u{index}");
        let role = RAW_ROLES[(rng.next() % RAW_ROLES.len() as u64) as usize];
        let plan = if rng.chance(PRO_SHARE) { "pro" } else { "free" };
        let pareto = (1.0 - rng.unit()).powf(-1.0 / PARETO_ALPHA);
        let propensity = (PROPENSITY_SCALE * pareto).clamp(0.01, 0.95);
        let rate = self.sample_rate();
        let sample = if rate < 1.0 {
            format!(r#","$sample":{rate:.4}"#)
        } else {
            String::new()
        };
        let first = first_day(now);
        let mut out = Out::new(project, KNOWN, index);
        let mut seen = false;
        for day in 0..DAYS {
            if !rng.chance(propensity) {
                continue;
            }
            let sampled = rng.unit() < rate;
            let sessions: u32 = if rng.chance(0.3) { 2 } else { 1 };
            let mut left = rng.poisson(BASE_MEAN).max(sessions);
            let release = format!("v{}", 1 + day / 15);
            for s in 0..sessions {
                let count = if s + 1 == sessions { left } else { left / 2 };
                left -= count;
                let mut at = first + Duration::days(day) + Duration::seconds(rng.range(0, 80_000));
                let session = format!("{user}-d{day}-s{s}");
                let who = Actor {
                    id: &user,
                    session: Some(&session),
                    role: Some(role),
                    plan: Some(plan),
                    release: &release,
                };
                if s == 0 {
                    // The first visit of a signup was anonymous, on the
                    // landing page, a quarter of an hour before.
                    let anon = format!("anon-{user}");
                    let props = if seen || index % 3 != 0 {
                        "{}".to_owned()
                    } else {
                        let visit = format!("{anon}-s");
                        let guest = Actor {
                            id: &anon,
                            session: Some(&visit),
                            role: None,
                            plan: None,
                            release: &release,
                        };
                        landing_visit(&mut out, &mut rng, &guest, at - Duration::minutes(15));
                        format!(r#"{{"$anon_distinct_id":"{anon}"}}"#)
                    };
                    seen = true;
                    // At most one `$identify` a user-day (D-097).
                    let path = Some("/".to_owned());
                    out.push(&who, at, "$identify".into(), None, path, props);
                }
                let feat = 1 + rng.next() % FEATURES;
                for e in 0..count {
                    // Exponential gaps, mean 25 s.
                    at += Duration::milliseconds((-25_000.0 * (1.0 - rng.unit()).ln()) as i64);
                    if at > now {
                        break;
                    }
                    let ctx = Day {
                        feat,
                        sampled,
                        sample: &sample,
                    };
                    browser_event(&mut out, &mut rng, &who, at, e == 0, &ctx);
                }
            }
        }
        out.events
    }

    fn server_day(&self, project: &str, day: u64, now: DateTime<Utc>) -> Vec<Event> {
        let mut rng = seeded(day, 0x5e7e_0706_0b5e_7a75);
        let day = i64::try_from(day).unwrap_or(0);
        let start = first_day(now) + Duration::days(day);
        let release = format!("v{}", 1 + day / 15);
        let who = Actor {
            id: "$server",
            session: None,
            role: None,
            plan: None,
            release: &release,
        };
        let traffic = self.known as f64 / PERSONA_KNOWN_USERS as f64;
        let mut out = Out::new(project, SERVER, u64::try_from(day).unwrap_or(0));
        if day % 15 == 0 {
            let props = r#"{"process.start":true,"process.memory.usage":90000000}"#.to_owned();
            out.push(&who, start, "$runtime_stats".into(), None, None, props);
        }
        for w in 0..WINDOWS {
            let at = start + Duration::seconds(w * WINDOW_S);
            if at > now {
                break;
            }
            // Busy during the day, quiet at night (UTC).
            let hour = (w / 6) as f64;
            let diurnal =
                0.35 + 0.65 * (0.5 - 0.5 * ((hour - 3.0) / 24.0 * std::f64::consts::TAU).cos());
            for r in 0..ROUTES {
                let (method, route) = route(r);
                let base = 2_400.0 / (r as f64 + 1.0);
                let count = (base * diurnal * traffic * (0.9 + 0.2 * rng.unit()))
                    .round()
                    .max(1.0) as u64;
                let median = 2 + (r % 5) as usize;
                let props = request_stats(method, &route, 200, count, median);
                out.push(&who, at, "$request_stats".into(), None, None, props);
                if rng.chance(0.03) {
                    let props = request_stats(method, &route, 500, (count / 50).max(1), median + 2);
                    out.push(&who, at, "$request_stats".into(), None, None, props);
                }
            }
            let props = format!(
                r#"{{"$lib":"node","nodejs.eventloop.delay.p50":{:.4},"nodejs.eventloop.delay.p99":{:.4},"process.memory.usage":{},"v8js.memory.heap.used":{},"$window_s":{WINDOW_S}}}"#,
                0.010 + 0.004 * rng.unit(),
                0.040 + 0.040 * rng.unit(),
                180_000_000 + rng.next() % 40_000_000,
                90_000_000 + rng.next() % 20_000_000,
            );
            out.push(&who, at, "$runtime_stats".into(), None, None, props);
        }
        out.events
    }
}

/// A visitor who never signs up: one visit to the landing page.
fn visitor(project: &str, index: u64, now: DateTime<Utc>) -> Vec<Event> {
    let mut rng = seeded(index, 0x0a11_0706_5ee5_0f0f);
    let id = format!("anon-v{index}");
    let day = rng.range(0, DAYS);
    let at = first_day(now) + Duration::days(day) + Duration::seconds(rng.range(0, 80_000));
    let session = format!("{id}-s");
    let release = format!("v{}", 1 + day / 15);
    let who = Actor {
        id: &id,
        session: Some(&session),
        role: None,
        plan: None,
        release: &release,
    };
    let mut out = Out::new(project, VISITOR, index);
    landing_visit(&mut out, &mut rng, &who, at);
    out.events
}

/// A few page views and clicks on the landing page, from `at`.
fn landing_visit(out: &mut Out, rng: &mut Rng, who: &Actor, mut at: DateTime<Utc>) {
    let count = rng.poisson(3.0).max(1);
    for e in 0..count {
        let name = if e == 0 || rng.chance(0.5) {
            "$pageview"
        } else {
            "$click"
        };
        let path = Some(LANDING[(rng.next() % LANDING.len() as u64) as usize].to_owned());
        out.push(who, at, name.into(), None, path, "{}".into());
        at += Duration::seconds(rng.range(5, 60));
    }
}

/// What one active user-day shares between its events.
struct Day<'a> {
    feat: u64,
    sampled: bool,
    /// `,"$sample":rate`, or empty when every session is measured.
    sample: &'a str,
}

/// One browser event of a known user at `at` (`first` opens the session
/// with a page view), with the distribution events that follow it on a
/// sampled day.
fn browser_event(
    out: &mut Out,
    rng: &mut Rng,
    who: &Actor,
    at: DateTime<Utc>,
    first: bool,
    day: &Day,
) {
    let feat = day.feat;
    let in_feature = rng.chance(0.8);
    let path = if in_feature {
        format!("/app/feat{feat}/{}", rng.next() % 1_000)
    } else {
        OTHER_PATHS[(rng.next() % OTHER_PATHS.len() as u64) as usize].to_owned()
    };
    let pick = if first { 0 } else { rng.next() % 100 };
    let (name, props): (String, String) = match pick {
        0..=7 => ("$pageview".into(), "{}".into()),
        8..=35 if day.sampled => (
            "$click".into(),
            format!(
                r#"{{"$x":{},"$y":{},"$vw":1280}}"#,
                rng.next() % 1280,
                rng.next() % 900
            ),
        ),
        8..=35 => ("$click".into(), "{}".into()),
        36..=46 => (format!("feat{feat}_done"), "{}".into()),
        47..=52 => (
            CUSTOM[(rng.next() % CUSTOM.len() as u64) as usize].into(),
            "{}".into(),
        ),
        53..=57 => ("$submit".into(), "{}".into()),
        58 => (
            "$request_failed".into(),
            format!(
                r#"{{"$method":"GET","$path":"/api/feat{feat}/{}","$status":500}}"#,
                rng.next() % 1_000
            ),
        ),
        59..=60 => (
            "$error".into(),
            r#"{"$message":"boom","$source":"https://app.example.com/a.js:1:2"}"#.into(),
        ),
        _ => ("$feature".into(), "{}".into()),
    };
    let feature = (in_feature && matches!(name.as_str(), "$click" | "$feature" | "$submit")
        || name.ends_with("_done"))
    .then(|| format!("feat{feat}"));
    let sample = day.sample;
    if day.sampled && name == "$submit" {
        // The fields left before the form is sent.
        for (i, field) in FIELDS.iter().take(2).enumerate() {
            let props = format!(
                r#"{{"$form":"feat{feat}","$field":"{field}","$ms":{},"$changed":true{sample}}}"#,
                500 + rng.next() % 8_000
            );
            let t = at - Duration::seconds(2 * (2 - i as i64));
            out.push(who, t, "$field".into(), None, Some(path.clone()), props);
        }
    }
    let pageview = name == "$pageview";
    out.push(who, at, name, feature, Some(path.clone()), props);
    if day.sampled && pageview {
        // Up to 5 vitals and the scroll depth of the page.
        let vitals = rng.poisson(2.0).min(5) as usize;
        let from = (rng.next() % VITALS.len() as u64) as usize;
        for v in 0..vitals {
            let (n, value) = VITALS[(from + v) % VITALS.len()];
            let props = format!(r#"{{"$name":"{n}","$value":{value}{sample}}}"#);
            let t = at + Duration::milliseconds(200 * (v as i64 + 1));
            out.push(who, t, "$vital".into(), None, Some(path.clone()), props);
        }
        let props = format!(r#"{{"$depth":{}{sample}}}"#, 10 * (1 + rng.next() % 10));
        let t = at + Duration::seconds(2);
        out.push(who, t, "$scroll".into(), None, Some(path), props);
    }
}

/// Route `r` of the 30: the 24 feature APIs, then six others.
fn route(r: u64) -> (&'static str, String) {
    if r < FEATURES {
        let method = if r % 4 == 3 { "POST" } else { "GET" };
        (method, format!("/api/feat{}/:id", r + 1))
    } else {
        let (m, p) = OTHER_ROUTES[((r - FEATURES) as usize) % OTHER_ROUTES.len()];
        (m, p.to_owned())
    }
}

/// One `$request_stats` histogram point (#601) of `count` requests around
/// bucket `median`, as the server SDKs store it.
fn request_stats(method: &str, route: &str, status: u16, count: u64, median: usize) -> String {
    let mut buckets = [0u64; BOUNDS_S.len() + 1];
    let last = buckets.len() - 1;
    let spread = [(0usize, 60u64), (1, 15), (usize::MAX, 20), (3, 5)];
    let mut placed = 0;
    for (offset, share) in spread {
        // usize::MAX stands for the bucket below the median.
        let b = if offset == usize::MAX {
            median.saturating_sub(1)
        } else {
            (median + offset).min(last)
        };
        let n = count * share / 100;
        buckets[b] += n;
        placed += n;
    }
    buckets[median.min(last)] += count - placed;
    // A request is counted at the middle of its bucket.
    let mid = |b: usize| {
        let lo = if b == 0 { 0.0 } else { BOUNDS_S[b - 1] };
        let hi = BOUNDS_S.get(b).copied().unwrap_or(lo * 2.0);
        (lo + hi) / 2.0
    };
    let sum: f64 = buckets
        .iter()
        .enumerate()
        .map(|(b, n)| *n as f64 * mid(b))
        .sum();
    let lowest = buckets.iter().position(|n| *n > 0).unwrap_or(0);
    let highest = buckets.iter().rposition(|n| *n > 0).unwrap_or(0);
    let min = if lowest == 0 {
        0.001
    } else {
        BOUNDS_S[lowest - 1]
    };
    let max = BOUNDS_S.get(highest).copied().unwrap_or(20.0);
    let list = buckets.map(|n| n.to_string()).join(",");
    format!(
        r#"{{"$lib":"node","$metric":"http.server.request.duration","$unit":"s","http.route":"{route}","http.request.method":"{method}","http.response.status_code":{status},"count":{count},"sum":{sum:.6},"min":{min},"max":{max},"bucket_counts":[{list}],"$window_s":{WINDOW_S}}}"#
    )
}

/// UTC midnight 90 days before `now`'s day: a day of the profile is a
/// UTC day, as the SDK's once-a-day `$identify` is (D-097), and the last
/// one is yesterday.
fn first_day(now: DateTime<Utc>) -> DateTime<Utc> {
    (now - Duration::days(DAYS))
        .date_naive()
        .and_time(NaiveTime::MIN)
        .and_utc()
}

fn seeded(index: u64, salt: u64) -> Rng {
    let mut rng = Rng(index.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ salt | 1);
    for _ in 0..4 {
        rng.next(); // spread the seed
    }
    rng
}

/// Who sends an event.
struct Actor<'a> {
    id: &'a str,
    session: Option<&'a str>,
    role: Option<&'a str>,
    plan: Option<&'a str>,
    release: &'a str,
}

/// The events of one unit, with ids unique across units.
struct Out<'a> {
    project: &'a str,
    space: u16,
    unit: u64,
    events: Vec<Event>,
}

impl<'a> Out<'a> {
    fn new(project: &'a str, space: u16, unit: u64) -> Self {
        Self {
            project,
            space,
            unit,
            events: Vec::new(),
        }
    }

    fn push(
        &mut self,
        who: &Actor,
        at: DateTime<Utc>,
        name: String,
        feature: Option<String>,
        path: Option<String>,
        properties: String,
    ) {
        let seq = self.events.len() as u64 + 1;
        self.events.push(Event {
            uuid: format!(
                "{:08x}-{:04x}-7000-8000-{:012x}",
                self.unit & 0xffff_ffff,
                self.space,
                seq & 0xffff_ffff_ffff
            ),
            project: self.project.into(),
            event: name,
            distinct_id: who.id.into(),
            session_id: who.session.map(Into::into),
            timestamp: at,
            received_at: at,
            url: None,
            path,
            feature,
            role: who.role.map(Into::into),
            account: None,
            plan: who.plan.map(Into::into),
            environment: "production".into(),
            release: Some(who.release.into()),
            flags: None,
            properties,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    fn known(p: &Persona, n: u64, now: DateTime<Utc>) -> Vec<Event> {
        (0..n).flat_map(|i| p.unit("p", i, now)).collect()
    }

    #[test]
    fn a_unit_is_the_same_whenever_it_is_generated() {
        let now = Utc::now();
        let p = Persona::full();
        for k in [7, p.known_users() + 3, p.units() - 1] {
            let a = p.unit("p", k, now);
            let b = p.unit("p", k, now);
            assert!(!a.is_empty());
            assert!(
                a.iter()
                    .zip(&b)
                    .all(|(x, y)| x.uuid == y.uuid && x.timestamp == y.timestamp)
            );
        }
    }

    #[test]
    fn active_user_days_store_about_25_events_with_one_identify_at_most() {
        let now = Utc::now();
        let p = Persona::full();
        let evs = known(&p, 300, now);
        let mut per_day: HashMap<(String, String), usize> = HashMap::new();
        let mut identifies: HashMap<(String, String), usize> = HashMap::new();
        for e in evs.iter().filter(|e| e.distinct_id.starts_with('u')) {
            let key = (e.distinct_id.clone(), e.timestamp.format("%F").to_string());
            if e.event == "$identify" {
                *identifies.entry(key.clone()).or_default() += 1;
            }
            *per_day.entry(key).or_default() += 1;
        }
        let mean = per_day.values().sum::<usize>() as f64 / per_day.len() as f64;
        assert!((18.0..32.0).contains(&mean), "events per user-day: {mean}");
        assert!(identifies.values().all(|n| *n == 1));
        // A quarter of the user-days are active, give or take the sample.
        let active = per_day.len() as f64 / (300.0 * DAYS as f64);
        assert!((0.15..0.35).contains(&active), "active share: {active}");
        let share = |n: &str| evs.iter().filter(|e| e.event == n).count() as f64 / evs.len() as f64;
        assert!(share("$click") > share("$pageview"));
        assert!(share("$pageview") > share("$submit"));
        assert!(share("$submit") > share("$request_failed"));
    }

    #[test]
    fn only_sampled_days_carry_distribution_events_and_they_say_so() {
        let now = Utc::now();
        let p = Persona::full();
        assert!(p.sample_rate() < 1.0);
        let evs = known(&p, 600, now);
        let distribution: Vec<_> = evs
            .iter()
            .filter(|e| matches!(e.event.as_str(), "$vital" | "$scroll" | "$field"))
            .collect();
        assert!(!distribution.is_empty());
        assert!(
            distribution
                .iter()
                .all(|e| e.properties.contains("$sample"))
        );
        assert!(
            evs.iter()
                .filter(|e| !matches!(e.event.as_str(), "$vital" | "$scroll" | "$field"))
                .all(|e| !e.properties.contains("$sample"))
        );
        // Far fewer user-days carry them than are active.
        let days = |pred: &dyn Fn(&Event) -> bool| {
            evs.iter()
                .filter(|e| pred(e))
                .map(|e| (e.distinct_id.clone(), e.timestamp.format("%F").to_string()))
                .collect::<HashSet<_>>()
                .len()
        };
        let sampled = days(&|e| e.event == "$scroll");
        let active = days(&|e| e.event == "$identify");
        assert!(sampled * 5 < active, "{sampled} sampled of {active}");
        // A small app is measured in full, with no `$sample`.
        let small = Persona::scaled(1_000_000);
        assert!(small.known_users() < p.known_users());
        assert!(
            known(&small, 50, now)
                .iter()
                .all(|e| !e.properties.contains("$sample"))
        );
    }

    #[test]
    fn a_third_of_users_are_first_seen_anonymous_and_visitors_outnumber_signups() {
        let now = Utc::now();
        let p = Persona::full();
        assert!(p.visitors() > p.signups());
        let evs = known(&p, 90, now);
        let linked: HashSet<_> = evs
            .iter()
            .filter(|e| e.event == "$identify" && e.properties.contains("$anon_distinct_id"))
            .map(|e| e.distinct_id.clone())
            .collect();
        let users: HashSet<_> = evs
            .iter()
            .filter(|e| e.distinct_id.starts_with('u'))
            .map(|e| e.distinct_id.clone())
            .collect();
        assert!(linked.len() * 2 < users.len() && linked.len() * 5 > users.len());
        for e in evs.iter().filter(|e| e.distinct_id.starts_with("anon-")) {
            assert!(e.role.is_none() && e.plan.is_none());
        }
        let visitor = p.unit("p", p.known_users(), now);
        assert!(visitor.iter().all(|e| e.distinct_id.starts_with("anon-v")));
        assert_eq!(visitor.first().map(|e| e.event.as_str()), Some("$pageview"));
        let plans: HashSet<_> = evs.iter().filter_map(|e| e.plan.as_deref()).collect();
        assert_eq!(plans.len(), 2);
    }

    #[test]
    fn a_server_day_has_every_route_in_every_window() {
        let now = Utc::now();
        let p = Persona::full();
        let day = p.unit("p", p.known_users() + p.visitors() + 10, now);
        let routes: HashSet<_> = day
            .iter()
            .filter(|e| e.event == "$request_stats")
            .filter_map(|e| e.properties.split("\"http.route\":\"").nth(1))
            .filter_map(|s| s.split('"').next())
            .collect();
        assert_eq!(routes.len() as u64, ROUTES);
        let runtime = day.iter().filter(|e| e.event == "$runtime_stats").count();
        assert!(runtime as i64 >= WINDOWS);
        assert!(
            day.iter()
                .all(|e| e.distinct_id == "$server" && e.session_id.is_none())
        );
    }

    #[test]
    fn the_scaled_persona_follows_the_event_count() {
        assert!(
            Persona::scaled(2_000_000).known_users() < Persona::scaled(4_000_000).known_users()
        );
        assert_eq!(
            Persona::scaled(PERSONA_NOMINAL_EVENTS).known_users(),
            Persona::full().known_users()
        );
    }
}

//! A freemium app with plans (#712, #713): 300 users who start free; the
//! ones who use [`CONVERSION_DRIVER`] in their first week move to `pro`
//! far more often than the others, and a few move back. Everyone uses
//! `search`, which changes nothing. Some users send a plan value the map
//! below does not match ([`UNMATCHED_PLAN`]).

use chrono::{DateTime, Duration, Utc};
use openlens_event::Event;

use super::Rng;

/// Feature whose first-week use drives the free → paid move.
pub const CONVERSION_DRIVER: &str = "invite";
/// The raw plan no plan of [`plans_feature_map`] matches.
pub const UNMATCHED_PLAN: &str = "legacy";

/// The feature map of [`plans_dataset`]: a free plan, then `pro`.
pub fn plans_feature_map() -> String {
    "version: 1\nroles:\n  - key: member\n    match: [member]\n\
     plans:\n  - key: free\n    match: [free]\n    paid: false\n  - key: pro\n    match: ['pro*']\n\
     features:\n  - key: search\n    name: Search\n    entry: { routes: ['/search'] }\n\
     \x20 - key: invite\n    name: Invite a teammate\n    entry: { routes: ['/invite'] }\n"
        .to_owned()
}

/// Who sends an event, and on which plan.
struct Ctx<'a> {
    project: &'a str,
    user: &'a str,
    session: &'a str,
    plan: &'a str,
}

fn event(c: &Ctx<'_>, name: &str, at: DateTime<Utc>, path: &str, n: &mut u64) -> Event {
    *n += 1;
    Event {
        uuid: format!("00000000-0000-7000-9000-{:012x}", *n),
        project: c.project.into(),
        event: name.into(),
        distinct_id: c.user.into(),
        session_id: Some(c.session.into()),
        timestamp: at,
        received_at: at,
        url: None,
        path: Some(path.into()),
        feature: None,
        role: Some("member".into()),
        account: None,
        plan: Some(c.plan.into()),
        environment: "production".into(),
        release: Some(if at > Utc::now() - Duration::days(30) {
            "v2".into()
        } else {
            "v1".into()
        }),
        flags: None,
        properties: "{}".into(),
    }
}

/// The dataset: about 90 days of history ending now. Every session starts
/// with an `$identify` carrying the plan of the time, as the SDK sends it.
pub fn plans_dataset(project: &str) -> Vec<Event> {
    let now = Utc::now();
    let mut rng = Rng(0x0713_c0de_feed_5eed);
    let mut out = Vec::new();
    let mut n = 0u64;
    for u in 0..300 {
        let user = format!("f{u}");
        let first = now - Duration::days(rng.range(35, 85)) - Duration::hours(rng.range(0, 20));
        let legacy = u % 25 == 0;
        let early_driver = rng.chance(0.4);
        let converts = !legacy && rng.chance(if early_driver { 0.6 } else { 0.08 });
        let upgrade_at = first + Duration::days(rng.range(8, 25));
        let downgrade_at = (converts && rng.chance(0.15)).then(|| upgrade_at + Duration::days(10));
        let last = (first + Duration::days(40)).min(now - Duration::hours(1));
        let mut day = first;
        let mut s = 0;
        while day <= last {
            s += 1;
            let session = format!("{user}-s{s}");
            let plan = if legacy {
                UNMATCHED_PLAN
            } else if converts && day >= upgrade_at && downgrade_at.is_none_or(|d| day < d) {
                "pro"
            } else {
                "free"
            };
            let c = Ctx {
                project,
                user: &user,
                session: &session,
                plan,
            };
            let mut t = day;
            out.push(event(&c, "$identify", t, "/", &mut n));
            out.push(event(&c, "$pageview", t, "/home", &mut n));
            if rng.chance(0.9) {
                t += Duration::minutes(2);
                out.push(event(&c, "$pageview", t, "/search", &mut n));
            }
            if early_driver && day <= first + Duration::days(7) {
                t += Duration::minutes(2);
                out.push(event(&c, "$pageview", t, "/invite", &mut n));
            }
            day += Duration::days(rng.range(1, 4));
        }
    }
    out
}

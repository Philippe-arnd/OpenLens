//! Release lifecycle (#500, D-049) on stored events: a rollback in
//! production, and the time a release spent in staging first.

use super::*;
use openlens_engine::releases::{State, releases};
use openlens_meta::releases::ReleaseBuild;

fn ev(n: usize, env: &str, release: &str, user: usize, days_ago: i64) -> Event {
    let ts = Utc::now() - Duration::days(days_ago);
    Event {
        uuid: format!("r-{n}"),
        project: "p".into(),
        event: "$pageview".into(),
        distinct_id: format!("u{user}"),
        session_id: None,
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some("/".into()),
        feature: None,
        role: None,
        account: None,
        plan: None,
        environment: env.into(),
        release: Some(release.into()),
        flags: None,
        properties: "{}".into(),
    }
}

#[tokio::test]
async fn a_rollback_and_a_promotion_are_read_from_the_traffic() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    let mut add = |env: &str, release: &str, users: std::ops::Range<usize>, days_ago: i64| {
        for u in users {
            let n = evs.len();
            evs.push(ev(n, env, release, u, days_ago));
        }
    };
    // Production: v1, then v2 takes most of the traffic, then v1 is back.
    add("production", "v1", 0..10, 8);
    add("production", "v1", 0..2, 5);
    add("production", "v2", 2..10, 5);
    add("production", "v1", 0..10, 2);
    // The same build was in staging first, under its automatic name, then
    // under its explicit one.
    add("staging", "2026-01-01.1", 50..52, 7);
    add("staging", "v2", 50..52, 6);
    // Bots are not releases.
    add("bot", "v9", 90..95, 2);
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let build = |env: &str, name: &str| ReleaseBuild {
        environment: env.into(),
        build: "aaaa1111".into(),
        name: name.into(),
        first_seen: 0,
    };
    let builds = [build("staging", "2026-01-01.1"), build("production", "v2")];
    let r = releases(&q, "p", "UTC", None, &builds).await.unwrap();
    let find = |env: &str, rel: &str| {
        r.releases
            .iter()
            .find(|x| x.environment == env && x.release == rel)
            .unwrap()
    };
    assert_eq!(find("production", "v2").state, State::RolledBack);
    assert_eq!(find("production", "v1").state, State::Live);
    assert!(find("production", "v1").users > find("production", "v2").users);
    // Its time in staging starts at the build's first staging event.
    let v2 = find("production", "v2");
    assert_eq!(v2.staged_as.as_deref(), Some("2026-01-01.1"));
    assert!(v2.staging_secs.unwrap() > 24 * 3600);
    assert!(find("production", "v1").staging_secs.is_none());
    assert!(r.releases.iter().all(|x| x.environment != "bot"));
    assert_eq!(r.releases[0].environment, "production", "production first");
    // One environment only.
    let staging = releases(&q, "p", "UTC", Some("staging"), &builds)
        .await
        .unwrap();
    assert!(!staging.releases.is_empty());
    assert!(staging.releases.iter().all(|x| x.environment == "staging"));
}

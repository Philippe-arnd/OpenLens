//! Outgoing dependencies (#608, D-090): a host failing for 15 minutes is
//! announced once under `dep:<host>`, and closes when it recovers.

use openlens_engine::commands::Host;
use openlens_engine::commands::alerts::track_dependencies_at;
use openlens_engine::dependencies::{FAILING_WAIT_SECS, Failing, rule};

use crate::maintenance::TestHost;

fn stripe() -> Vec<Failing> {
    vec![Failing {
        host: "api.stripe.com".into(),
        cause: "api.stripe.com failed 80% of 40 calls in the last 20 minutes".into(),
    }]
}

fn open(h: &TestHost, project: &str) -> bool {
    h.meta()
        .open_incident(project, &rule("api.stripe.com"))
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn a_dependency_failing_for_15_minutes_alerts_once_and_closes_on_recovery() {
    let (h, p) = TestHost::new();
    let t0 = chrono::Utc::now().timestamp() - 3600;
    // Failing, checked every five minutes: nothing said before 15 minutes.
    let mut t = t0;
    while t - t0 < FAILING_WAIT_SECS {
        track_dependencies_at(&h, &p, &stripe(), t).await;
        t += 300;
    }
    assert!(
        open(&h, &p.id),
        "the incident is open from the first failure"
    );
    assert!(h.sent().is_empty(), "not announced before 15 minutes");
    // Still failing at 15 minutes and after: announced once.
    for _ in 0..3 {
        track_dependencies_at(&h, &p, &stripe(), t).await;
        t += 300;
    }
    let sent = h.sent();
    assert_eq!(sent.len(), 1, "announced once: {sent:?}");
    assert!(sent[0].0.contains("dependency") && sent[0].1.contains("api.stripe.com"));
    // Recovered: closed, and said once.
    track_dependencies_at(&h, &p, &[], t).await;
    assert!(!open(&h, &p.id));
    assert_eq!(h.sent().len(), 2);
    track_dependencies_at(&h, &p, &[], t + 300).await;
    assert_eq!(h.sent().len(), 2, "nothing more once closed");
}

#[tokio::test]
async fn a_short_failure_is_recorded_but_never_announced() {
    let (h, p) = TestHost::new();
    let t0 = chrono::Utc::now().timestamp() - 3600;
    track_dependencies_at(&h, &p, &stripe(), t0).await;
    track_dependencies_at(&h, &p, &stripe(), t0 + 300).await;
    track_dependencies_at(&h, &p, &[], t0 + 600).await;
    assert!(!open(&h, &p.id));
    assert!(h.sent().is_empty(), "ten minutes of failures say nothing");
}

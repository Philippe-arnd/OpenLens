//! Outgoing dependencies (#608, D-090): a host failing for 15 minutes is
//! announced once under `dep:<host>`, and closes when it recovers; only
//! hosts are ever read back, never a path or a query (#630).

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

/// Stats stored with a URL in `server.address` (a client that skipped
/// the ingestion's check) are never read back as a host: the Errors
/// page's Dependencies table shows hosts only (#630).
#[tokio::test]
async fn only_hosts_are_read_back_never_a_path_or_a_query() {
    let at = chrono::Utc::now() - chrono::Duration::hours(1);
    let ev = |n: usize, address: &str| {
        let mut buckets = vec![0; openlens_engine::routes::BUCKETS];
        buckets[3] = 60;
        openlens_event::Event {
            uuid: format!("dep-{n}"),
            project: "p".into(),
            event: openlens_event::names::CLIENT_STATS.into(),
            distinct_id: "$server".into(),
            session_id: None,
            timestamp: at,
            received_at: at,
            url: None,
            path: None,
            feature: None,
            role: None,
            account: None,
            plan: None,
            environment: "production".into(),
            release: Some("v1".into()),
            flags: None,
            properties: serde_json::json!({
                "http.request.method": "GET", "http.response.status_code": 200,
                "server.address": address, "count": 60, "sum": 2.4, "max": 0.05,
                "bucket_counts": buckets, "$window_s": 600,
            })
            .to_string(),
        }
    };
    let evs = [
        ev(1, "api.stripe.com"),
        ev(2, "https://api.example.com/v1/users?token=secret"),
        ev(3, "api.example.com/v1/users"),
        ev(4, "api.example.com?token=secret"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) =
        openlens_storage::Writer::open(dir.path(), openlens_storage::FlushPolicy::default())
            .unwrap();
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = openlens_query::QueryEngine::new(dir.path(), openlens_query::Limits::default());
    let hosts = openlens_engine::dependencies::list(&q, "p", "production", 1)
        .await
        .unwrap();
    assert!(
        hosts.iter().any(|h| h.host == "api.stripe.com"),
        "{hosts:?}"
    );
    assert!(
        hosts
            .iter()
            .all(|h| !h.host.contains(['/', '?', '=']) && !h.host.contains("secret")),
        "{hosts:?}"
    );
}

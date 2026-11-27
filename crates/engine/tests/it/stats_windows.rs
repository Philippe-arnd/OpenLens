//! The route-stats window follows the host (#628, D-091): the same day
//! stored as 10-minute windows and as 1-minute windows reads the same in
//! the routes, the failing-route and failing-host alerts, the runtime
//! hours and restarts, and the verdict's checks. No reader assumes a
//! window length.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use openlens_engine::{dependencies, routes, runtime};
use openlens_event::Event;
use openlens_query::{Limits, QueryEngine};
use openlens_storage::{FlushPolicy, Writer};

const DAY_WINDOWS: i64 = 144;

fn ev(
    n: &mut usize,
    event: &str,
    release: &str,
    at: DateTime<Utc>,
    props: serde_json::Value,
) -> Event {
    *n += 1;
    Event {
        uuid: format!("sw-{n}"),
        project: "p".into(),
        event: event.into(),
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
        release: Some(release.into()),
        flags: None,
        properties: props.to_string(),
    }
}

/// One histogram point of `count` requests (or calls), all in bucket 3.
fn point(target: (&str, &str), status: i64, count: i64, window_s: i64) -> serde_json::Value {
    let mut buckets = vec![0; routes::BUCKETS];
    buckets[3] = count;
    let mut p = serde_json::json!({
        "http.request.method": "GET", "http.response.status_code": status,
        "count": count, "sum": count as f64 * 0.04, "min": 0.03, "max": 0.05,
        "bucket_counts": buckets, "$window_s": window_s,
    });
    p[target.0] = target.1.into();
    p
}

/// One day of stats ending 5 minutes ago, in windows of `split` per 10
/// minutes (1: 10-minute windows, 10: 1-minute windows). Every number is
/// spread evenly over the finer windows, so both stores hold the same
/// requests, calls and process starts.
fn day(split: i64) -> Vec<Event> {
    let end = Utc::now() - Duration::minutes(5);
    let len = 600 / split;
    let mut n = 0;
    let mut out = Vec::new();
    let first = end - Duration::minutes(10 * DAY_WINDOWS);
    for k in 0..DAY_WINDOWS {
        // k = 0 is the newest 10-minute window.
        let start10 = end - Duration::minutes(10 * (k + 1));
        let release = if k < DAY_WINDOWS / 2 { "v2" } else { "v1" };
        // The last hour fails on the route, the latest window on the host.
        let (ok, bad) = if k < 6 { (100, 100) } else { (190, 10) };
        let (calls_ok, calls_bad) = if k == 0 { (20, 80) } else { (200, 0) };
        for j in 0..split {
            let at = start10 + Duration::seconds(len * j);
            let route = ("http.route", "/api/entries/:id");
            let host = ("server.address", "api.example.com");
            out.push(ev(
                &mut n,
                "$request_stats",
                release,
                at,
                point(route, 200, ok / split, len),
            ));
            out.push(ev(
                &mut n,
                "$request_stats",
                release,
                at,
                point(route, 500, bad / split, len),
            ));
            out.push(ev(
                &mut n,
                "$client_stats",
                release,
                at,
                point(host, 200, calls_ok / split, len),
            ));
            if calls_bad > 0 {
                out.push(ev(
                    &mut n,
                    "$client_stats",
                    release,
                    at,
                    point(host, 503, calls_bad / split, len),
                ));
            }
            out.push(ev(
                &mut n,
                "$runtime_stats",
                release,
                at,
                serde_json::json!({ "process.memory.usage": 200_000_000 + k, "$window_s": len }),
            ));
        }
    }
    // Process starts, the same in both stores: each release's deploy, one
    // restart of v1, twelve of v2.
    let start = serde_json::json!({ "process.start": true, "process.memory.usage": 1 });
    let v2_from = end - Duration::minutes(10 * (DAY_WINDOWS / 2));
    out.push(ev(&mut n, "$runtime_stats", "v1", first, start.clone()));
    out.push(ev(
        &mut n,
        "$runtime_stats",
        "v1",
        first + Duration::hours(3),
        start.clone(),
    ));
    out.push(ev(&mut n, "$runtime_stats", "v2", v2_from, start.clone()));
    for h in 1..=12 {
        let at = v2_from + Duration::minutes(50 * h);
        out.push(ev(&mut n, "$runtime_stats", "v2", at, start.clone()));
    }
    out
}

fn store(evs: &[Event]) -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

/// Everything the readers answer for one store, as text to compare.
async fn answers(q: &QueryEngine) -> (String, usize, usize) {
    let list = routes::list(q, "p", "production", 2, None).await.unwrap();
    assert!(!list.is_empty());
    let routes: BTreeMap<String, (i64, i64, Vec<i64>)> = list
        .iter()
        .map(|r| (r.name(), (r.requests, r.errors, r.buckets.clone())))
        .collect();
    let failing: Vec<String> = routes::failing_routes(q, "p", None)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.route)
        .collect();
    let hosts: Vec<String> = dependencies::failing_hosts(q, "p")
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.host)
        .collect();
    let since = (Utc::now() - Duration::days(3))
        .format("%Y-%m-%d")
        .to_string();
    let rt = runtime::by_release(q, "p", "production", &since)
        .await
        .unwrap();
    let side = |name: &str| {
        rt.iter()
            .find(|r| r.release == name)
            .map(|r| r.stat.clone())
            .unwrap()
    };
    let (v1, v2) = (side("v1"), side("v2"));
    assert!(v1.hours > 0.0 && v2.restarts > v1.restarts);
    let rt_check = runtime::check(&v2, &v1);
    let pair = routes::latest_releases(q, "p", "production", None)
        .await
        .unwrap()
        .unwrap();
    let before = pair.previous.map(|(_, r)| r).unwrap_or_default();
    let checks: Vec<String> = routes::checks(&pair.current_routes, &before)
        .iter()
        .map(|c| format!("{} {:?}", c.key, c.outcome))
        .collect();
    let text = format!(
        "routes {routes:?}\nfailing {failing:?}\nhosts {hosts:?}\n\
         v1 {} restarts in {:.3} h, v2 {} restarts in {:.3} h\nruntime {:?}\nchecks {checks:?}",
        v1.restarts, v1.hours, v2.restarts, v2.hours, rt_check.outcome
    );
    (text, failing.len(), hosts.len())
}

#[tokio::test]
async fn a_day_in_1_minute_windows_reads_as_the_same_day_in_10_minute_windows() {
    let (_d10, q10) = store(&day(1));
    let (_d1, q1) = store(&day(10));
    let (ten, failing, hosts) = answers(&q10).await;
    let (one, _, _) = answers(&q1).await;
    // The alerts have something to agree on.
    assert!(failing > 0 && hosts > 0, "{ten}");
    assert_eq!(ten, one);
}

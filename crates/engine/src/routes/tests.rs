use super::*;

fn stat(requests: i64, errors: i64, fast: i64, slow_bucket: usize) -> RouteStat {
    let mut buckets = vec![0; BUCKETS];
    buckets[2] = fast;
    buckets[slow_bucket] += requests - fast;
    RouteStat {
        method: "POST".into(),
        route: "/api/login".into(),
        requests,
        errors,
        sum_ms: 0,
        max_ms: 20_000,
        buckets,
        ..RouteStat::default()
    }
}

#[test]
fn p95_comes_from_the_histogram_and_never_passes_the_largest_duration() {
    let s = stat(100, 0, 96, 6);
    assert!(s.p95() <= BOUNDS_MS[2] && s.p50() <= s.p95());
    let slow = stat(100, 0, 50, 8);
    assert!(slow.p95() > s.p95());
    let mut tiny = stat(10, 0, 0, BUCKETS - 1);
    tiny.max_ms = 12_000;
    assert!(tiny.p95() <= 12_000.0);
}

#[test]
fn a_route_that_starts_failing_or_slows_down_is_worse_and_too_little_traffic_is_not_judged() {
    let before = [stat(400, 0, 390, 4)];
    let broken = [stat(400, 400, 390, 4)];
    let worse = checks(&broken, &before);
    assert!(
        worse
            .iter()
            .any(|c| c.key.starts_with("5xx:") && c.outcome == Outcome::Worse)
    );
    let slow = [stat(400, 0, 100, 9)];
    assert!(
        checks(&slow, &before)
            .iter()
            .any(|c| c.key.starts_with("p95:") && c.outcome == Outcome::Worse)
    );
    assert!(
        checks(&before, &before)
            .iter()
            .all(|c| c.outcome != Outcome::Worse)
    );
    assert!(checks(&[stat(10, 10, 0, 9)], &before).is_empty());
}

#[test]
fn a_failing_hour_alerts_only_well_above_the_usual_share() {
    let week = stat(10_000, 100, 9_000, 4);
    assert!(failing(&stat(100, 80, 90, 4), Some(&week)).is_some());
    assert!(failing(&stat(100, 2, 90, 4), Some(&week)).is_none());
    assert!(failing(&stat(5, 5, 0, 4), Some(&week)).is_none());
}

#[test]
fn a_days_p95_comes_from_its_merged_histograms_never_an_average_of_p95s() {
    let days = trend_days(chrono::Utc::now());
    assert_eq!(days.len() as i64, TREND_DAYS);
    let fast = stat(100, 0, 100, 2);
    let slow = stat(10, 0, 0, 10);
    let today = days.last().cloned().unwrap_or_default();
    let rows = vec![(today.clone(), fast.clone()), (today, slow.clone())];
    let series = daily_p95(&rows, &days);
    let series = series.get(&fast.name()).cloned().unwrap_or_default();
    let mut both = fast.clone();
    both.merge(&slow);
    let average = (fast.p95() + slow.p95()) / 2.0;
    assert_eq!(series.last().copied().flatten(), Some(both.p95()));
    assert!((both.p95() - average).abs() > 1.0);
    assert!(
        series.first().copied().flatten().is_none(),
        "a day without requests has no p95"
    );
}

#[test]
fn a_route_that_breaks_moves_more_than_one_that_holds() {
    let before = [stat(400, 0, 390, 4)];
    let broken = [stat(400, 120, 390, 4)];
    let name = before[0].name();
    let moved = changes(&broken, &before);
    let held = changes(&before, &before);
    let (Some(m), Some(h)) = (moved.get(&name), held.get(&name)) else {
        panic!("both are judged");
    };
    assert!(m.worse && m.errors_moved && !h.worse && !h.errors_moved);
    assert!(m.size > h.size && m.size >= 1.0);
    assert!(m.previous_error_rate < m.error_rate);
}

#[test]
fn a_slower_routes_check_carries_its_slowest_request_and_keeps_its_outcome() {
    let before = [stat(400, 0, 390, 4)];
    let mut slow = stat(400, 0, 100, 9);
    let plain = checks(std::slice::from_ref(&slow), &before);
    slow.slowest = Some(Exemplar {
        ms: 980.0,
        session_id: Some("s1".into()),
        ..Exemplar::default()
    });
    let with = checks(std::slice::from_ref(&slow), &before);
    let p95 = |list: &[Check]| {
        list.iter()
            .find(|c| c.key.starts_with("p95:"))
            .map(|c| (c.outcome, c.exemplar.clone()))
    };
    let (Some((o1, e1)), Some((o2, e2))) = (p95(&plain), p95(&with)) else {
        panic!("the p95 is judged");
    };
    assert_eq!(o1, o2, "an exemplar never changes the outcome");
    assert!(e1.is_none() && e2 == slow.slowest);
    // A route that did not get slower carries none.
    let mut same = before[0].clone();
    same.slowest = slow.slowest.clone();
    assert!(
        checks(&[same], &before)
            .iter()
            .all(|c| c.exemplar.is_none())
    );
}

#[test]
fn hostile_exemplars_are_bounded_and_their_bad_fields_left_out() {
    let good = r#"{"value":1.5,"time":"2026-11-27T10:00:00.000Z","trace_id":"5b8efff798038103d269b633813fc60c","filtered_attributes":{"session.id":"s-1","user.id":"u7"}}"#;
    let list = exemplars_of(&format!(r#"{{"exemplars":[{good}]}}"#), 503);
    assert_eq!(list.len(), 1);
    assert!(list[0].ms > 1000.0 && list[0].status == 503);
    assert!(list[0].trace_id.is_some() && list[0].session_id.is_some());
    let shown = serde_json::to_string(&list[0]).unwrap();
    assert!(
        !shown.contains("u7") && !shown.contains("user"),
        "a stored user.id is never read back"
    );
    let hostile = format!(
        r#"{{"exemplars":[{{"value":1,"time":"2026-11-27T10:00:00Z","trace_id":"ZZ{z}","filtered_attributes":{{"session.id":"a b;c","user.id":"{u}"}}}},
        {{"value":-1,"time":"2026-11-27T10:00:00Z"}},{{"value":1e300,"time":"2026-11-27T10:00:00Z"}},
        {{"value":1,"time":"yesterday"}},{{"value":"1","time":"2026-11-27T10:00:00Z"}},{good},{good},{good}]}}"#,
        z = "0".repeat(30),
        u = "x".repeat(10_000)
    );
    let list = exemplars_of(&hostile, 200);
    assert!(list.len() <= MAX_POINT_EXEMPLARS);
    let first = &list[0];
    assert!(first.trace_id.is_none() && first.session_id.is_none());
    assert!(
        list.iter()
            .all(|e| e.ms >= 0.0 && e.ms <= MAX_EXEMPLAR_S * 1000.0)
    );
    assert!(!valid_trace_id(&"0".repeat(32)));
    assert!(exemplars_of("not json", 200).is_empty());
    assert!(exemplars_of(r#"{"exemplars":"x"}"#, 200).is_empty());
}

#[test]
fn key_actions_match_whatever_the_case_and_spacing() {
    assert_eq!(normalize("post   /api/login"), "POST /api/login");
}

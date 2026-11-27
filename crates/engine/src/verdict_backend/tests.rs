use super::*;
use crate::routes::{BUCKETS, Exemplar};
use crate::verdict::{NewError, Side, compare};

fn side(release: &str) -> Side {
    Side {
        release: release.into(),
        users: 120,
        sessions: 400,
        bounced: 100,
        error_sessions: 8,
        task_sessions: 200,
        tasks: 400,
        avg_secs: 60.0,
        ..Side::default()
    }
}

fn route(path: &str, errors: i64, key: bool) -> RouteStat {
    let mut buckets = vec![0; BUCKETS];
    if let Some(b) = buckets.get_mut(3) {
        *b = 300;
    }
    RouteStat {
        method: "POST".into(),
        route: path.into(),
        requests: 300,
        errors,
        buckets,
        max_ms: 40,
        key_of: key.then(|| "sign-in".to_owned()),
        ..RouteStat::default()
    }
}

/// #594's scenario: v2 breaks the sign-in route (5xx 0% → 100%), two
/// other routes do not move.
fn broken() -> Report {
    let mut prev = side("v1");
    prev.routes = vec![
        route("/api/auth/sign-in", 0, true),
        route("/api/a", 3, false),
        route("/api/b", 3, false),
    ];
    let mut new = side("v2");
    let mut sign_in = route("/api/auth/sign-in", 300, true);
    sign_in.first_5xx = Some(Exemplar {
        ms: 30.0,
        time: "2026-11-20T10:00:00.000Z".into(),
        status: 503,
        trace_id: None,
        session_id: Some("browser-1".into()),
        replay: false,
    });
    new.routes = vec![
        sign_in,
        route("/api/a", 3, false),
        route("/api/b", 3, false),
    ];
    new.new_errors.push(NewError {
        fingerprint: "Error: pool @ db.js".into(),
        sample: "Error: pool exhausted".into(),
        users: 3,
        count: 9,
        server: true,
    });
    compare("production", &new, Some(&prev), 30, &[])
}

#[test]
fn a_broken_key_route_leads_the_backend_group_with_the_unchanged_ones_counted() {
    let r = broken();
    let g = backend_layer(&r).unwrap();
    let first = &g.rows[0];
    assert!(first.label.contains("POST /api/auth/sign-in"), "{g:?}");
    assert!(first.key && first.outcome == Outcome::Worse);
    assert!(
        g.rows
            .iter()
            .any(|x| x.label.contains("New server error") && x.label.contains("pool exhausted")),
        "{g:?}"
    );
    let rest = g.rows.last().unwrap();
    assert!(
        rest.label.starts_with("2 other routes") && rest.to == "unchanged",
        "{rest:?}"
    );
    // The lead names the route, in the Backend group, between the others.
    let lead = crate::board_verdict::lead(&r).unwrap();
    assert!(
        lead.headline.contains("/api/auth/sign-in"),
        "{}",
        lead.headline
    );
    let names: Vec<&str> = lead.layers.iter().map(|l| l.name).collect();
    let at = |n: &str| names.iter().position(|x| *x == n);
    assert!(at("Backend").is_some());
    assert!(
        at("Browser").is_none_or(|b| Some(b) < at("Backend")),
        "{names:?}"
    );
    // The table row: worse 5xx, its exemplar's session.
    let rows = worse_routes(&r);
    assert_eq!(rows.len(), 1);
    let w = &rows[0];
    assert!(w.errors_worse && !w.p95_worse);
    assert!(w.error_rate > w.previous_error_rate.unwrap());
    assert!(w.sessions.iter().any(|s| s.session == "browser-1"));
}

#[test]
fn a_release_without_server_stats_has_no_backend_group_and_keeps_its_verdict() {
    let prev = side("v1");
    let new = side("v2");
    let r = compare("production", &new, Some(&prev), 30, &[]);
    assert!(backend_layer(&r).is_none());
    assert!(worse_routes(&r).is_empty());
    assert!(
        crate::board_verdict::layers(&r)
            .iter()
            .all(|l| l.name != "Backend")
    );
    // The browser-only verdict is the same with or without the module.
    assert_eq!(r.verdict, Some(Verdict::Healthy), "{:?}", r.reasons);
}

/// #629: the table names a slower route's slowest request (the p95
/// check's exemplar); a route that only fails more names none, even when
/// it carries one.
#[test]
fn a_slower_route_names_its_slowest_request_and_one_only_failing_more_none() {
    let mut prev = side("v1");
    prev.routes = vec![route("/api/export", 0, false), route("/api/a", 0, false)];
    let slowest = Exemplar {
        ms: 2400.0,
        time: "2026-11-20T10:00:00.000Z".into(),
        session_id: Some("browser-2".into()),
        ..Exemplar::default()
    };
    let mut slow = route("/api/export", 0, false);
    slow.buckets = vec![0; BUCKETS];
    if let Some(b) = slow.buckets.get_mut(10) {
        *b = 300;
    }
    slow.slowest = Some(slowest.clone());
    let mut failing = route("/api/a", 300, false);
    failing.slowest = Some(slowest.clone());
    let mut new = side("v2");
    new.routes = vec![slow, failing];
    let rows = worse_routes(&compare("production", &new, Some(&prev), 30, &[]));
    let of = |p: &str| rows.iter().find(|w| w.route == p);
    let (Some(s), Some(f)) = (of("/api/export"), of("/api/a")) else {
        panic!("both got worse: {rows:?}");
    };
    assert!(s.p95_worse && s.slowest.as_ref() == Some(&slowest), "{s:?}");
    assert!(
        f.errors_worse && !f.p95_worse && f.slowest.is_none(),
        "{f:?}"
    );
}

//! Automatic releases from the build fingerprint (#495, D-044): a new
//! fingerprint is named `YYYY-MM-DD.N`, an explicit release wins and becomes
//! the fingerprint's name for later events, past events keep theirs.

use super::*;

fn event(uuid: u32, env: &str, build: &str, release: Option<&str>) -> serde_json::Value {
    let mut props = serde_json::json!({ "$environment": env, "$build": build });
    if let Some(r) = release {
        props["$release"] = r.into();
    }
    serde_json::json!({
        "event": "$pageview",
        "distinct_id": "u1",
        "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
        "properties": props,
    })
}

async fn capture(h: &Harness, events: Vec<serde_json::Value>) {
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
}

/// The stored release of each event, by the last digits of its uuid.
async fn releases(h: &Harness) -> std::collections::BTreeMap<u32, Option<String>> {
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql("default", "SELECT uuid, release FROM events")
        .await
        .unwrap();
    r.rows
        .iter()
        .map(|row| {
            let uuid = row["uuid"].as_str().unwrap();
            let n = uuid.rsplit('-').next().unwrap().parse().unwrap();
            (
                n,
                row.get("release")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
            )
        })
        .collect()
}

/// `(day, N)` of an automatic name.
fn split(name: &str) -> (&str, u32) {
    let (day, n) = name.rsplit_once('.').unwrap();
    assert!(
        chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok(),
        "{name}"
    );
    (day, n.parse().unwrap())
}

#[tokio::test]
async fn new_fingerprints_get_the_days_next_name_and_keep_it() {
    let h = harness();
    capture(&h, vec![event(1, "production", "aaaa1111", None)]).await;
    capture(
        &h,
        vec![
            event(2, "production", "bbbb2222", None),
            event(3, "production", "aaaa1111", None),
            event(4, "staging", "aaaa1111", None),
            // Not a fingerprint: no release is made up.
            event(5, "production", "../etc", None),
        ],
    )
    .await;
    let r = releases(&h).await;
    let a = r[&1].clone().unwrap();
    let b = r[&2].clone().unwrap();
    assert_eq!(r[&3].as_deref(), Some(a.as_str()), "same build, same name");
    let ((day_a, n_a), (day_b, n_b)) = (split(&a), split(&b));
    assert_eq!(day_a, day_b);
    assert!(n_b > n_a, "{a} then {b}");
    // Each environment counts its own builds.
    assert_eq!(split(r[&4].as_deref().unwrap()).1, n_a);
    assert_eq!(r[&5], None);
}

#[tokio::test]
async fn an_explicit_release_wins_and_names_the_fingerprint_from_then_on() {
    let h = harness();
    capture(&h, vec![event(1, "production", "aaaa1111", None)]).await;
    capture(&h, vec![event(2, "production", "aaaa1111", Some("v1.4.0"))]).await;
    capture(&h, vec![event(3, "production", "aaaa1111", None)]).await;
    // A fingerprint first seen with an explicit release is named by it.
    capture(
        &h,
        vec![
            event(4, "production", "cccc3333", Some("v2.0.0")),
            event(5, "production", "cccc3333", None),
        ],
    )
    .await;
    let r = releases(&h).await;
    let auto = r[&1].clone().unwrap();
    split(&auto);
    assert_eq!(r[&2].as_deref(), Some("v1.4.0"), "explicit wins");
    assert_eq!(
        r[&3].as_deref(),
        Some("v1.4.0"),
        "the alias applies to later events"
    );
    assert_eq!(r[&5].as_deref(), Some("v2.0.0"));
    // Stored events are not rewritten (D-024): the first keeps its name.
    assert_eq!(r[&1].as_deref(), Some(auto.as_str()));
    // The alias is recorded, not only cached.
    assert_eq!(
        h.state
            .meta
            .build_release("default", "production", "aaaa1111")
            .unwrap()
            .as_deref(),
        Some("v1.4.0")
    );
}

/// The Releases page and `GET /api/projects/{id}/releases` (#500, D-049)
/// show each release with its state and a link to the board on it.
#[tokio::test]
async fn the_releases_page_and_api_list_releases_with_their_state() {
    let h = harness();
    capture(
        &h,
        vec![
            event(1, "production", "aaaa1111", Some("v1.0.0")),
            event(2, "staging", "bbbb2222", Some("v1.1.0")),
        ],
    )
    .await;
    openlens_server::flush_now(&h.state).unwrap();
    let r = get(&h, "/api/projects/default/releases", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let rows = v["releases"].as_array().unwrap();
    let v1 = rows
        .iter()
        .find(|x| x["release"] == "v1.0.0" && x["environment"] == "production")
        .unwrap();
    assert_eq!(v1["state"], "live", "all of the day's traffic");
    let r = get(&h, "/api/projects/default/releases?env=staging", None, true).await;
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let staging = v["releases"].as_array().unwrap();
    assert!(!staging.is_empty());
    assert!(staging.iter().all(|x| x["environment"] == "staging"));
    let missing = get(&h, "/api/projects/nope/releases", None, true).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let cookie = login_cookie(&h).await;
    let page = get(&h, "/p/default/releases", Some(&cookie), false).await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(html.contains("v1.1.0"), "every environment");
    assert!(html.contains("/p/default?env=production&amp;release=v1.0.0"));
}

/// A release's verdict (#522, D-067): an error group first seen in it,
/// hitting several users, makes it regressed against the release before,
/// in its report and in the list; the first release has nothing to be
/// compared with.
#[tokio::test]
async fn a_new_error_in_a_release_makes_its_verdict_regressed() {
    let h = harness();
    let ago = |hours: i64| (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339();
    let mut events = Vec::new();
    for i in 0..4 {
        let u = format!("u{i}");
        for (rel, at) in [("v1", 72), ("v2", 30)] {
            let s = format!("{rel}-s{i}");
            events.push(serde_json::json!({"event": "$pageview", "distinct_id": u, "session_id": s, "timestamp": ago(at), "properties": {"$pathname": "/", "$release": rel}}));
            events.push(serde_json::json!({"event": "$vital", "distinct_id": u, "session_id": s, "timestamp": ago(at), "properties": {"$name": "LCP", "$value": 1200, "$release": rel}}));
        }
        events.push(serde_json::json!({"event": "$error", "distinct_id": u, "session_id": format!("v2-s{i}"), "timestamp": ago(29), "properties": {"$message": "TypeError: x is undefined", "$release": "v2"}}));
    }
    capture(&h, events).await;
    openlens_server::flush_now(&h.state).unwrap();

    let r = get(&h, "/api/projects/default/releases/v2/report", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert_eq!(v["previous_release"], "v1", "{v}");
    assert_eq!(v["verdict"], "regressed", "{v}");
    assert!(!v["current"]["new_errors"].as_array().unwrap().is_empty());

    let r = get(&h, "/api/projects/default/releases", None, true).await;
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let rows = v["releases"].as_array().unwrap();
    let verdict = |name: &str| {
        rows.iter()
            .find(|x| x["release"] == name)
            .map(|x| x["verdict"].clone())
            .unwrap()
    };
    assert_eq!(verdict("v2"), "regressed");
    assert_eq!(verdict("v1"), "inconclusive", "no earlier release");

    let none = get(&h, "/api/projects/default/releases/v9/report", None, true).await;
    assert_eq!(none.status(), StatusCode::BAD_REQUEST);
}

/// A regression planted in release N+1 (#614, #615): the board's status
/// strip says it, and its verdict is the lead of what to focus on, in
/// plain words, with the actions to take.
#[tokio::test]
async fn a_regressed_release_leads_the_board_as_its_verdict() {
    let h = harness();
    let ago = |hours: i64| (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339();
    let mut events = Vec::new();
    for i in 0..4 {
        let u = format!("u{i}");
        for (rel, at) in [("v1", 72), ("v2", 30)] {
            let s = format!("{rel}-s{i}");
            events.push(serde_json::json!({"event": "$pageview", "distinct_id": u, "session_id": s, "timestamp": ago(at), "properties": {"$pathname": "/", "$release": rel}}));
        }
        events.push(serde_json::json!({"event": "$error", "distinct_id": u, "session_id": format!("v2-s{i}"), "timestamp": ago(29), "properties": {"$message": "TypeError: x is undefined", "$release": "v2"}}));
    }
    capture(&h, events).await;
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    let strip = &board[board.find(r#"aria-label="Production right now""#).unwrap()..];
    assert!(
        strip.contains("Release v2") && strip.contains(r#"<strong class="v">Regressed</strong>"#),
        "{strip}"
    );
    let lead = &board[board.find(r#"class="card lead""#).unwrap()..];
    let lead = &lead[..lead.find("</article>").unwrap()];
    assert!(
        lead.contains("Release regressed") && lead.contains("Since v2"),
        "{lead}"
    );
    for action in ["Create ticket", "Errors on v2", "Full verdict"] {
        assert!(lead.contains(action), "{action}: {lead}");
    }
    // No server stats (#594 A, #623): no Backend group, no empty number.
    assert!(!lead.contains("Backend") && !lead.contains("5xx"), "{lead}");
    // Its Create ticket drafts a ticket from the verdict.
    let r = get(
        &h,
        "/p/default/tickets/new?kind=release&subject=production%3Av2",
        Some(&owner),
        false,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(body_text(r).await.contains("Fix the regression of v2"));
}

/// #594's scenario on the board (#623): release v2 breaks the sign-in
/// route, a key action (5xx 0% → 100%), while the browser sees nothing
/// new. The board judges v2 regressed and names the route in its lead's
/// Backend group; the Releases page shows the route among those that got
/// worse.
#[tokio::test]
async fn a_release_that_breaks_a_key_route_leads_the_board_with_the_route_named() {
    let h = harness();
    h.state
        .meta
        .save_feature_map(
            "default",
            "features:\n  - key: sign-in\n    name: Sign-in\n    entry: { routes: ['/login'] }\n    server: ['POST /api/auth/sign-in']\n",
        )
        .unwrap();
    let ago = |hours: i64| (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339();
    let mut events = Vec::new();
    for i in 0..4 {
        let u = format!("u{i}");
        for (rel, at) in [("v1", 72), ("v2", 30)] {
            let s = format!("{rel}-s{i}");
            events.push(serde_json::json!({"event": "$pageview", "distinct_id": u, "session_id": s, "timestamp": ago(at), "properties": {"$pathname": "/", "$release": rel}}));
        }
    }
    let mut buckets = [0; 15];
    buckets[3] = 200;
    for (n, (rel, at, status)) in [("v1", 71, 200), ("v2", 29, 503)].into_iter().enumerate() {
        events.push(serde_json::json!({
            "event": "$request_stats", "distinct_id": "$server", "timestamp": ago(at),
            "uuid": format!("0191f0c0-0000-7000-8000-{:012}", 900 + n),
            "properties": {
                "$lib": "node", "$environment": "production", "$release": rel,
                "$metric": "http.server.request.duration", "$unit": "s",
                "http.request.method": "POST", "http.route": "/api/auth/sign-in",
                "http.response.status_code": status,
                "count": 200, "sum": 6.0, "min": 0.001, "max": 0.05,
                "bucket_counts": buckets, "$window_s": 600,
            },
        }));
    }
    capture(&h, events).await;
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    let strip = &board[board.find(r#"aria-label="Production right now""#).unwrap()..];
    let strip = &strip[..strip.find("</section>").unwrap()];
    assert!(
        strip.contains("Release v2") && strip.contains(r#"<strong class="v">Regressed</strong>"#),
        "{strip}"
    );
    assert!(strip.contains("Key actions"), "{strip}");
    let lead = &board[board.find(r#"class="card lead""#).unwrap()..];
    let lead = &lead[..lead.find("</article>").unwrap()];
    assert!(lead.contains("Release regressed"), "{lead}");
    let backend = &lead[lead.find("Backend").expect("a Backend group")..];
    assert!(backend.contains("POST /api/auth/sign-in"), "{lead}");
    assert!(backend.contains(">Key<"), "tagged a key action: {lead}");

    let page = body_text(get(&h, "/p/default/releases", Some(&owner), false).await).await;
    let verdict = &page[page.find("Verdict of").expect("the latest verdict")..];
    assert!(verdict.contains("Backend"), "{verdict}");
    let worse = &page[page
        .find("Routes that got worse")
        .expect("the routes table")..];
    assert!(worse.contains("/api/auth/sign-in"), "{worse}");
}

/// The board judges the newest release alone (#748), on the days since
/// the release before it started, and says what the releases list says
/// for it. An error group seen in the release before is not new.
#[tokio::test]
async fn the_board_judges_the_newest_release_as_the_releases_list_does() {
    let h = harness();
    let ago = |hours: i64| (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339();
    let mut events = Vec::new();
    for i in 0..4 {
        let u = format!("u{i}");
        for (rel, at) in [("v1", 24 * 20), ("v2", 24 * 6), ("v3", 30)] {
            let s = format!("{rel}-s{i}");
            events.push(serde_json::json!({"event": "$pageview", "distinct_id": u, "session_id": s, "timestamp": ago(at), "properties": {"$pathname": "/", "$release": rel}}));
        }
        for (rel, at) in [("v2", 24 * 6 - 1), ("v3", 29)] {
            events.push(serde_json::json!({"event": "$error", "distinct_id": u, "session_id": format!("{rel}-s{i}"), "timestamp": ago(at), "properties": {"$message": "TypeError: x is undefined", "$release": rel}}));
        }
    }
    capture(&h, events).await;
    openlens_server::flush_now(&h.state).unwrap();

    let r = get(&h, "/api/projects/default/releases", None, true).await;
    let v: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let listed = v["releases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["release"] == "v3")
        .map(|x| x["verdict"].clone())
        .unwrap();

    let r = get(&h, "/api/projects/default/board", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    let b: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
    let latest = &b["latest_release"];
    assert_eq!(latest["state"], "judged", "{latest}");
    assert_eq!(latest["release"], "v3", "{latest}");
    assert_eq!(latest["previous_release"], "v2", "{latest}");
    assert_eq!(latest["verdict"], listed, "{latest}");
    assert!(
        latest["current"]["new_errors"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "the error was already in v2: {latest}"
    );
}

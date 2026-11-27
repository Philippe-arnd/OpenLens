//! Server errors (#593, #624): the `$server` pseudo user counts in no user
//! number, a server error group is told apart from a browser one, and it
//! links to the browser sessions that made the request, with their replays.

use super::*;
use openlens_engine::releases::releases;
use openlens_engine::{Origin, error_groups_of, mark_replays};

/// One stored event of the last hour, the `n`th newest.
fn ev(
    n: usize,
    who: &str,
    session: Option<&str>,
    event: &str,
    release: &str,
    props: serde_json::Value,
) -> Event {
    let ts = Utc::now() - Duration::minutes(30 + n as i64);
    Event {
        uuid: format!("se-{n}"),
        project: "p".into(),
        event: event.into(),
        distinct_id: who.into(),
        session_id: session.map(Into::into),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some("/".into()),
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

/// A server error as the Node SDK sends it, without a user.
fn server_error(n: usize, session: Option<&str>, trace: Option<&str>) -> Event {
    let mut props = serde_json::json!({
        "$lib": "node", "$kind": "server", "$message": "Error: db down",
        "$source": "/app/dist/api.js:3:4",
    });
    if let Some(t) = trace {
        props["trace_id"] = t.into();
    }
    ev(n, "$server", session, "$error", "v2", props)
}

fn store(evs: &[Event]) -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

/// Every user number the board and the releases give, for `evs`.
async fn user_numbers(evs: &[Event]) -> String {
    let (_d, q) = store(evs);
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    let r = releases(&q, "p", "UTC", None, &[], &[]).await.unwrap();
    let shares: Vec<_> = r
        .releases
        .iter()
        .map(|x| (x.release.clone(), x.users, x.share))
        .collect();
    let cohorts: Vec<_> = b.retention.iter().map(|c| c.users).collect();
    format!(
        "dau {} wau {} mau {} anonymous {} known {} cohorts {cohorts:?} releases {shares:?}",
        b.users.dau, b.users.wau, b.users.mau, b.users.anonymous, b.users.known
    )
}

/// #593 D: server errors without a user change no user number (active
/// users, retention cohorts, anonymous visitors, release shares), whether
/// or not the project identifies its users.
#[tokio::test]
async fn server_events_change_no_user_number() {
    for identified in [false, true] {
        let mut people = Vec::new();
        for (i, u) in ["u1", "u2"].into_iter().enumerate() {
            let s = format!("s{i}");
            if identified {
                people.push(ev(
                    people.len(),
                    u,
                    Some(&s),
                    "$identify",
                    "v1",
                    serde_json::json!({}),
                ));
            }
            people.push(ev(
                people.len(),
                u,
                Some(&s),
                "$pageview",
                "v1",
                serde_json::json!({}),
            ));
            people.push(ev(
                people.len(),
                u,
                Some(&s),
                "$click",
                "v1",
                serde_json::json!({}),
            ));
        }
        // A visitor who never logged in.
        people.push(ev(
            people.len(),
            "anon",
            Some("s9"),
            "$pageview",
            "v1",
            serde_json::json!({}),
        ));
        let mut with_server = people.clone();
        for k in 0..3 {
            with_server.push(server_error(100 + k, None, None));
        }
        let without = user_numbers(&people).await;
        assert_eq!(
            user_numbers(&with_server).await,
            without,
            "identified: {identified}"
        );
    }
}

/// #624: a server error sent with a recorded browser session links to that
/// replay; one sent without a session links through its trace id to the
/// session of the failed request; the origin filter splits the groups.
#[tokio::test]
async fn a_server_error_links_the_browser_sessions_that_made_the_request() {
    let trace = "4bf92f3577b34da6a3ce929d0e0e4736";
    let evs = vec![
        ev(
            0,
            "u1",
            Some("rec-1"),
            "$pageview",
            "v2",
            serde_json::json!({}),
        ),
        server_error(1, Some("rec-1"), None),
        ev(
            2,
            "u2",
            Some("plain-2"),
            "$request_failed",
            "v2",
            serde_json::json!({"$method": "POST", "$path": "/api/save", "$status": 500, "trace_id": trace}),
        ),
        server_error(3, None, Some(trace)),
        ev(
            4,
            "u3",
            Some("b-3"),
            "$error",
            "v2",
            serde_json::json!({"$message": "TypeError: x is undefined", "$source": "https://app.test/a.js:1:2"}),
        ),
    ];
    let (dir, q) = store(&evs);
    let chunk = serde_json::value::RawValue::from_string("[{\"type\":2}]".into()).unwrap();
    openlens_replay::append(dir.path(), "p", "rec-1", "u1", 0, &chunk, Utc::now()).unwrap();

    let src = openlens_engine::source(None);
    let f = Filters::default();
    let all = error_groups_of(&q, "p", &src, &f, Origin::All)
        .await
        .unwrap();
    let browser = error_groups_of(&q, "p", &src, &f, Origin::Browser)
        .await
        .unwrap();
    let mut server = error_groups_of(&q, "p", &src, &f, Origin::Server)
        .await
        .unwrap();
    assert!(!browser.is_empty() && !server.is_empty(), "{all:#?}");
    assert_eq!(all.len(), browser.len() + server.len());
    assert!(browser.iter().all(|g| !g.server) && server.iter().all(|g| g.server));
    assert!(server.iter().all(|g| g.users == 0), "$server is no user");

    mark_replays(dir.path(), "p", &mut server);
    let linked = &server[0].linked;
    let of = |s: &str| linked.iter().find(|x| x.session == s);
    assert!(of("rec-1").is_some_and(|x| x.replay), "{linked:#?}");
    let through_trace = of("plain-2").unwrap();
    assert!(!through_trace.replay, "no recording of that session");
    assert_eq!(through_trace.user.as_deref(), Some("u2"));
    assert!(linked.iter().all(|x| x.session != "b-3"));
}

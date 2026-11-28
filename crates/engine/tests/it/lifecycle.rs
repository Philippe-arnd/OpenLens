//! The signup date (#707), the lifecycle (#708) and the Visitors block
//! (#709), on small datasets counted by hand (D-111).

use chrono::{Duration, Utc};
use openlens_engine::lifecycle::{By, Lifecycle, lifecycle};
use openlens_engine::{Filters, board};
use openlens_event::Event;
use openlens_query::{Limits, QueryEngine};
use openlens_storage::{FlushPolicy, Writer};

/// One event `mins_ago`, in production.
pub(super) fn ev(
    who: &str,
    event: &str,
    session: &str,
    mins_ago: i64,
    path: &str,
    props: &str,
) -> Event {
    let ts = Utc::now() - Duration::minutes(mins_ago);
    Event {
        uuid: format!("{who}-{event}-{session}-{mins_ago}"),
        project: "p".into(),
        event: event.into(),
        distinct_id: who.into(),
        session_id: Some(session.into()),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some(path.into()),
        feature: None,
        role: None,
        account: None,
        plan: None,
        environment: "production".into(),
        release: Some("v1".into()),
        flags: None,
        properties: props.into(),
    }
}

pub(super) fn store(evs: &[Event]) -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

fn stage(l: &Lifecycle, key: &str) -> Option<i64> {
    l.total
        .stages
        .iter()
        .find(|s| s.key == key)
        .map(|s| s.people)
}

/// #707: a customer created a year ago and seen for the first time today
/// signed up a year ago; a user without `created_at` signed up on their
/// first `identify`. An internal tool (everyone logged in, no revenue)
/// has no visitor and no paid stage, and the board no Visitors block.
#[tokio::test]
async fn a_backfilled_user_is_not_a_signup_today_and_an_internal_tool_has_no_visitor_stage() {
    let year_ago = (Utc::now() - Duration::days(365)).to_rfc3339();
    let created = format!(r#"{{"$created_at":"{year_ago}"}}"#);
    let mut evs = vec![
        ev("old", "$identify", "s-old", 50, "/", &created),
        ev("old", "$pageview", "s-old", 49, "/home", "{}"),
        ev("new", "$identify", "s-new", 40, "/", "{}"),
        ev("new", "$pageview", "s-new", 39, "/home", "{}"),
    ];
    for k in 0..3 {
        evs.push(ev("colleague", "$identify", "s-c", 30 + k, "/", "{}"));
    }
    evs.push(ev("colleague", "$pageview", "s-c", 20, "/tasks", "{}"));
    let (_dir, q) = store(&evs);
    let f = Filters::default();
    let l = lifecycle(&q, "p", None, &f, By::None).await.unwrap();
    let accounts = stage(&l, "account").unwrap();
    // Three people identified today, two of them new: the backfilled one
    // is not a signup of this window.
    assert!(accounts > 0 && accounts < 3, "{l:?}");
    assert!(
        stage(&l, "visitor").is_none(),
        "everyone is logged in: {l:?}"
    );
    assert!(stage(&l, "paid").is_none(), "no revenue: {l:?}");
    assert!(l.total.visitor_rate.is_none());
    let b = board(&q, "p", &f, None).await.unwrap();
    assert!(
        b.visitors.is_none(),
        "no anonymous traffic, no Visitors block"
    );
    assert_eq!(
        b.lifecycle.as_ref().map(|x| &x.total),
        Some(&l.total),
        "the board and the command agree"
    );
}

/// #708, #709: a landing page and signups. Five anonymous visitors land
/// on `/` from search; two of them sign up (their anonymous id is linked
/// by `identify`), three never do; a third account signs up with no
/// visit on record. By hand: 3 never-linked visitors, 2 linked accounts,
/// 3 accounts: the lower bound is 2 / (3 + 2), the upper 3 / (3 + 3).
#[tokio::test]
async fn a_landing_page_gives_a_visitor_to_account_rate_between_its_bounds() {
    let landing = r#"{"$channel":"search"}"#;
    let mut evs = Vec::new();
    for i in 1..=5 {
        let anon = format!("anon-{i}");
        let s = format!("s-{i}");
        evs.push(ev(&anon, "$pageview", &s, 120, "/", landing));
        // Two of them leave at once (a bounce), the others read on.
        if i > 2 {
            evs.push(ev(&anon, "$pageview", &s, 118, "/pricing", "{}"));
        }
    }
    for (user, anon) in [("u1", "anon-1"), ("u2", "anon-4")] {
        let link = format!(r#"{{"$anon_distinct_id":"{anon}"}}"#);
        evs.push(ev(
            user,
            "$identify",
            &format!("s-{user}"),
            100,
            "/signup",
            &link,
        ));
        evs.push(ev(
            user,
            "$pageview",
            &format!("s-{user}"),
            99,
            "/app",
            "{}",
        ));
    }
    evs.push(ev("u3", "$identify", "s-u3", 90, "/signup", "{}"));
    evs.push(ev("u3", "$pageview", "s-u3", 89, "/app", "{}"));
    let (_dir, q) = store(&evs);
    let f = Filters::default();
    let l = lifecycle(&q, "p", None, &f, By::None).await.unwrap();
    let rate = l.total.visitor_rate.clone().expect("anonymous traffic");
    assert!(rate.available, "{rate:?}");
    assert_eq!(
        (rate.visitors, rate.linked_accounts, rate.accounts),
        (3, 2, 3),
        "{l:?}"
    );
    assert!(rate.rate > 0.0 && rate.rate < rate.upper, "{rate:?}");
    assert!(stage(&l, "visitor").unwrap() > stage(&l, "account").unwrap());

    let by = lifecycle(&q, "p", None, &f, By::Channel).await.unwrap();
    let search = by
        .groups
        .iter()
        .find(|g| g.value == "search")
        .expect("{by:?}");
    let linked = search
        .visitor_rate
        .as_ref()
        .map_or(0, |r| r.linked_accounts);
    assert!(linked > 0, "the linked accounts landed from search: {by:?}");

    let b = board(&q, "p", &f, None).await.unwrap();
    let v = b.visitors.expect("a landing page shows the Visitors block");
    let home = v.pages.iter().find(|p| p.value == "/").expect("{v:?}");
    assert!(
        home.signed_up > 0 && home.signed_up < home.visitors,
        "{home:?}"
    );
    assert!(v.bounces > 0 && v.bounces < v.sessions, "{v:?}");
    assert!(
        v.channels
            .iter()
            .any(|c| c.value == "search" && c.signup_rate() > 0.0)
    );
    assert_eq!(b.lifecycle.map(|x| x.total), Some(l.total));
}

/// #714: with `plans:` in the map, a person whose plan resolves to a paid
/// plan is paid, with no revenue on any event, and one on the free plan
/// is not; without `plans:`, a plan alone makes nobody paid.
#[tokio::test]
async fn a_pro_plan_user_is_paid_when_the_map_declares_plans() {
    let mut evs = Vec::new();
    for (who, plan) in [("payer", "pro"), ("trial", "free")] {
        let s = format!("s-{who}");
        for (event, mins, path) in [("$identify", 60, "/"), ("$pageview", 59, "/home")] {
            let mut e = ev(who, event, &s, mins, path, "{}");
            e.plan = Some(plan.into());
            evs.push(e);
        }
    }
    let (_dir, q) = store(&evs);
    let map = openlens_featuremap::parse(&openlens_testkit::plans_feature_map())
        .map
        .unwrap();
    let f = Filters::default();
    let l = lifecycle(&q, "p", Some((&map, 1)), &f, By::None)
        .await
        .unwrap();
    let paid = stage(&l, "paid").unwrap_or_else(|| panic!("a pro user pays: {l:?}"));
    let accounts = stage(&l, "account").unwrap();
    assert!(paid > 0 && paid < accounts, "{l:?}");
    let without = lifecycle(&q, "p", None, &f, By::None).await.unwrap();
    assert!(
        stage(&without, "paid").is_none(),
        "without plans:, a plan alone is not paid: {without:?}"
    );
}

//! The board's Visitors block (#709, D-111): entry pages and channels of
//! the anonymous sessions, with their bounce and signup rates, on small
//! datasets counted by hand.

use openlens_engine::visitors::VisitorRow;
use openlens_engine::{Filters, board};

use super::lifecycle::{ev, store};

fn row<'a>(rows: &'a [VisitorRow], value: &str) -> &'a VisitorRow {
    rows.iter()
        .find(|r| r.value == value)
        .unwrap_or_else(|| panic!("{value} missing from {rows:?}"))
}

/// Two landing pages, eight anonymous visitors. On `/pricing` four land
/// from search and leave at once (single page views), and three of them
/// sign up later (`identify` links their anonymous id). On `/docs` four
/// land from social and read three pages, and only one signs up. The
/// page with more signups has the higher signup rate, the page of single
/// page views the higher bounce rate: the two orders are opposite, so a
/// swapped column shows.
#[tokio::test]
async fn the_visitors_block_ranks_entry_pages_by_signups_and_bounces() {
    let mut evs = Vec::new();
    for i in 1..=4 {
        let (anon, s) = (format!("p-anon-{i}"), format!("p-s-{i}"));
        evs.push(ev(
            &anon,
            "$pageview",
            &s,
            200,
            "/pricing",
            r#"{"$channel":"search"}"#,
        ));
    }
    for i in 1..=4 {
        let (anon, s) = (format!("d-anon-{i}"), format!("d-s-{i}"));
        evs.push(ev(
            &anon,
            "$pageview",
            &s,
            200,
            "/docs",
            r#"{"$channel":"social"}"#,
        ));
        evs.push(ev(&anon, "$pageview", &s, 198, "/docs/start", "{}"));
        evs.push(ev(&anon, "$pageview", &s, 196, "/pricing", "{}"));
    }
    let signups = [
        ("u1", "p-anon-1"),
        ("u2", "p-anon-2"),
        ("u3", "p-anon-3"),
        ("u4", "d-anon-1"),
    ];
    for (user, anon) in signups {
        let link = format!(r#"{{"$anon_distinct_id":"{anon}"}}"#);
        let s = format!("s-{user}");
        evs.push(ev(user, "$identify", &s, 150, "/signup", &link));
        evs.push(ev(user, "$pageview", &s, 149, "/app", "{}"));
    }
    let (_dir, q) = store(&evs);
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    let v = b
        .visitors
        .expect("anonymous landings show the Visitors block");

    let (pricing, docs) = (row(&v.pages, "/pricing"), row(&v.pages, "/docs"));
    // Entry pages only: `/pricing` is also read later in the docs
    // sessions, which must not count as landings there.
    assert_eq!(pricing.sessions, docs.sessions, "{v:?}");
    assert_eq!(pricing.visitors, docs.visitors, "{v:?}");
    assert!(
        v.pages
            .iter()
            .all(|p| p.value != "/app" && p.value != "/signup"),
        "the signed-in sessions are not anonymous: {v:?}"
    );
    assert!(
        pricing.signup_rate() > docs.signup_rate() && docs.signed_up > 0,
        "{pricing:?} {docs:?}"
    );
    assert!(pricing.signed_up < pricing.visitors, "{pricing:?}");
    assert!(
        pricing.bounce_rate() > docs.bounce_rate(),
        "{pricing:?} {docs:?}"
    );
    // The block's totals are those of its rows.
    let sessions: i64 = v.pages.iter().map(|p| p.sessions).sum();
    let bounces: i64 = v.pages.iter().map(|p| p.bounces).sum();
    assert_eq!((v.sessions, v.bounces), (sessions, bounces), "{v:?}");
    assert!(v.bounces > 0 && v.bounces < v.sessions, "{v:?}");
    // Channels follow their landing page.
    let (search, social) = (row(&v.channels, "search"), row(&v.channels, "social"));
    assert!(search.signup_rate() > social.signup_rate(), "{v:?}");
    assert!(search.bounce_rate() > social.bounce_rate(), "{v:?}");
}

/// An internal tool: everyone logs in, some sessions start with page
/// views before `identify` but under the user's own id. No session is
/// anonymous, so the board shows no Visitors block.
#[tokio::test]
async fn an_internal_tool_where_everyone_is_identified_has_no_visitors_block() {
    let mut evs = Vec::new();
    for (k, who) in ["alice", "bob", "carol"].into_iter().enumerate() {
        let off = 10 * i64::try_from(k).unwrap();
        let s = format!("s-{who}");
        evs.push(ev(who, "$pageview", &s, 100 + off, "/login", "{}"));
        evs.push(ev(who, "$identify", &s, 99 + off, "/login", "{}"));
        evs.push(ev(who, "$pageview", &s, 98 + off, "/tasks", "{}"));
        // A second, single-page session: a bounce, but not a visitor's.
        evs.push(ev(
            who,
            "$pageview",
            &format!("{s}-2"),
            50 + off,
            "/tasks",
            "{}",
        ));
    }
    let (_dir, q) = store(&evs);
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    assert!(
        b.visitors.is_none(),
        "no anonymous session, no Visitors block: {:?}",
        b.visitors
    );
}

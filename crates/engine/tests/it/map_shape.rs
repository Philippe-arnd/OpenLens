//! Feature map, part 2 (#283), on seeded events: relations, split, merge
//! and step order suggestions, drift and the map version in force.

use super::*;

#[allow(clippy::too_many_arguments)]
fn ev(
    n: &mut usize,
    user: &str,
    session: &str,
    event: &str,
    path: &str,
    mins_ago: i64,
    release: &str,
) -> Event {
    *n += 1;
    let ts = Utc::now() - Duration::minutes(mins_ago);
    Event {
        uuid: format!("e{n}"),
        project: "p".into(),
        event: event.into(),
        distinct_id: user.into(),
        session_id: Some(session.into()),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some(path.into()),
        feature: None,
        role: Some("member".into()),
        account: None,
        plan: None,
        environment: "production".into(),
        release: Some(release.into()),
        flags: None,
        properties: "{}".into(),
    }
}

const MAP: &str = "version: 1\nfeatures:
  - key: billing
    name: Billing
    entry: {routes: ['/billing', '/billing/*']}
  - key: cart
    name: Cart
    steps: [cart_opened, item_added]
    success: item_added
    entry: {routes: ['/cart']}
  - key: checkout
    name: Checkout
    success: paid
    entry: {routes: ['/checkout']}
  - key: search
    name: Search
    entry: {routes: ['/search']}
";

/// 90 users over release v1 (two days ago) then v2 (today):
/// billing splits into invoices vs cards behind a shared landing page,
/// cart always goes with checkout and adds the item before opening the
/// cart, searching first makes paying likelier, and v2 brings `/reports`.
fn seeded() -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut n = 0;
    let mut rows = Vec::new();
    for i in 0..90 {
        let u = format!("u{i}");
        let (rel, base) = if i < 45 { ("v1", 60 * 48) } else { ("v2", 100) };
        let s = format!("s{i}");
        let side = if i % 2 == 0 {
            "/billing/invoices"
        } else {
            "/billing/cards"
        };
        rows.push(ev(&mut n, &u, &s, "$pageview", "/billing", base, rel));
        rows.push(ev(&mut n, &u, &s, "$pageview", side, base - 1, rel));
        let searched = i % 3 != 0;
        if searched {
            rows.push(ev(&mut n, &u, &s, "$pageview", "/search", base - 2, rel));
        }
        rows.push(ev(&mut n, &u, &s, "$pageview", "/cart", base - 3, rel));
        rows.push(ev(&mut n, &u, &s, "item_added", "/cart", base - 4, rel));
        rows.push(ev(&mut n, &u, &s, "cart_opened", "/cart", base - 5, rel));
        rows.push(ev(&mut n, &u, &s, "$pageview", "/checkout", base - 6, rel));
        // Searchers pay; most others do not.
        if searched || i % 9 == 0 {
            rows.push(ev(&mut n, &u, &s, "paid", "/checkout", base - 7, rel));
        }
        if rel == "v2" {
            rows.push(ev(
                &mut n,
                &u,
                &s,
                "$pageview",
                &format!("/reports/{i}"),
                base - 8,
                rel,
            ));
        }
    }
    w.append(&rows).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

#[tokio::test]
async fn sessions_suggest_splits_merges_order_and_drift() {
    let (_d, q) = seeded();
    let m = parse(MAP).map.unwrap();
    let h = openlens_engine::map_health(&q, "p", &m, &Filters::default())
        .await
        .unwrap();
    let kinds: Vec<(&str, Option<&str>)> = h
        .findings
        .iter()
        .map(|f| (f.kind, f.feature.as_deref()))
        .collect();
    for want in [
        ("too_big", Some("billing")),
        ("too_small", Some("checkout")),
        ("step_order", Some("cart")),
        ("drift", None),
    ] {
        assert!(kinds.contains(&want), "{want:?} missing from {kinds:?}");
    }
    let drift = h.findings.iter().find(|f| f.kind == "drift").unwrap();
    assert!(drift.message.contains("/reports/:id") && drift.message.contains("v2"));
    // The new route is drift, not also a coverage gap.
    assert!(
        !h.findings
            .iter()
            .any(|f| f.kind == "coverage" && f.message.contains("/reports"))
    );
    assert!(!kinds.contains(&("too_big", Some("cart"))), "{kinds:?}");

    // Relations: cart and checkout always together, cart first; searching
    // first leads to paying.
    let cc = h
        .relations
        .iter()
        .find(|r| r.first == "cart" && r.then == "checkout")
        .unwrap();
    assert!(cc.lift >= 1.0 && cc.first_share > 0.5, "{cc:?}");
    let sc = h
        .relations
        .iter()
        .find(|r| r.first == "search" && r.then == "checkout")
        .unwrap();
    assert!(sc.success_after > sc.success_without, "{sc:?}");
    assert!(sc.leads_to_value, "{sc:?}");
    // A release filter gives the relations of that release only.
    let v1 = Filters {
        release: Some("v1".into()),
        ..Filters::default()
    };
    let h1 = openlens_engine::map_health(&q, "p", &m, &v1).await.unwrap();
    let cc1 = h1
        .relations
        .iter()
        .find(|r| r.first == "cart" && r.then == "checkout")
        .unwrap();
    assert!(cc1.together < cc.together);
    assert!(
        !h1.findings
            .iter()
            .any(|f| f.kind == "drift" && f.message.contains("v1"))
    );
}

/// Drift reads the current release of the release states (D-049, #483
/// item 3): a canary next to a live release is not it, so its new route
/// waits until it takes over.
#[tokio::test]
async fn a_canary_is_not_the_release_drift_reads() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut n = 0;
    let mut rows = Vec::new();
    for i in 0..20 {
        let u = format!("u{i}");
        rows.push(ev(
            &mut n,
            &u,
            &format!("a{i}"),
            "$pageview",
            "/search",
            60 * 48,
            "v1",
        ));
        // Today, two users of twenty are on the canary, with a new route.
        let (rel, path) = if i < 2 {
            ("v2", format!("/reports/{i}"))
        } else {
            ("v1", "/cart".to_owned())
        };
        rows.push(ev(
            &mut n,
            &u,
            &format!("b{i}"),
            "$pageview",
            &path,
            100,
            rel,
        ));
    }
    w.append(&rows).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let m = parse(MAP).map.unwrap();
    let h = openlens_engine::map_health(&q, "p", &m, &Filters::default())
        .await
        .unwrap();
    assert!(
        !h.findings.iter().any(|f| f.kind == "drift"),
        "{:?}",
        h.findings
    );
}

#[tokio::test]
async fn each_release_keeps_the_map_version_it_shipped_with() {
    let (_d, q) = seeded();
    let old = parse(MAP).map.unwrap();
    // Version 2, pushed yesterday (after v1, before v2), moves `/search`
    // into cart.
    let new_yaml = MAP.replace("['/cart']", "['/cart', '/search']");
    let new = parse(&new_yaml).map.unwrap();
    let versions = vec![
        openlens_engine::MapAt {
            version: 1,
            saved: Utc::now() - Duration::days(5),
            map: old.clone(),
        },
        openlens_engine::MapAt {
            version: 2,
            saved: Utc::now() - Duration::days(1),
            map: new.clone(),
        },
    ];
    let history = openlens_engine::History::from(versions.clone());
    let ctx =
        openlens_engine::feature_context(&q, "p", &new, &history, &Filters::default(), "cart")
            .await
            .unwrap()
            .unwrap();
    let v1 = ctx.releases.iter().find(|r| r.release == "v1").unwrap();
    let v2 = ctx.releases.iter().find(|r| r.release == "v2").unwrap();
    assert_eq!((v1.map_version, v2.map_version), (Some(1), Some(2)));
    // Same users on both releases; only v2 counts the search pages as cart.
    assert!(v2.uses > v1.uses, "{v1:?} {v2:?}");
    // Counted with the current map for both, v1 would change.
    let now_only = openlens_engine::feature_context(
        &q,
        "p",
        &new,
        &Default::default(),
        &Filters::default(),
        "cart",
    )
    .await
    .unwrap()
    .unwrap();
    let v1_now = now_only
        .releases
        .iter()
        .find(|r| r.release == "v1")
        .unwrap();
    assert!(v1_now.uses > v1.uses);
    assert!(ctx.relations.iter().any(|r| r.then == "checkout"));

    // The versions found the first time are recorded (D-070) and win over
    // a later derivation: with version 1 gone from the stored versions read
    // (out of the newest 50), v1 would be counted with version 2.
    let rows = openlens_engine::inforce::to_record(&Filters::default(), &ctx.releases);
    assert!(rows.iter().any(|r| r.release == "v1" && r.map_version == 1));
    let later = openlens_engine::History {
        versions: versions[1..].to_vec(),
        recorded: rows,
        older: vec![(1, old.clone())],
        touched: Vec::new(),
    };
    let kept = openlens_engine::feature_context(&q, "p", &new, &later, &Filters::default(), "cart")
        .await
        .unwrap()
        .unwrap();
    let k1 = kept.releases.iter().find(|r| r.release == "v1").unwrap();
    assert_eq!((k1.map_version, k1.recorded), (Some(1), true));
    assert_eq!(k1.uses, v1.uses);
    let derived = openlens_engine::feature_context(
        &q,
        "p",
        &new,
        &versions[1..].to_vec().into(),
        &Filters::default(),
        "cart",
    )
    .await
    .unwrap()
    .unwrap();
    let d1 = derived.releases.iter().find(|r| r.release == "v1").unwrap();
    assert_eq!(d1.map_version, Some(2), "what the record prevents");
    // A role filter derives from part of the traffic: nothing is recorded.
    let staff = Filters {
        role: Some("member".into()),
        ..Filters::default()
    };
    assert!(openlens_engine::inforce::to_record(&staff, &ctx.releases).is_empty());
}

/// Closed days rolled up with the map in force give the relations and the
/// per-feature numbers of their events (#483), even once those events are
/// gone; a rollup made with another map version is not read.
#[tokio::test]
#[ignore = "heavy: rolls up every closed day; run by `cargo xtask ci --heavy` (nightly, release tags)"]
async fn rolled_up_days_give_the_numbers_of_their_events() {
    use openlens_storage::rollup;
    let (d, q) = seeded();
    let m = parse(MAP).map.unwrap();
    let f = Filters::default();
    let live = openlens_engine::map_health(&q, "p", &m, &f).await.unwrap();
    let features = openlens_engine::rollups::day_features(Some((&m, 1)));
    let days = rollup::missing(d.path(), "p", Utc::now()).unwrap();
    assert!(!days.is_empty(), "v1 is on a closed day");
    for day in &days {
        let label = day.format("%Y-%m-%d").to_string();
        let b = q.rollup_day("p", &label, None, &features).await.unwrap();
        assert!(rollup::write(d.path(), "p", *day, rollup::generation(), &b).unwrap());
        // Only the rollup is left of that day.
        let events = d.path().join(format!("events/project=p/date={label}"));
        std::fs::remove_dir_all(events).unwrap();
    }
    let q = QueryEngine::new(d.path(), Limits::default());
    let rolled = openlens_engine::map_health_in(&q, "p", &m, Some(1), &f)
        .await
        .unwrap();
    let numbers = |h: &openlens_engine::MapHealth| -> Vec<(String, i64, i64, i64)> {
        h.features
            .iter()
            .map(|x| (x.key.clone(), x.uses, x.users, x.success_users))
            .collect()
    };
    assert_eq!(numbers(&rolled), numbers(&live));
    assert!(!live.relations.is_empty());
    assert_eq!(rolled.relations, live.relations);
    let alone = openlens_engine::feature_relations(&q, "p", &m, Some(1), &f)
        .await
        .unwrap();
    assert_eq!(alone, live.relations);
    // Rolled with version 1, read for version 2: the events, now gone.
    let other = openlens_engine::feature_relations(&q, "p", &m, Some(2), &f)
        .await
        .unwrap();
    let together = |r: &[openlens_engine::Relation]| {
        r.iter()
            .find(|x| x.first == "cart" && x.then == "checkout")
            .map_or(0, |x| x.together)
    };
    assert!(together(&other) < together(&live.relations));
}

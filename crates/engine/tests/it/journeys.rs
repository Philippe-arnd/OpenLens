//! Paths to value (journeys): the common path into each success, listed
//! once per success event (#555).

use super::*;

/// Six users who go home, search, then write an entry.
fn journey_dataset() -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for u in 0..6 {
        let user = format!("j{u}");
        let steps: &[(&str, Option<&str>, &str)] = &[
            ("$pageview", None, "/home"),
            ("$pageview", None, "/home"),
            ("$click", Some("search"), "/search"),
            ("$pageview", None, "/entries/new"),
            ("entry_saved", None, "/entries/new"),
        ];
        for (i, (name, feat, path)) in steps.iter().enumerate() {
            let e = E {
                user: &user,
                role: "member",
                session: "s",
                event: name,
                mins_ago: 50 - i as i64,
                feature: *feat,
                release: "v1",
                props: "{}",
            };
            let mut ev = mk(&e, u * 10 + i);
            ev.path = Some((*path).into());
            evs.push(ev);
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

#[tokio::test]
async fn journeys_show_the_common_path_into_a_success() {
    let (_d, q) = journey_dataset();
    let map = parse("features:\n  - key: write\n    name: Write\n    success: entry_saved\n    entry: { routes: ['/entries/new'] }\n").map.unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    let j = b.journeys.first().expect("a journey");
    assert_eq!(j.success, "entry_saved");
    // Places are features, in the order of their first use in the
    // session; a page without a feature is not one (D-059).
    assert_eq!(
        j.path,
        ["search", "write"],
        "repeats collapse; route resolves to the feature"
    );
    assert_eq!(j.users, 6);
    assert!((j.share - 1.0).abs() < 1e-9);
}

#[tokio::test]
async fn features_sharing_a_success_event_list_each_path_once() {
    let (_d, q) = journey_dataset();
    let map = parse(
        "features:\n  - key: write\n    name: Write\n    success: entry_saved\n    entry: { routes: ['/entries/new'] }\n  - key: draft\n    name: Draft\n    success: entry_saved\n    entry: { routes: ['/drafts'] }\n",
    )
    .map
    .unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    assert!(!b.journeys.is_empty());
    let mut keys: Vec<_> = b.journeys.iter().map(|j| (&j.success, &j.path)).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), b.journeys.len(), "{:?}", b.journeys);
    assert!(
        b.journeys
            .iter()
            .all(|j| j.feature.contains("write") && j.feature.contains("draft"))
    );
}

#[tokio::test]
async fn journeys_are_reused_until_they_are_an_hour_old() {
    let (d, q) = journey_dataset();
    let map = parse("features:\n  - key: write\n    name: Write\n    success: entry_saved\n    entry: { routes: ['/entries/new'] }\n").map.unwrap();
    let f = Filters::default();
    let total = |b: &openlens_engine::Board| b.journeys.iter().map(|j| j.users).sum::<i64>();
    let first = board(&q, "p", &f, Some((&map, 1))).await.unwrap();
    assert!(total(&first) > 0);
    // Three more users take the same path: the next board reuses the
    // journeys computed a moment ago (#730) instead of four slow queries.
    let (mut w, _) = Writer::open(d.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for u in 6..9 {
        let user = format!("j{u}");
        for (i, (name, feat, path)) in [
            ("$click", Some("search"), "/search"),
            ("entry_saved", None, "/entries/new"),
        ]
        .iter()
        .enumerate()
        {
            let e = E {
                user: &user,
                role: "member",
                session: "s",
                event: name,
                mins_ago: 40 - i as i64,
                feature: *feat,
                release: "v1",
                props: "{}",
            };
            let mut ev = mk(&e, 1000 + u * 10 + i);
            ev.path = Some((*path).into());
            evs.push(ev);
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let again = board(&q, "p", &f, Some((&map, 1))).await.unwrap();
    assert!(
        again.users.dau > first.users.dau,
        "the new users are counted"
    );
    assert_eq!(
        total(&again),
        total(&first),
        "the journeys are the ones kept"
    );
    // Other filters are another key: computed now, with the new users.
    let with_team = Filters {
        environment: "production".into(),
        internal: true,
        ..Filters::default()
    };
    let other = board(&q, "p", &with_team, Some((&map, 1))).await.unwrap();
    assert!(total(&other) > total(&first));
}

/// Two success events, read in one query (#748): each keeps its own
/// paths and its own count of people who reached it.
#[tokio::test]
async fn each_success_event_keeps_its_own_paths_in_one_query() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for u in 0..6 {
        let user = format!("k{u}");
        let steps: &[(&str, Option<&str>, &str)] = &[
            ("$click", Some("search"), "/search"),
            ("found", None, "/search"),
            ("$pageview", None, "/entries/new"),
            ("entry_saved", None, "/entries/new"),
        ];
        for (i, (name, feat, path)) in steps.iter().enumerate() {
            let e = E {
                user: &user,
                role: "member",
                session: "s",
                event: name,
                mins_ago: 50 - i as i64,
                feature: *feat,
                release: "v1",
                props: "{}",
            };
            let mut ev = mk(&e, u * 10 + i);
            ev.path = Some((*path).into());
            evs.push(ev);
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    let map = parse(
        "features:\n  - key: search\n    name: Search\n    success: found\n    entry: { routes: ['/search'] }\n\
         \x20 - key: write\n    name: Write\n    success: entry_saved\n    entry: { routes: ['/entries/new'] }\n",
    )
    .map
    .unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    let of = |ev: &str| {
        b.journeys
            .iter()
            .find(|j| j.success == ev)
            .unwrap_or_else(|| panic!("a path into {ev}: {:?}", b.journeys))
    };
    let (found, saved) = (of("found"), of("entry_saved"));
    assert_eq!(found.feature, "search");
    assert_eq!(saved.feature, "write");
    assert!(
        saved.path.len() > found.path.len(),
        "writing comes after searching: {:?} {:?}",
        found.path,
        saved.path
    );
    assert!(saved.path.contains(&"search".to_owned()));
    assert!((found.share - 1.0).abs() < 1e-9 && (saved.share - 1.0).abs() < 1e-9);
}

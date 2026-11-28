#![allow(clippy::unwrap_used, clippy::field_reassign_with_default)]
use super::*;

const MAP: &str = "features:
  - key: docs
    name: Docs
    area: Reading
    success: doc_read
    entry: {routes: ['/docs/*']}
  - key: edit
    name: Edit
    area: Writing
    success: doc_saved
    entry: {routes: ['/docs/:id/edit']}
  - key: share
    name: Share
    area: Writing
    success: shared
    requires: [edit]
    entry: {routes: ['/share']}
  - key: old
    name: Old thing
    success: old_done
    entry: {routes: ['/old']}
";

fn usage() -> Usage {
    let mut u = Usage::default();
    u.by_feature = vec![
        (Some("docs".into()), true, 500, 40),
        (Some("edit".into()), true, 300, 30),
        (Some("share".into()), true, 100, 25),
        (Some("old".into()), false, 12, 3),
        (None, true, 200, 20),
    ];
    u.success = vec![("docs".into(), true, 40), ("share".into(), true, 10)];
    u.paths = vec![
        ("/docs/1".into(), 400, true),
        ("/docs/1/edit".into(), 250, true),
        ("/billing/42".into(), 150, false),
        ("/billing/43".into(), 50, false),
        ("/share".into(), 150, true),
    ];
    u.tagged = vec![("export".into(), 30), ("edit".into(), 10)];
    for i in 0..25 {
        u.pairs.push((format!("u{i}"), "share".into()));
        if i < 5 {
            u.pairs.push((format!("u{i}"), "edit".into()));
        }
    }
    u
}

#[test]
fn planted_defects_are_all_found_with_fixes() {
    let m = openlens_featuremap::parse(MAP).map.unwrap();
    let h = assess(&m, &usage());
    let kinds: Vec<(&str, Option<&str>)> = h
        .findings
        .iter()
        .map(|f| (f.kind, f.feature.as_deref()))
        .collect();
    for want in [
        ("coverage", None),
        ("overlap", Some("edit")),
        ("dead", Some("old")),
        ("success_never", Some("edit")),
        ("success_always", Some("docs")),
        ("undeclared", Some("export")),
        ("requires", Some("share")),
    ] {
        assert!(kinds.contains(&want), "{want:?} missing from {kinds:?}");
    }
    assert!(h.findings.iter().all(|f| f.fix.is_some()));
    // `/billing/:id` is 20% of all: an error, ranked first.
    assert_eq!(h.findings[0].kind, "coverage");
    assert!(
        h.findings[0]
            .fix
            .as_deref()
            .unwrap()
            .contains("/billing/:id")
    );
    assert!((h.coverage - 0.8).abs() < 1e-9);
    assert!(h.score < 60, "{}", h.score);
    let share = &h.ladder[0];
    assert_eq!((share.from_users, share.reached, share.skipped), (5, 5, 20));
    let writing = h.areas.iter().find(|a| a.area == "Writing").unwrap();
    assert_eq!(
        (writing.features, writing.uses, writing.users),
        (2, 400, 25)
    );
    let old = h.features.iter().find(|f| f.key == "old").unwrap();
    assert_eq!((old.status, old.prev_uses), ("dead", 12));
}

#[test]
fn a_clean_map_scores_100() {
    let m = openlens_featuremap::parse(
        "features:\n  - key: a\n    name: A\n    success: done\n    entry: {routes: ['/a']}\n",
    )
    .map
    .unwrap();
    let mut u = Usage::default();
    u.by_feature = vec![(Some("a".into()), true, 100, 30)];
    u.success = vec![("a".into(), true, 12)];
    u.paths = vec![("/a".into(), 100, true)];
    let h = assess(&m, &u);
    assert_eq!(h.score, 100, "{:?}", h.findings);
    assert!(h.areas.is_empty());
    assert_eq!(h.features[0].success_rate(), Some(0.4));
}

#[test]
fn routes_and_keys_are_suggested_from_paths() {
    assert_eq!(route_for("/billing/42/invoices/"), "/billing/:id/invoices");
    assert_eq!(
        route_for("/d/0f8fad5b-d9cb-469f-a165-70867728950e"),
        "/d/:id"
    );
    assert_eq!(route_for("/"), "/");
    assert_eq!(key_for("/billing/:id/invoices"), "billing-invoices");
    assert_eq!(key_for("/"), "home");
}

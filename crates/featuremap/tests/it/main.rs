//! Integration tests for `openlens-featuremap`.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use openlens_featuremap::{parse, route_regex, to_yaml};

const EXAMPLE: &str = include_str!("../../../../docs/schema/features.example.yaml");

#[test]
fn the_documented_example_is_valid() {
    let p = parse(EXAMPLE);
    assert!(p.map.is_some(), "{:?}", p.issues);
    assert!(p.issues.iter().all(|i| i.level != "error"));
}

/// `set_feature` (#524) rewrites the stored map from its parsed form: what
/// it writes must read back as the same map.
#[test]
fn a_written_map_reads_back_the_same() {
    let m = parse(EXAMPLE).map.unwrap();
    let again = parse(&to_yaml(&m).unwrap());
    assert_eq!(again.map.as_ref(), Some(&m), "{:?}", again.issues);
}

#[test]
fn roles_resolve_first_match_wins_case_insensitive_with_wildcards() {
    let m = parse("roles:\n  - key: admin\n    match: [Owner, superadmin]\n  - key: member\n    match: ['team_*', user]\n")
        .map
        .unwrap();
    assert_eq!(m.resolve_role("OWNER"), Some("admin"));
    assert_eq!(m.resolve_role("team_paris"), Some("member"));
    assert_eq!(m.resolve_role("guest"), None);
}

#[test]
fn routes_match_params_and_wildcards_only_where_declared() {
    let m = parse(
        "features:\n  - key: edit\n    name: Edit\n    entry: { routes: ['/entries/:id/edit'] }\n  - key: docs\n    name: Docs\n    entry: { routes: ['/docs/*'] }\n",
    )
    .map
    .unwrap();
    assert_eq!(m.feature_for_path("/entries/42/edit"), Some("edit"));
    assert_eq!(m.feature_for_path("/entries/42"), None);
    assert_eq!(m.feature_for_path("/docs/a/b"), Some("docs"));
    assert_eq!(route_regex("/entries/:id/edit"), "^/entries/[^/]+/edit/?$");
}

#[test]
fn errors_name_the_path_and_how_to_fix_it() {
    let p = parse("features:\n  - key: Bad Key\n    name: ''\n  - key: Bad Key\n    name: x\n");
    assert!(p.map.is_none());
    let paths: Vec<_> = p
        .issues
        .iter()
        .filter(|i| i.level == "error")
        .map(|i| i.path.as_str())
        .collect();
    assert!(paths.contains(&"features[0].key"));
    assert!(paths.contains(&"features[0].name"));
    assert!(paths.contains(&"features[1].key"), "duplicate key");
    assert!(p.issues.iter().any(|i| i.message.contains("lowercase")));
}

#[test]
fn missing_success_and_roles_are_warnings_not_errors() {
    let p =
        parse("features:\n  - key: search\n    name: Search\n    entry: { routes: ['/search'] }\n");
    assert!(p.map.is_some());
    assert!(p.issues.iter().any(|i| i.path == "roles"));
    assert!(p.issues.iter().any(|i| i.path == "features[0].success"));
}

#[test]
fn unknown_fields_and_garbage_are_refused_without_panicking() {
    assert!(parse("featurez: []").map.is_none());
    for junk in [
        "",
        "{",
        "- - -",
        "roles: 3",
        "\u{0}",
        &"a: [".repeat(10_000),
    ] {
        let _ = parse(junk);
    }
    assert!(parse(&"x".repeat(300 * 1024)).map.is_none());
}

#[test]
fn sql_expressions_quote_hostile_values() {
    let m = parse("roles:\n  - key: admin\n    match: [\"o'brien\"]\nfeatures:\n  - key: a\n    name: A\n    entry: { routes: ['/x'] }\n")
        .map
        .unwrap();
    assert!(m.role_sql().contains("'o''brien'"));
    assert!(m.feature_sql().contains("regexp_like(path, '^/x/?$')"));
}

#[test]
fn area_requires_and_renamed_from_are_validated() {
    let ok = parse(
        "features:\n  - key: write\n    name: Write\n    area: Writing\n    success: saved\n    renamed_from: [editor]\n    entry: {routes: [/write]}\n  - key: share\n    name: Share\n    area: Writing\n    requires: [write]\n    success: shared\n    entry: {routes: [/share/*]}\n",
    );
    assert!(ok.map.is_some(), "{:?}", ok.issues);
    let m = ok.map.unwrap_or_default();
    assert_eq!(m.current_key("editor"), Some("write"));
    assert_eq!(m.current_key("share"), Some("share"));
    assert_eq!(m.current_key("nope"), None);
    assert!(
        m.feature_sql()
            .starts_with("CASE WHEN feature IN ('editor') THEN 'write'")
    );
    let bad = |src: &str, path: &str| {
        let p = parse(src);
        assert!(p.map.is_none(), "{src}");
        assert!(
            p.issues.iter().any(|i| i.path.starts_with(path)),
            "{path}: {:?}",
            p.issues
        );
    };
    let base = "  - key: a\n    name: A\n    success: x\n";
    bad(
        &format!("features:\n{base}    requires: [ghost]\n"),
        "features[0].requires[0]",
    );
    bad(
        &format!("features:\n{base}    requires: [a]\n"),
        "features[0].requires[0]",
    );
    bad(
        &format!(
            "features:\n{base}    requires: [b]\n  - key: b\n    name: B\n    requires: [c]\n  - key: c\n    name: C\n    requires: [a]\n"
        ),
        "features[",
    );
    bad(
        &format!("features:\n{base}    area: \"\"\n"),
        "features[0].area",
    );
    bad(
        &format!("features:\n{base}  - key: b\n    name: B\n    renamed_from: [a]\n"),
        "features[1].renamed_from[0]",
    );
    // A chain without a loop is fine.
    let chain = parse(&format!(
        "features:\n{base}  - key: b\n    name: B\n    requires: [a]\n  - key: c\n    name: C\n    requires: [a, b]\n"
    ));
    assert!(chain.map.is_some(), "{:?}", chain.issues);
}

#[test]
fn overlapping_routes_are_all_listed() {
    let m = parse(
        "features:\n  - key: docs\n    name: Docs\n    entry: {routes: [/docs/*]}\n  - key: edit\n    name: Edit\n    entry: {routes: [/docs/:id/edit]}\n",
    )
    .map
    .unwrap_or_default();
    assert_eq!(m.features_for_path("/docs/7/edit"), vec!["docs", "edit"]);
    assert_eq!(m.features_for_path("/docs/7"), vec!["docs"]);
    assert!(m.features_for_path("/other").is_empty());
}

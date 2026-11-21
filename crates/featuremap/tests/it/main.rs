//! Integration tests for `openlens-featuremap`.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use openlens_featuremap::{parse, route_regex};

const EXAMPLE: &str = include_str!("../../../../docs/schema/features.example.yaml");

#[test]
fn the_documented_example_is_valid() {
    let p = parse(EXAMPLE);
    assert!(p.map.is_some(), "{:?}", p.issues);
    assert!(p.issues.iter().all(|i| i.level != "error"));
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

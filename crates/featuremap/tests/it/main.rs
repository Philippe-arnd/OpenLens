//! Integration tests for `openlens-featuremap`.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use openlens_featuremap::{
    MAX_CHANGED_FILES, MAX_PATHS, code_path_matches, parse, route_regex, to_yaml, touched_features,
};

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

/// Plans (#712, D-103): matched like roles, ordered lowest first, and paid
/// only when the map says so.
#[test]
fn plans_tell_upgrades_from_downgrades_and_never_guess_paid() {
    let m = parse(
        "plans:\n  - key: free\n    match: [free, Trial]\n    paid: false\n  - key: pro\n    match: ['pro*']\n  - key: team\n    match: [team]\n",
    )
    .map
    .unwrap();
    assert_eq!(m.canonical_plan("PRO_yearly"), "pro");
    assert!(m.canonical_plan("enterprise").starts_with("unmatched:"));
    assert_eq!(m.plan_move("free", "pro"), Some(true));
    assert_eq!(m.plan_move("team", "pro_monthly"), Some(false));
    assert_eq!(m.plan_move("pro", "pro_yearly"), None, "same plan");
    assert_eq!(m.plan_move("pro", "enterprise"), None, "unknown");
    assert!(m.knows_paid() && m.plan_paid_sql().is_some());
    // A plan named `free` without `paid: false` is paid: no guessing.
    let named = parse("plans:\n  - key: free\n    match: [free]\n  - key: pro\n    match: [pro]\n")
        .map
        .unwrap();
    assert!(!named.knows_paid() && named.plan_paid_sql().is_none());
    // A free plan listed after a paid one is out of order.
    let late = parse(
        "plans:\n  - key: pro\n    match: [pro]\n  - key: free\n    match: [free]\n    paid: false\n",
    );
    assert!(late.issues.iter().any(|i| i.path == "plans[1].paid"));
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

#[test]
fn code_path_globs_match_within_and_across_folders() {
    assert!(code_path_matches("src/export/**", "src/export/pdf.rs"));
    assert!(code_path_matches("src/export/**", "src/export/a/b/c.rs"));
    assert!(!code_path_matches("src/export/**", "src/exporter/pdf.rs"));
    // `*` stays inside one folder, `**` crosses any number (zero included).
    assert!(code_path_matches("src/*.rs", "src/main.rs"));
    assert!(!code_path_matches("src/*.rs", "src/a/main.rs"));
    assert!(code_path_matches("src/**/mod.rs", "src/mod.rs"));
    assert!(code_path_matches("src/**/mod.rs", "src/a/b/mod.rs"));
    assert!(code_path_matches("**/*.test.ts", "web/x/y.test.ts"));
    assert!(code_path_matches("app/v?/page.tsx", "app/v2/page.tsx"));
    assert!(!code_path_matches("app/v?/page.tsx", "app/v10/page.tsx"));
    // Case-sensitive, like git paths.
    assert!(!code_path_matches("src/Export/**", "src/export/pdf.rs"));
}

#[test]
fn hostile_code_paths_are_refused_with_the_feature_named() {
    let base = "features:\n  - key: export\n    name: Export\n    success: x\n    paths: ";
    for bad in [
        "['/src/**']",
        "['src/../secrets']",
        "['./src']",
        "['src//x']",
        "['src/']",
        "['']",
        "['src/a**']",
        "['src/{a,b}']",
        "['src/[ab]']",
        "['!src']",
        "['C:/x']",
        "['src\\\\x']",
        "[\"src/\\ta\"]",
    ] {
        let p = parse(&format!("{base}{bad}\n"));
        assert!(p.map.is_none(), "{bad} accepted");
        assert!(
            p.issues.iter().any(|i| i.level == "error"
                && i.path == "features[0].paths[0]"
                && i.message.contains("`export`")),
            "{bad}: {:?}",
            p.issues
        );
    }
    let long = format!("{base}['{}']\n", "a/".repeat(150));
    assert!(parse(&long).map.is_none(), "an overlong glob is refused");
    let many: Vec<String> = (0..=MAX_PATHS).map(|i| format!("'src/f{i}/**'")).collect();
    let p = parse(&format!("{base}[{}]\n", many.join(", ")));
    assert!(p.issues.iter().any(|i| i.path == "features[0].paths"));
    let ok = parse(&format!("{base}['src/export/**', 'lib/pdf?.rs']\n"));
    assert!(ok.map.is_some(), "{:?}", ok.issues);
}

#[test]
fn changed_files_name_the_features_touched_and_count_the_rest() {
    let m = parse(
        "features:\n  - key: export\n    name: Export\n    paths: [src/export/**]\n  - key: pdf\n    name: PDF\n    paths: ['src/export/pdf*.rs', src/pdf/**]\n  - key: search\n    name: Search\n    paths: [src/search/**]\n  - key: nopaths\n    name: No paths\n",
    )
    .map
    .unwrap();
    let t = touched_features(
        &m,
        &[
            "src/export/csv.rs",
            "src/export/pdf_page.rs",
            "./src/pdf/render.rs",
            "README.md",
            "docs/x.md",
        ],
    );
    let keys: Vec<&str> = t.features.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, ["export", "pdf"], "map order, untouched left out");
    assert_eq!(t.unmatched, 2);
    let matched = 5 - t.unmatched;
    assert!(
        t.features.iter().map(|f| f.files).sum::<usize>() > matched,
        "one file can touch several features: {t:?}"
    );
    assert_eq!(t.skipped, 0);
    assert!(t.features_with_paths < m.features.len());
}

#[test]
fn the_matcher_bounds_hostile_inputs() {
    // Star-heavy globs against long paths: the matcher backtracks to the
    // last star only, so this returns at once instead of exploding.
    let glob = "**/a*a*a*a*a*a*a*a*a*b/**/**/**/**/c";
    let file = format!("{}/x", "a".repeat(1000));
    assert!(!code_path_matches(glob, &file));
    let deep = format!("{}c/d", "a/".repeat(500));
    assert!(!code_path_matches(glob, &deep));
    // Too many and too long changed files are counted, not examined.
    let m = parse("features:\n  - key: all\n    name: All\n    paths: ['**']\n")
        .map
        .unwrap();
    let mut files: Vec<String> = vec!["x".repeat(5000), String::new()];
    files.extend((0..MAX_CHANGED_FILES + 10).map(|i| format!("f{i}")));
    let t = touched_features(&m, &files);
    assert!(t.skipped >= 12, "{t:?}");
    assert_eq!(t.skipped + t.features[0].files + t.unmatched, files.len());
    // Unvalidated globs match nothing instead of misbehaving.
    let mut bad = m.clone();
    bad.features[0].paths = vec!["../**".into(), "/**".into()];
    assert!(touched_features(&bad, &["a/b"]).features.is_empty());
}

//! Integration tests for `openlens-meta`.

// Test helpers may unwrap: a failure here is a test failure (clippy.toml
// already allows it inside #[test] functions; this extends it to helpers).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Utc};
use openlens_meta::{Meta, slug};

#[test]
fn capture_keys_resolve_to_their_project_and_unknown_keys_do_not() {
    let m = Meta::in_memory().unwrap();
    let a = m.create_project("Journal App", None).unwrap();
    let b = m.create_project("Kanban", None).unwrap();
    assert_ne!(a.api_key, b.api_key);
    assert_eq!(
        m.project_by_key(&a.api_key).unwrap().unwrap().id,
        "journal-app"
    );
    assert!(m.project_by_key("pk_nope").unwrap().is_none());
}

#[test]
fn default_project_is_created_once() {
    let m = Meta::in_memory().unwrap();
    let first = m.ensure_default_project(Some("pk_fixed")).unwrap();
    let again = m.ensure_default_project(None).unwrap();
    assert_eq!(first, again);
    assert_eq!(first.api_key, "pk_fixed");
}

#[test]
fn role_changes_are_recorded_with_dates_and_repeats_are_not() {
    let m = Meta::in_memory().unwrap();
    let t0 = Utc::now();
    m.identify("p", "u1", Some("member"), None, None, t0)
        .unwrap();
    m.identify(
        "p",
        "u1",
        Some("member"),
        Some("acme"),
        None,
        t0 + Duration::days(1),
    )
    .unwrap();
    m.identify("p", "u1", None, None, Some("pro"), t0 + Duration::days(2))
        .unwrap();
    m.identify("p", "u1", Some("admin"), None, None, t0 + Duration::days(3))
        .unwrap();

    let hist = m.role_history("p", "u1").unwrap();
    let roles: Vec<_> = hist.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(roles, ["member", "admin"]);
    assert!(hist[0].1 < hist[1].1);

    let person = m.person("p", "u1").unwrap().unwrap();
    assert_eq!(person.role.as_deref(), Some("admin"));
    assert_eq!(
        person.account.as_deref(),
        Some("acme"),
        "a missing field keeps the old value"
    );
    assert_eq!(person.plan.as_deref(), Some("pro"));
}

#[test]
fn admin_token_is_stable_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.db");
    let t1 = Meta::open(&path).unwrap().admin_token().unwrap();
    let t2 = Meta::open(&path).unwrap().admin_token().unwrap();
    assert_eq!(t1, t2);
    assert!(t1.len() > 20);
}

#[test]
fn slugs_are_safe_partition_names() {
    for name in ["Hello World!", "../../etc", "ÉtÉ 2026", "a//b"] {
        let s = slug(name);
        assert!(
            s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{s}"
        );
    }
    assert!(slug("!!!").is_empty());
}

#[test]
fn login_links_work_once_and_expire() {
    let m = Meta::in_memory().unwrap();
    let t = m.create_login_link(600).unwrap();
    assert!(m.consume_login_link(&t).unwrap());
    assert!(!m.consume_login_link(&t).unwrap(), "second use must fail");
    let expired = m.create_login_link(-1).unwrap();
    assert!(!m.consume_login_link(&expired).unwrap());
    assert!(!m.consume_login_link("ll_made_up").unwrap());
}

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
fn admin_token_is_stable_across_reopen_and_never_stored_in_clear() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.db");
    let (h1, plain) = Meta::open(&path).unwrap().admin_token_hash().unwrap();
    let plain = plain.expect("shown once, when generated");
    let meta = Meta::open(&path).unwrap();
    let (h2, again) = meta.admin_token_hash().unwrap();
    assert_eq!(h1, h2);
    assert!(again.is_none() && plain.len() > 20);
    let stored = meta.setting("admin_token").unwrap().unwrap();
    assert!(!stored.contains(&plain));
    // A token stored in clear by an older release is hashed in place.
    meta.set_setting("admin_token", "adm_old").unwrap();
    let (h3, _) = meta.admin_token_hash().unwrap();
    assert_eq!(h3, openlens_meta::sha256_hex("adm_old"));
    assert!(
        !meta
            .setting("admin_token")
            .unwrap()
            .unwrap()
            .contains("adm_old")
    );
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
    assert_eq!(m.consume_login_link(&t).unwrap(), Some(None), "owner link");
    assert_eq!(
        m.consume_login_link(&t).unwrap(),
        None,
        "second use must fail"
    );
    let expired = m.create_login_link(-1).unwrap();
    assert_eq!(m.consume_login_link(&expired).unwrap(), None);
    assert_eq!(m.consume_login_link("ll_made_up").unwrap(), None);
}

#[test]
fn users_are_validated_updated_and_bound_to_their_links() {
    let m = Meta::in_memory().unwrap();
    let u = m
        .upsert_user(
            " Ana@Example.com ",
            "viewer",
            Some(&["a".into(), " b ".into()]),
        )
        .unwrap();
    assert_eq!(u.email, "ana@example.com");
    assert_eq!(u.projects, Some(vec!["a".to_owned(), "b".to_owned()]));
    let again = m.upsert_user("ana@example.com", "admin", None).unwrap();
    assert_eq!(
        (
            again.id.as_str(),
            again.role.as_str(),
            again.projects.clone()
        ),
        (u.id.as_str(), "admin", None)
    );
    assert!(m.upsert_user("not-an-email", "viewer", None).is_err());
    assert!(m.upsert_user("x@y.z", "root", None).is_err());
    let t = m.create_login_link_for(Some(&u.id), 600).unwrap();
    assert_eq!(m.consume_login_link(&t).unwrap(), Some(Some(u.id.clone())));
    m.upsert_user("other@example.com", "admin", None).unwrap();
    assert!(m.delete_user(&u.id).unwrap());
    assert!(m.user(&u.id).unwrap().is_none());
}

#[test]
fn feature_maps_are_versioned_and_identical_pushes_are_no_ops() {
    let m = Meta::in_memory().unwrap();
    assert!(m.feature_map("p").unwrap().is_none());
    assert_eq!(m.save_feature_map("p", "a").unwrap(), 1);
    assert_eq!(m.save_feature_map("p", "a").unwrap(), 1);
    assert_eq!(m.save_feature_map("p", "b").unwrap(), 2);
    assert_eq!(m.feature_map("p").unwrap().unwrap(), (2, "b".to_owned()));
}

#[test]
fn projects_rename_rotate_and_drop_their_settings() {
    let m = Meta::in_memory().unwrap();
    let p = m.create_project("Journal", None).unwrap();
    assert!(m.rename_project(&p.id, " Journal app ").unwrap());
    assert!(m.rename_project(&p.id, "  ").is_err());
    assert_eq!(m.project(&p.id).unwrap().unwrap().name, "Journal app");
    let k = m.rotate_project_key(&p.id).unwrap().unwrap();
    assert_ne!(k, p.api_key);
    assert!(m.project_by_key(&p.api_key).unwrap().is_none());
    assert_eq!(m.project_by_key(&k).unwrap().unwrap().id, p.id);
    assert!(m.rotate_project_key("nope").unwrap().is_none());
    m.set_project_setting(&p.id, "app_url", "https://x.example")
        .unwrap();
    assert_eq!(
        m.project_setting(&p.id, "app_url").unwrap().as_deref(),
        Some("https://x.example")
    );
    m.set_project_setting(&p.id, "app_url", "").unwrap();
    assert!(m.project_setting(&p.id, "app_url").unwrap().is_none());
    m.set_project_setting(&p.id, "icon", "x").unwrap();
    assert!(m.delete_project(&p.id).unwrap());
    assert!(m.project_setting(&p.id, "icon").unwrap().is_none());
}

#[test]
fn users_update_disable_and_keep_one_admin() {
    let m = Meta::in_memory().unwrap();
    let a = m.upsert_user("a@example.com", "admin", None).unwrap();
    let v = m.upsert_user("v@example.com", "viewer", None).unwrap();
    // The only admin can't be demoted, disabled or removed.
    assert!(
        m.update_user(&a.id, None, &a.email, "viewer", None)
            .is_err()
    );
    assert!(m.set_user_disabled(&a.id, true).is_err());
    assert!(m.delete_user(&a.id).is_err());
    // A viewer can be anything; email changes keep the id.
    let u = m
        .update_user(
            &v.id,
            Some(" Vera "),
            "vera@example.com",
            "admin",
            Some(&["p".into()]),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        (u.id.as_str(), u.name.as_deref(), u.email.as_str()),
        (v.id.as_str(), Some("Vera"), "vera@example.com")
    );
    assert!(
        m.update_user(&v.id, None, "a@example.com", "admin", None)
            .is_err()
    );
    // Two admins: one may step down.
    assert!(
        m.update_user(&a.id, None, &a.email, "viewer", None)
            .unwrap()
            .is_some()
    );
    m.create_login_link_for(Some(&v.id), 600).unwrap();
    assert!(m.pending_link(&v.id).unwrap().is_some());
    m.set_user_disabled(&a.id, true).unwrap();
    assert!(m.user(&a.id).unwrap().unwrap().disabled);
    m.set_user_timezone(&a.id, Some("Europe/Paris")).unwrap();
    assert!(m.set_user_timezone(&a.id, Some("x y")).is_err());
    assert_eq!(
        m.user(&a.id).unwrap().unwrap().timezone.as_deref(),
        Some("Europe/Paris")
    );
    m.touch_sign_in(&v.id).unwrap();
    assert!(m.user(&v.id).unwrap().unwrap().last_sign_in.is_some());
    assert_eq!(m.revoke_links(&v.id).unwrap(), 1);
}

#[test]
fn replay_comments_are_pinned_and_owned() {
    let m = Meta::in_memory().unwrap();
    let p = m.create_project("App", None).unwrap();
    let c = m
        .add_replay_comment(&p.id, "s1", 12_400, "u1", "ana", " form error here ")
        .unwrap();
    m.add_replay_comment(&p.id, "s1", 2_000, "u2", "bo", "first")
        .unwrap();
    assert!(
        m.add_replay_comment(&p.id, "s1", 1, "u1", "ana", "  ")
            .is_err()
    );
    assert!(
        m.add_replay_comment(&p.id, "s1", 1, "u1", "ana", &"x".repeat(2001))
            .is_err()
    );
    let l = m.replay_comments(&p.id, "s1").unwrap();
    assert_eq!(
        l.iter().map(|c| c.at_ms).collect::<Vec<_>>(),
        vec![2_000, 12_400]
    );
    assert_eq!(l[1].body, "form error here");
    assert_eq!(
        m.replay_comment_counts(&p.id).unwrap(),
        vec![("s1".to_owned(), 2)]
    );
    assert!(!m.delete_replay_comment(&p.id, c.id, "u2", false).unwrap());
    assert!(m.delete_replay_comment(&p.id, c.id, "u1", false).unwrap());
    assert!(m.delete_project(&p.id).unwrap());
    assert!(m.replay_comments(&p.id, "s1").unwrap().is_empty());
}

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

/// SEC-07: a user's first OIDC sign-in binds (issuer, subject); another
/// provider account, even one that shares the e-mail, is then refused.
#[test]
fn oidc_subject_is_bound_on_first_sign_in_and_checked_after() {
    let m = Meta::in_memory().unwrap();
    let u = m.upsert_user("ana@example.com", "viewer", None).unwrap();
    assert_eq!(u.oidc_iss, None);
    assert!(
        m.bind_oidc_subject(&u.id, "https://idp.example", "sub-1")
            .unwrap()
    );
    let bound = m.user(&u.id).unwrap().unwrap();
    assert_eq!(bound.oidc_iss.as_deref(), Some("https://idp.example"));
    assert_eq!(bound.oidc_sub.as_deref(), Some("sub-1"));
    // The same (issuer, subject) is accepted again...
    assert!(
        m.bind_oidc_subject(&u.id, "https://idp.example", "sub-1")
            .unwrap()
    );
    // ...a different subject at the same provider is not...
    assert!(
        !m.bind_oidc_subject(&u.id, "https://idp.example", "sub-2")
            .unwrap()
    );
    // ...and neither is the same subject at a different provider.
    assert!(
        !m.bind_oidc_subject(&u.id, "https://other.example", "sub-1")
            .unwrap()
    );
    assert!(
        !m.bind_oidc_subject("no-such-user", "https://idp.example", "sub-1")
            .unwrap()
    );
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

#[test]
fn a_person_forgotten_after_a_lookup_is_seen_at_once() {
    let meta = Meta::in_memory().unwrap();
    assert!(!meta.is_forgotten("p", "u1").unwrap());
    meta.forget("p", &["u1".to_owned()]).unwrap();
    assert!(meta.is_forgotten("p", "u1").unwrap());
    assert!(!meta.is_forgotten("p", "u2").unwrap());
    assert!(!meta.is_forgotten("other", "u1").unwrap());
}

#[test]
fn reads_on_the_reader_connections_see_each_write_at_once() {
    // A file store has reader connections (#344); a write must be visible
    // to the next read, whichever connection serves it.
    let dir = tempfile::tempdir().unwrap();
    let meta = std::sync::Arc::new(Meta::open(&dir.path().join("meta.db")).unwrap());
    for i in 0..20 {
        meta.set_setting("k", &i.to_string()).unwrap();
        assert_eq!(
            meta.setting("k").unwrap().as_deref(),
            Some(i.to_string().as_str())
        );
    }
    // Reads from many threads at once, while writes go on.
    std::thread::scope(|s| {
        for _ in 0..8 {
            let meta = meta.clone();
            s.spawn(move || {
                for _ in 0..50 {
                    assert!(meta.setting("k").unwrap().is_some());
                }
            });
        }
        for i in 0..50 {
            meta.set_setting("other", &i.to_string()).unwrap();
        }
    });
    assert_eq!(meta.setting("other").unwrap().as_deref(), Some("49"));
}

/// #450: identity kept at ingestion follows the rules the query layer
/// applied to the files, and erasure removes it.
#[test]
fn identity_rows_follow_the_alias_rules_and_erasure_removes_them() {
    let m = Meta::in_memory().unwrap();
    let id = |p: &str, a: Option<&str>, at: i64, internal: bool| {
        (p.to_owned(), a.map(str::to_owned), at, internal)
    };
    // Not readable until the backfill ran, even with rows recorded.
    m.record_identities("p", &[id("bob", Some("anon-1"), 20, false)])
        .unwrap();
    assert!(m.identities("p").unwrap().is_none());
    m.backfill_identities("p", &[], &[]).unwrap();
    m.record_identities(
        "p",
        &[
            // Identified as two users: the smallest wins, whatever the order.
            id("alice", Some("anon-1"), 30, false),
            id("carol", Some("anon-1"), 5, true),
            // An id is never its own alias.
            id("dave", Some("dave"), 40, false),
            // `internal` stays set, `first_seen` keeps the earliest.
            id("carol", None, 50, false),
        ],
    )
    .unwrap();
    let t = m.identities("p").unwrap().unwrap();
    assert_eq!(t.aliases, vec![("anon-1".to_owned(), "alice".to_owned())]);
    let carol = t.persons.iter().find(|p| p.person == "carol").unwrap();
    assert!(carol.internal);
    assert!(carol.first_seen < 50);
    let mut ids: Vec<u32> = t.persons.iter().map(|p| p.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), 4, "one id per person: {:?}", t.persons);

    // Erasure drops the person and every alias to or from them; their id
    // is never given to anyone else.
    let alice = t.persons.iter().find(|p| p.person == "alice").unwrap().id;
    let top = t.persons.iter().map(|p| p.id).max().unwrap();
    m.forget("p", &["alice".to_owned()]).unwrap();
    let t = m.identities("p").unwrap().unwrap();
    assert!(t.aliases.is_empty());
    assert!(t.persons.iter().all(|p| p.person != "alice"));
    assert!(m.aliases_of("p", "alice").unwrap().is_empty());
    // A late write (a backfill that read the files before the erasure)
    // does not bring them back.
    m.record_identities("p", &[id("alice", Some("anon-2"), 1, false)])
        .unwrap();
    assert!(
        m.identities("p")
            .unwrap()
            .unwrap()
            .persons
            .iter()
            .all(|p| p.person != "alice")
    );
    m.record_identities("p", &[id("erin", None, 60, false)])
        .unwrap();
    let erin = m.identities("p").unwrap().unwrap().persons;
    let erin = erin.iter().find(|p| p.person == "erin").unwrap().id;
    assert!(erin > top && erin != alice);
}

#[test]
fn build_fingerprints_stay_bounded_per_day_and_per_project() {
    use openlens_meta::releases::{MAX_BUILDS, MAX_NEW_BUILDS_PER_DAY};
    let m = Meta::in_memory().unwrap();
    let rec =
        |b: &str, day: &str, at: i64| m.record_build("p", "production", b, None, day, at).unwrap();
    let first = rec("known", "2026-11-01", 0).unwrap();
    for i in 1..MAX_NEW_BUILDS_PER_DAY {
        assert!(rec(&format!("b{i}"), "2026-11-01", i).is_some());
    }
    // The day is full: a new fingerprint is refused, a known one keeps its name.
    assert_eq!(rec("one-too-many", "2026-11-01", 999), None);
    assert_eq!(rec("known", "2026-11-01", 999), Some(first));
    assert!(
        rec("one-too-many", "2026-11-02", 1_000).is_some(),
        "a new day has room"
    );
    // Over the project's cap, the oldest fingerprints go first.
    let days = MAX_BUILDS / MAX_NEW_BUILDS_PER_DAY + 1;
    let mut at = 2_000;
    for d in 0..days {
        for i in 0..MAX_NEW_BUILDS_PER_DAY {
            at += 1;
            rec(&format!("d{d}-{i}"), &format!("2027-01-{:02}", d + 1), at);
        }
    }
    assert_eq!(m.build_release("p", "production", "known").unwrap(), None);
    let newest = format!("d{}-{}", days - 1, MAX_NEW_BUILDS_PER_DAY - 1);
    assert!(
        m.build_release("p", "production", &newest)
            .unwrap()
            .is_some()
    );
}

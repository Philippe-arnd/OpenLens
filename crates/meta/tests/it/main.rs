//! Integration tests for `openlens-meta`.

// Test helpers may unwrap: a failure here is a test failure (clippy.toml
// already allows it inside #[test] functions; this extends it to helpers).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Utc};
use openlens_meta::{Meta, slug};

mod uptime_history;

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

/// #712 (D-103): a user who moves from free to pro has the plan of the
/// time: free before the change, pro after; a repeat is no change, and
/// forgetting the user drops the history.
#[test]
fn a_plan_change_is_dated_and_the_plan_at_a_time_follows_it() {
    let m = Meta::in_memory().unwrap();
    let t0 = Utc::now() - Duration::days(10);
    let upgrade = t0 + Duration::days(4);
    m.identify("p", "u1", Some("member"), None, Some("free"), t0)
        .unwrap();
    m.identify("p", "u1", None, None, Some("free"), t0 + Duration::days(1))
        .unwrap();
    m.identify("p", "u1", None, None, None, t0 + Duration::days(2))
        .unwrap();
    m.identify("p", "u1", None, None, Some("pro"), upgrade)
        .unwrap();
    let hist = m.plan_history("p", "u1").unwrap();
    let plans: Vec<_> = hist.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(plans, ["free", "pro"], "repeats and silences are no change");
    let at = |t| openlens_meta::plan_at(&hist, t);
    assert_eq!(at(t0 - Duration::hours(1)), None, "before the first plan");
    assert_eq!(at(upgrade - Duration::hours(1)), Some("free"));
    assert_eq!(at(upgrade + Duration::hours(1)), Some("pro"));
    m.forget("p", &["u1".to_owned()]).unwrap();
    assert!(m.plan_history("p", "u1").unwrap().is_empty());
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

#[test]
fn a_deploy_hook_secret_is_kept_hashed_and_deploys_stay_bounded() {
    use openlens_meta::deploys::{Deploy, MAX_DEPLOYS};
    let m = Meta::in_memory().unwrap();
    m.ensure_default_project(None).unwrap();
    assert_eq!(m.rotate_deploy_hook("nope").unwrap(), None);
    let first = m.rotate_deploy_hook("default").unwrap().unwrap();
    assert_eq!(
        m.deploy_hook_project(&first).unwrap().as_deref(),
        Some("default")
    );
    let second = m.rotate_deploy_hook("default").unwrap().unwrap();
    assert_eq!(m.deploy_hook_project(&first).unwrap(), None, "rotated away");
    assert_eq!(
        m.deploy_hook_project(&second).unwrap().as_deref(),
        Some("default")
    );
    assert!(m.deploy_hook_since("default").unwrap().is_some());
    let d = |at: i64| Deploy {
        environment: "production".into(),
        release: None,
        commit: None,
        status: "succeeded".into(),
        at,
        source: "plain".into(),
    };
    for at in 0..MAX_DEPLOYS + 10 {
        m.record_deploy("default", &d(at), at).unwrap();
    }
    let kept = m.deploys("default", 0).unwrap();
    assert_eq!(kept.len() as i64, MAX_DEPLOYS);
    assert_eq!(
        kept.first().map(|d| d.at),
        Some(10),
        "the oldest went first"
    );
    assert!(m.delete_project("default").unwrap());
    assert!(m.deploys("default", 0).unwrap().is_empty());
    assert_eq!(m.deploy_hook_project(&second).unwrap(), None);
}

#[test]
fn a_recorded_release_starts_its_range_after_the_previous_one() {
    use openlens_meta::deploys::{Deploy, Range};
    let m = Meta::in_memory().unwrap();
    m.ensure_default_project(None).unwrap();
    let d = |release: &str, commit: &str, status: &str, at: i64| Deploy {
        environment: "production".into(),
        release: Some(release.into()),
        commit: Some(commit.into()),
        status: status.into(),
        at,
        source: "cli".into(),
    };
    assert_eq!(
        m.previous_commit("default", "production", None).unwrap(),
        None
    );
    m.record_release(
        "default",
        &d("v1", "aaaaaaa", "succeeded", 10),
        &Range::default(),
        10,
    )
    .unwrap();
    m.record_deploy("default", &d("v2", "bbbbbbb", "failed", 20), 20)
        .unwrap();
    // A failed deploy is not where the next range starts, and a release
    // recorded again with the same commit does not start after itself.
    let from = m
        .previous_commit("default", "production", Some("ccccccc"))
        .unwrap();
    assert_eq!(from.as_deref(), Some("aaaaaaa"));
    assert_eq!(
        m.previous_commit("default", "production", Some("aaaaaaa"))
            .unwrap(),
        None
    );
    assert_eq!(
        m.previous_commit("default", "staging", Some("ccccccc"))
            .unwrap(),
        None,
        "another environment's deploys are not this one's range"
    );
    m.record_release(
        "default",
        &d("v3", "ccccccc", "succeeded", 30),
        &Range {
            from: from.as_deref(),
            first_commit_at: Some(25),
        },
        30,
    )
    .unwrap();
    let (start, end) = m
        .release_range("default", "production", "v3")
        .unwrap()
        .unwrap();
    assert_eq!(
        (start.as_deref(), end.as_str()),
        (Some("aaaaaaa"), "ccccccc")
    );
    assert_eq!(
        m.deploys("default", 0).unwrap().len(),
        3,
        "a release is a deploy"
    );
    // Only the release whose deploy step said when its first commit was
    // made has a first commit time (#633).
    assert_eq!(
        m.first_commits("default", 0).unwrap(),
        vec![("v3".to_owned(), 25)]
    );
    // The GitHub read of a range (#574) gives the first commit of a
    // release recorded without it; the deploy step's own time still wins.
    use openlens_meta::release_reads::{GitRead, ReleaseRange};
    let range = |release: &str, from: &str, to: &str| ReleaseRange {
        environment: "production".into(),
        release: release.into(),
        from: from.into(),
        to: to.into(),
    };
    m.record_release(
        "default",
        &d("v4", "ddddddd", "succeeded", 40),
        &Range {
            from: Some("ccccccc"),
            first_commit_at: None,
        },
        40,
    )
    .unwrap();
    for (release, from, to, at) in [
        ("v4", "ccccccc", "ddddddd", 35),
        ("v3", "aaaaaaa", "ccccccc", 5),
    ] {
        let read = GitRead {
            first_commit_at: Some(at),
            ..GitRead::default()
        };
        m.store_release_read("default", &range(release, from, to), &read, 50)
            .unwrap();
    }
    let kept: std::collections::HashMap<String, i64> =
        m.first_commits("default", 0).unwrap().into_iter().collect();
    assert_eq!(kept.get("v4"), Some(&35), "{kept:?}");
    assert_eq!(
        kept.get("v3"),
        Some(&25),
        "the deploy step's time wins: {kept:?}"
    );
}

/// #510: a write transaction that meets the write lock held waits for it
/// (`BEGIN IMMEDIATE` calls the busy handler) instead of failing at once
/// with "database is locked", which capture answered with a 503.
#[test]
fn a_write_transaction_waits_for_the_lock_instead_of_failing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.db");
    let meta = Meta::open(&path).unwrap();
    let other = rusqlite::Connection::open(&path).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    std::thread::scope(|s| {
        s.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            other.execute_batch("COMMIT").unwrap();
        });
        let row = ("bob".to_owned(), Some("anon-1".to_owned()), 1, false);
        assert!(meta.record_identities("p", &[row]).unwrap());
    });
}

/// #510: a batch's people and identity rows go in one write, with the
/// same effect as one `identify` each.
#[test]
fn a_batch_of_identifies_is_recorded_in_one_write() {
    let m = Meta::in_memory().unwrap();
    let at = Utc::now();
    let seen = |id: &'static str, role: &'static str| openlens_meta::Seen {
        distinct_id: id,
        role: Some(role),
        account: None,
        plan: Some("pro"),
        at,
        signed_up: None,
    };
    let rows = [("ann".to_owned(), Some("anon-9".to_owned()), 1, false)];
    let changed = m
        .record_identify("p", &[seen("ann", "admin"), seen("ann", "member")], &rows)
        .unwrap();
    assert!(changed);
    let ann = m.person("p", "ann").unwrap().unwrap();
    assert_eq!(ann.role.as_deref(), Some("member"));
    assert_eq!(ann.plan.as_deref(), Some("pro"));
    assert_eq!(m.role_history("p", "ann").unwrap().len(), 2);
    assert_eq!(m.aliases_of("p", "ann").unwrap(), vec!["anon-9".to_owned()]);
    // Nothing to record: no write, nothing changed.
    assert!(!m.record_identify("p", &[], &[]).unwrap());
}

/// #707 (D-111): the account's creation date is kept on the person, the
/// earliest one wins whatever the order, an identify without one keeps
/// it, and the identity tables the query layer reads carry it.
#[test]
fn the_earliest_signup_date_wins_and_reaches_the_identity_tables() {
    let m = Meta::in_memory().unwrap();
    m.backfill_identities("p", &[], &[]).unwrap();
    let now = Utc::now();
    let (early, late) = (
        now - chrono::Duration::days(400),
        now - chrono::Duration::days(30),
    );
    let seen = |signed_up| openlens_meta::Seen {
        distinct_id: "ann",
        role: None,
        account: None,
        plan: None,
        at: now,
        signed_up,
    };
    let row = [("ann".to_owned(), None, now.timestamp_micros(), false)];
    assert!(m.record_identify("p", &[seen(Some(late))], &row).unwrap());
    assert!(
        m.record_identify("p", &[seen(Some(early))], &[]).unwrap(),
        "an earlier date is a change"
    );
    assert!(
        !m.record_identify("p", &[seen(Some(late))], &[]).unwrap(),
        "a later one is not"
    );
    assert!(!m.record_identify("p", &[seen(None)], &[]).unwrap());
    let t = m.identities("p").unwrap().unwrap();
    let ann = t.persons.iter().find(|p| p.person == "ann").unwrap();
    let kept = ann.signed_up.unwrap();
    assert!(kept < late.timestamp_micros() && kept < ann.first_seen);
    assert!(m.person("p", "ann").unwrap().unwrap().signed_up.is_some());
}

#[test]
fn a_release_keeps_the_map_version_first_recorded() {
    use openlens_meta::release_maps::ReleaseMap;
    let m = Meta::in_memory().unwrap();
    let row = |release: &str, version: i64| ReleaseMap {
        environment: "production".into(),
        release: release.into(),
        first_seen: "2026-11-01 10:00:00".into(),
        map_version: version,
        before_first_map: false,
    };
    assert_eq!(m.record_release_maps("p", &[row("v1", 1)]).unwrap(), 1);
    // A later read derives another version (its first event left the
    // window, or the version left the newest 50): the record wins.
    assert_eq!(
        m.record_release_maps("p", &[row("v1", 3), row("v2", 3)])
            .unwrap(),
        1
    );
    let got = m.release_maps("p", "production").unwrap();
    let v1 = got.iter().find(|r| r.release == "v1").unwrap();
    assert_eq!(v1.map_version, 1);
    assert_eq!(got.len(), 2);
    assert!(m.release_maps("p", "staging").unwrap().is_empty());
}

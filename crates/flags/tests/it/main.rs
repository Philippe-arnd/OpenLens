//! Integration tests for `openlens-flags`.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use openlens_flags::{Flag, Subject, bucket, evaluate};

fn flag(rollout: u8) -> Flag {
    Flag {
        key: "new-editor".into(),
        description: String::new(),
        enabled: true,
        rollout,
        roles: vec![],
        accounts: vec![],
    }
}

fn share_on(f: &Flag, n: usize) -> f64 {
    let on = (0..n)
        .filter(|i| {
            f.is_on(&Subject {
                distinct_id: &format!("user-{i}"),
                ..Subject::default()
            })
        })
        .count();
    on as f64 / n as f64
}

#[test]
fn rollout_share_holds_within_one_point_over_100k_users() {
    for pct in [1u8, 10, 50, 90] {
        let got = share_on(&flag(pct), 100_000);
        assert!(
            (got - f64::from(pct) / 100.0).abs() < 0.01,
            "{pct}% gave {got}"
        );
    }
}

#[test]
fn answers_are_stable_and_widening_never_turns_anyone_off() {
    let (narrow, wide) = (flag(20), flag(60));
    for i in 0..5_000 {
        let s = Subject {
            distinct_id: &format!("u{i}"),
            ..Subject::default()
        };
        assert_eq!(narrow.is_on(&s), narrow.is_on(&s));
        assert!(
            !narrow.is_on(&s) || wide.is_on(&s),
            "u{i} lost the flag when the rollout widened"
        );
    }
}

#[test]
fn buckets_differ_between_flags() {
    let same = (0..2_000)
        .filter(|i| {
            (bucket("a", &format!("u{i}")) < 5_000) == (bucket("b", &format!("u{i}")) < 5_000)
        })
        .count();
    assert!(
        same > 800 && same < 1_200,
        "two flags at 50% should be independent, overlap {same}"
    );
}

#[test]
fn targeting_by_role_and_account() {
    let f = Flag {
        roles: vec!["admin".into()],
        accounts: vec!["acme".into()],
        ..flag(100)
    };
    assert!(f.is_on(&Subject {
        distinct_id: "x",
        role: Some("admin"),
        account: None
    }));
    assert!(!f.is_on(&Subject {
        distinct_id: "x",
        role: Some("member"),
        account: None
    }));
    assert!(!f.is_on(&Subject {
        distinct_id: "x",
        role: None,
        account: None
    }));
    assert!(
        f.is_on(&Subject {
            distinct_id: "x",
            role: Some("member"),
            account: Some("acme")
        }),
        "accounts bypass targeting"
    );
    let off = Flag {
        enabled: false,
        ..f
    };
    assert!(!off.is_on(&Subject {
        distinct_id: "x",
        role: Some("admin"),
        account: Some("acme")
    }));
}

#[test]
fn validation_and_evaluate() {
    assert!(
        Flag {
            key: "Bad Key".into(),
            ..flag(10)
        }
        .validate()
        .is_err()
    );
    assert!(flag(100).validate().is_ok());
    let flags = vec![
        flag(100),
        Flag {
            key: "z-off".into(),
            enabled: false,
            ..flag(100)
        },
        Flag {
            key: "a".into(),
            ..flag(100)
        },
    ];
    assert_eq!(
        evaluate(
            &flags,
            &Subject {
                distinct_id: "u",
                ..Subject::default()
            }
        ),
        ["a", "new-editor"]
    );
}

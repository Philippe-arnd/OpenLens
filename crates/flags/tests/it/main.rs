//! Integration tests for `openlens-flags`.

// Test helpers may unwrap: a failure here is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use openlens_flags::{Flag, Rule, Subject, bucket, evaluate};

fn flag(rollout: u8) -> Flag {
    Flag {
        key: "new-editor".into(),
        description: String::new(),
        enabled: true,
        rollout,
        roles: vec![],
        accounts: vec![],
        variants: vec![],
        rules: vec![],
        users: vec![],
        holdout: 0,
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
        account: None,
        properties: &[]
    }));
    assert!(!f.is_on(&Subject {
        distinct_id: "x",
        role: Some("member"),
        account: None,
        properties: &[]
    }));
    assert!(!f.is_on(&Subject {
        distinct_id: "x",
        role: None,
        account: None,
        properties: &[]
    }));
    assert!(
        f.is_on(&Subject {
            distinct_id: "x",
            role: Some("member"),
            account: Some("acme"),
            properties: &[]
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
        account: Some("acme"),
        properties: &[]
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

#[test]
fn variants_split_by_weight_and_stay_put_when_the_rollout_grows() {
    use openlens_flags::Variant;
    let mut f = flag(50);
    f.variants = vec![
        Variant {
            key: "control".into(),
            weight: 70,
        },
        Variant {
            key: "short".into(),
            weight: 30,
        },
    ];
    assert!(f.validate().is_ok());
    let n = 40_000;
    let picks: Vec<Option<String>> = (0..n)
        .map(|i| {
            f.variant(&Subject {
                distinct_id: &format!("user-{i}"),
                ..Subject::default()
            })
            .map(str::to_owned)
        })
        .collect();
    let on = picks.iter().filter(|p| p.is_some()).count() as f64;
    let short = picks
        .iter()
        .filter(|p| p.as_deref() == Some("short"))
        .count() as f64;
    assert!(
        (on / n as f64 - 0.5).abs() < 0.01,
        "only users in the rollout get a variant"
    );
    assert!((short / on - 0.3).abs() < 0.01, "30% of them get `short`");
    // Widening the rollout keeps everyone's variant.
    let mut wider = f.clone();
    wider.rollout = 100;
    for (i, p) in picks.iter().enumerate().take(5000) {
        if let Some(v) = p {
            let s = Subject {
                distinct_id: &format!("user-{i}"),
                ..Subject::default()
            };
            assert_eq!(wider.variant(&s), Some(v.as_str()));
        }
    }
    // Validation.
    let mut bad = f.clone();
    bad.variants[0].weight = 60;
    assert!(bad.validate().is_err(), "weights must add up to 100");
    bad.variants = vec![Variant {
        key: "solo".into(),
        weight: 100,
    }];
    assert!(bad.validate().is_err(), "at least two");
    let off = Flag {
        variants: f.variants.clone(),
        ..flag(0)
    };
    assert_eq!(
        off.variant(&Subject {
            distinct_id: "x",
            ..Subject::default()
        }),
        None
    );
}

#[test]
fn property_rules_all_match_and_a_missing_property_fails() {
    let f = Flag {
        roles: vec!["admin".into()],
        accounts: vec!["vip".into()],
        rules: vec![
            Rule::parse("plan in pro, Team").unwrap(),
            Rule::parse("country not in FR").unwrap(),
        ],
        ..flag(100)
    };
    assert!(f.validate().is_ok());
    let props = |plan: &str, country: &str| {
        let mut v = Vec::new();
        if !plan.is_empty() {
            v.push(("plan".to_owned(), plan.to_owned()));
        }
        if !country.is_empty() {
            v.push(("country".to_owned(), country.to_owned()));
        }
        v
    };
    let on = |role: &str, account: Option<&str>, p: &[(String, String)]| {
        f.is_on(&Subject {
            distinct_id: "u",
            role: Some(role),
            account,
            properties: p,
        })
    };
    assert!(on("admin", None, &props("team", "DE")), "case-insensitive");
    assert!(!on("admin", None, &props("free", "DE")));
    assert!(!on("admin", None, &props("pro", "fr")), "not in");
    assert!(
        !on("admin", None, &props("pro", "")),
        "missing fails not_in too"
    );
    assert!(!on("member", None, &props("pro", "DE")), "AND with roles");
    assert!(on("member", Some("vip"), &[]), "accounts still bypass");
    let role_rule = Flag {
        rules: vec![Rule::parse("account in acme").unwrap()],
        ..flag(100)
    };
    assert!(role_rule.is_on(&Subject {
        distinct_id: "u",
        account: Some("acme"),
        ..Subject::default()
    }));
    // Parsing and limits.
    assert_eq!(f.rules[1].to_string(), "country not in FR");
    assert!(Rule::parse("plan pro").is_err());
    assert!(Rule::parse("Plan Name in x").is_err());
    assert!(Rule::parse("plan in ").is_err());
    let many = Flag {
        rules: vec![Rule::parse("plan in pro").unwrap(); 11],
        ..flag(100)
    };
    assert!(many.validate().is_err());
    let json = serde_json::to_string(&f.rules[1]).unwrap();
    assert!(json.contains("\"not_in\""), "{json}");
}

#[test]
fn a_user_list_with_a_holdout_keeps_a_stable_control_group() {
    let users: Vec<String> = (0..1000).map(|i| format!("u{i}")).collect();
    let f = Flag {
        users: users.clone(),
        holdout: 20,
        ..flag(100)
    };
    assert!(f.validate().is_ok());
    let on = |id: &str| {
        f.is_on(&Subject {
            distinct_id: id,
            ..Subject::default()
        })
    };
    assert!(!on("stranger"), "only listed users");
    let treated = users.iter().filter(|u| on(u)).count();
    assert!((720..880).contains(&treated), "{treated}");
    assert!(users.iter().all(|u| on(u) != f.held_out(u)));
    assert!(
        Flag {
            holdout: 60,
            ..f.clone()
        }
        .validate()
        .is_err()
    );
}

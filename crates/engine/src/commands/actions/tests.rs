use super::*;

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn card(kind: &'static str, subject: &str, score: f64) -> Insight {
    Insight {
        kind,
        title: format!("{kind} on {subject}"),
        evidence: "numbers".into(),
        next_step: "look".into(),
        score,
        subject: subject.into(),
        figure: None,
    }
}

fn lead(release: &str) -> Lead {
    Lead {
        release: release.into(),
        previous: Some("v1".into()),
        sessions: (800, Some(900)),
        headline: "More sessions hit an error".into(),
        evidence: "800 sessions on v2, 900 on v1.".into(),
        next_step: "Open the errors of v2".into(),
        layers: Vec::new(),
        metric: Some("error_rate".into()),
    }
}

fn tracked(id: &str, since: &str) -> Tracked {
    Tracked {
        id: id.into(),
        finding: from_card("p", &card("friction", id, 1.0)),
        since: since.into(),
        proposed: None,
        approved: None,
        dismissed: None,
    }
}

#[test]
fn the_same_finding_seen_twice_is_one_action() {
    let c = card("friction", "checkout", 3.0);
    let found = vec![from_card("p", &c), from_card("p", &c)];
    let all = assemble("p", found, &[], &[], d("2026-11-28"));
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].state, State::New);
    assert_eq!(all[0].id, id_of("p", "insight", "friction:checkout"));
}

#[test]
fn a_regressed_release_ranks_above_a_low_impact_card() {
    let low = from_card("p", &card("friction", "search", 0.5));
    let big = from_card("p", &card("stick_driver", "export", 40.0));
    let rel = from_lead("p", "production", &lead("v2"));
    let all = assemble("p", vec![low, big, rel], &[], &[], d("2026-11-28"));
    let order: Vec<&str> = all.iter().map(|a| a.finding.subject.as_str()).collect();
    assert_eq!(
        order,
        ["production:v2", "stick_driver:export", "friction:search"]
    );
    assert!(all.windows(2).all(|w| w[0].rank >= w[1].rank));
}

#[test]
fn a_dismissed_action_stays_hidden_for_thirty_days() {
    let c = card("friction", "checkout", 3.0);
    let id = id_of("p", "insight", "friction:checkout");
    let today = d("2026-11-28");
    let mut t = tracked(&id, "2026-11-28");
    t.dismissed = Some(Dismissal {
        until: ymd(today + chrono::Duration::days(DISMISS_DAYS)),
        reason: "not now".into(),
    });
    let seen = |on: NaiveDate| {
        let all = assemble(
            "p",
            vec![from_card("p", &c)],
            std::slice::from_ref(&t),
            &[],
            on,
        );
        list(all, None, Vec::new())
    };
    let before = seen(today + chrono::Duration::days(DISMISS_DAYS - 1));
    assert!(before.actions.is_empty() && before.dismissed == 1);
    let after = seen(today + chrono::Duration::days(DISMISS_DAYS));
    assert_eq!(after.actions.len(), 1, "back once the dismissal ends");
    assert_eq!(after.dismissed, 0);
}

#[test]
fn a_finding_that_left_the_board_while_new_leaves_the_list() {
    let id = id_of("p", "insight", "friction:checkout");
    let mut t = tracked(&id, "2026-11-01");
    let today = d("2026-11-28");
    assert!(assemble("p", Vec::new(), &[], &[], today).is_empty());
    t.approved = Some("2026-11-02".into());
    let gone = assemble("p", Vec::new(), &[t], &[], today);
    assert!(gone.len() == 1 && !gone[0].current && gone[0].state == State::Approved);
}

#[test]
fn pruning_drops_the_finished_first() {
    let mut all: Vec<Tracked> = (0..MAX_TRACKED + 2)
        .map(|i| tracked(&format!("a{i}"), &format!("2026-01-{:02}", 1 + i % 28)))
        .collect();
    all[5].since = "2025-01-01".into();
    let keep = all[5].id.clone();
    let done = ["a7", "a9"];
    prune(&mut all, |t| done.contains(&t.id.as_str()));
    assert_eq!(all.len(), MAX_TRACKED);
    assert!(
        all.iter().any(|t| t.id == keep),
        "the oldest open one stays"
    );
    assert!(all.iter().all(|t| !done.contains(&t.id.as_str())));
}

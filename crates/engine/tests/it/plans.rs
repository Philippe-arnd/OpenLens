//! Plans (#712, #713, D-113): upgrades and downgrades per period and
//! release, plan gaps, the conversion drivers card, and nothing at all
//! for an app without plans.

use super::*;

fn identify(user: &str, plan: &str, days_ago: i64, release: &str, n: usize) -> Event {
    let ts = Utc::now() - Duration::days(days_ago);
    Event {
        uuid: format!("{user}-{days_ago}-{n}"),
        project: "p".into(),
        event: "$identify".into(),
        distinct_id: user.into(),
        session_id: Some(format!("{user}-{days_ago}")),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some("/".into()),
        feature: None,
        role: Some("member".into()),
        account: None,
        plan: Some(plan.into()),
        environment: "production".into(),
        release: Some(release.into()),
        flags: None,
        properties: "{}".into(),
    }
}

fn engine(events: &[Event]) -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    w.append(events).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

/// A user who moves from free to pro is free before the change and pro
/// after: one upgrade, on the release they were on when it happened; a
/// move back is a downgrade, and a change within one plan is neither.
#[tokio::test]
async fn a_move_from_free_to_pro_counts_once_as_an_upgrade_on_its_release() {
    let events = [
        identify("ann", "free", 12, "v1", 0),
        identify("ann", "free", 9, "v1", 1),
        identify("ann", "pro", 5, "v2", 2),
        identify("ann", "pro", 2, "v2", 3),
        identify("bob", "pro", 11, "v1", 4),
        identify("bob", "free", 3, "v2", 5),
        identify("cyd", "pro", 10, "v1", 6),
        identify("cyd", "pro_yearly", 4, "v2", 7),
    ];
    let (_d, q) = engine(&events);
    let map = parse(&openlens_testkit::plans_feature_map()).map.unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    let plans = b.plans.expect("the map declares plans");
    assert_eq!(
        (plans.upgrades, plans.downgrades),
        (1, 1),
        "ann upgrades, bob downgrades, cyd stays on pro: {plans:?}"
    );
    let on = |r: &str| plans.releases.iter().find(|x| x.release == r);
    assert!(
        on("v2").is_some_and(|x| x.upgrades == 1 && x.downgrades == 1),
        "{plans:?}"
    );
    assert!(on("v1").is_none(), "nobody moved on v1: {plans:?}");
}

/// The planted conversion driver ranks first among the conversion
/// drivers, and the plan nobody declared is a plan gap.
#[tokio::test]
async fn the_planted_conversion_driver_ranks_first() {
    let (_d, q) = engine(&openlens_testkit::plans_dataset("p"));
    let map = parse(&openlens_testkit::plans_feature_map()).map.unwrap();
    let b = board(&q, "p", &Filters::default(), Some((&map, 1)))
        .await
        .unwrap();
    let kinds: Vec<_> = b
        .insights
        .iter()
        .map(|i| (i.kind, i.subject.as_str()))
        .collect();
    let first = b
        .insights
        .iter()
        .find(|i| i.kind == "conversion_driver")
        .unwrap_or_else(|| panic!("no conversion driver in {kinds:?}"));
    assert_eq!(
        first.subject,
        openlens_testkit::CONVERSION_DRIVER,
        "{kinds:?}"
    );
    assert!(first.evidence.contains("free users"), "{}", first.evidence);
    let plans = b.plans.unwrap();
    assert!(plans.upgrades + plans.prev_upgrades > plans.downgrades + plans.prev_downgrades);
    assert!(
        plans
            .gaps
            .iter()
            .any(|(raw, _)| raw == openlens_testkit::UNMATCHED_PLAN),
        "{:?}",
        plans.gaps
    );
}

/// An app whose map declares no plans (an internal tool, or one that has
/// not drafted `plans:` yet) sees nothing new, even if it sends plans.
#[tokio::test]
async fn an_app_without_plans_sees_nothing_new() {
    let (_d, q) = engine(&openlens_testkit::plans_dataset("p"));
    let with = openlens_testkit::plans_feature_map();
    let start = with.find("plans:").unwrap();
    let end = with.find("features:").unwrap();
    let without = format!("{}{}", &with[..start], &with[end..]);
    let map = parse(&without).map.unwrap();
    assert!(map.plans.is_empty());
    for m in [Some((&map, 1)), None] {
        let b = board(&q, "p", &Filters::default(), m).await.unwrap();
        assert!(b.plans.is_none());
        assert!(b.insights.iter().all(|i| i.kind != "conversion_driver"));
        let v = serde_json::to_value(&b).unwrap();
        assert!(v.get("plans").is_none(), "no plans key in the board");
    }
}

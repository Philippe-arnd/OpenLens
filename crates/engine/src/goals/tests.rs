use super::*;

fn s(v: &[(&str, f64)]) -> Vec<(String, f64)> {
    v.iter().map(|(d, x)| ((*d).to_owned(), *x)).collect()
}

#[test]
fn suggests_a_reachable_step() {
    // Errors (down is better): baseline = median of the last 3 = 0.10;
    // the step to 0.09 is closer than the best release seen (0.08).
    let (base, t) = suggest(
        0.12,
        &s(&[
            ("2026-01-01", 0.08),
            ("2026-02-01", 0.10),
            ("2026-03-01", 0.11),
            ("2026-04-01", 0.10),
        ]),
        false,
        true,
    );
    assert!((base - 0.10).abs() < 1e-9);
    assert!((t - 0.09).abs() < 1e-9);
    // Up: best seen 52 is closer than +10% (55) from 50.
    let (_, t) = suggest(
        50.0,
        &s(&[
            ("2026-01-01", 52.0),
            ("2026-02-01", 50.0),
            ("2026-03-01", 49.0),
            ("2026-04-01", 50.0),
        ]),
        true,
        false,
    );
    assert!((t - 52.0).abs() < 1e-9);
    // No history: +10% of the current value.
    assert!((suggest(0.5, &[], true, true).1 - 0.55).abs() < 1e-9);
}

#[test]
fn projects_the_trend_to_the_deadline() {
    let today = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap_or_default();
    let g = Goal {
        id: "g".into(),
        kpi: "error_rate".into(),
        target: 0.05,
        up: false,
        deadline: "2026-06-01".into(),
        owner: String::new(),
        baseline: 0.12,
        created: "2026-03-01".into(),
        guardrails: Vec::new(),
        releases: None,
    };
    // Falling 0.02 a month: reaches 0.04 by June → on track.
    let falling = s(&[
        ("2026-01-01", 0.12),
        ("2026-02-01", 0.10),
        ("2026-03-01", 0.08),
    ]);
    assert_eq!(status(&g, Some(0.08), &falling, today).0, Status::OnTrack);
    // Flat at 0.08 → off track (60% above the target).
    let flat = s(&[
        ("2026-01-01", 0.08),
        ("2026-02-01", 0.08),
        ("2026-03-01", 0.08),
    ]);
    assert_eq!(status(&g, Some(0.08), &flat, today).0, Status::OffTrack);
    // Close: projected 0.054 → at risk.
    let close = s(&[
        ("2026-01-01", 0.058),
        ("2026-02-01", 0.057),
        ("2026-03-01", 0.056),
    ]);
    assert_eq!(status(&g, Some(0.056), &close, today).0, Status::AtRisk);
    assert_eq!(status(&g, Some(0.04), &flat, today).0, Status::Reached);
    assert_eq!(
        status(&g, Some(0.08), &flat[..2], today).0,
        Status::NotEnoughData
    );
}

#[test]
fn guardrails_breach_when_worse_than_the_limit() {
    let g = Guardrail {
        kpi: "bounce_rate".into(),
        limit: Guardrail::suggested(0.10, false, true),
        up: false,
    };
    assert!(
        (g.limit - 0.11).abs() < 1e-9,
        "a share moves by a point at least"
    );
    assert!(g.breached(0.12) && !g.breached(0.10));
    let up = Guardrail {
        kpi: "dau".into(),
        limit: Guardrail::suggested(200.0, true, false),
        up: true,
    };
    assert!((up.limit - 190.0).abs() < 1e-9);
    assert!(up.breached(180.0) && !up.breached(195.0));
}

#[test]
fn objectives_take_their_worst_key_result() {
    assert_eq!(
        objective_status(&[Status::Reached, Status::AtRisk, Status::OnTrack]),
        Status::AtRisk
    );
    assert_eq!(objective_status(&[]), Status::NotEnoughData);
}

#[test]
fn deadlines_in_releases_use_the_median_gap() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap_or_default();
    let firsts: Vec<String> = [
        "2026-10-01T10:00:00Z",
        "2026-10-15",
        "2026-10-29",
        "2026-11-26",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    // Gaps 14, 14, 28: median 14 → 3 releases = 42 days.
    assert_eq!(
        deadline_in_releases(&firsts, 3, today),
        NaiveDate::from_ymd_opt(2027, 1, 4)
    );
    assert_eq!(deadline_in_releases(&firsts[..1], 3, today), None);
}

#[test]
fn weekly_users_fold_into_dau_mau_and_stickiness() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap_or_default(); // a Wednesday
    let p = |u: &str, d: &str| (u.to_owned(), d.to_owned());
    let pairs = vec![
        p("a", "2026-11-23"),
        p("b", "2026-11-23"),
        p("a", "2026-11-24"),
        p("c", "2026-11-10"),
    ];
    let w = fold_users(&pairs, today);
    let empty = (String::new(), 0.0);
    let last = w.dau.last().unwrap_or(&empty);
    assert_eq!(last.0, "2026-11-23");
    // Mon 2 users, Tue 1, Wed 0 → 1.0 per day over 3 days.
    assert!((last.1 - 1.0).abs() < 1e-9);
    assert_eq!(
        w.mau.last().map(|x| x.1),
        Some(3.0),
        "c was active 2 weeks ago"
    );
    assert!((w.stickiness.last().map_or(0.0, |x| x.1) - 1.0 / 3.0).abs() < 1e-9);
    assert_eq!(
        w.dau.first().map(|x| x.0.as_str()),
        Some("2026-11-09"),
        "empty weeks skipped"
    );
}

#[test]
fn revenue_in_several_currencies_is_refused_unless_the_rates_convert_it() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap_or_default();
    let rows = vec![
        ("2026-11-20".to_owned(), Some("EUR".to_owned()), 100.0),
        ("2026-11-21".to_owned(), Some("USD".to_owned()), 50.0),
    ];
    let mixed = fold_revenue(&rows, None, today);
    assert!(mixed.now.is_none() && mixed.refused.is_some(), "{mixed:?}");
    let rates = RevenueSettings {
        currency: "EUR".into(),
        rates: [("USD".to_owned(), 0.9)].into_iter().collect(),
    };
    let one = fold_revenue(&rows, Some(&rates), today);
    assert!(one.refused.is_none(), "{one:?}");
    let now = one.now.unwrap_or_default();
    assert!(now > 100.0 && now < 150.0, "USD converted, not added as is");
    assert_eq!(one.series.last().map(|x| x.1), one.now);
}

#[test]
fn adoption_is_a_share_of_the_same_weeks_active_users() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap_or_default();
    let p = |u: &str, d: &str| (u.to_owned(), d.to_owned());
    let pairs = vec![
        p("a", "2026-11-23"),
        p("b", "2026-11-23"),
        p("c", "2026-11-24"),
        p("a", "2026-11-16"),
    ];
    let weekly = vec![
        ("2026-11-23".to_owned(), "search".to_owned(), 1),
        ("2026-11-16".to_owned(), "search".to_owned(), 1),
    ];
    let last30 = vec![("search".to_owned(), 2)];
    let features = vec!["search".to_owned(), "invite".to_owned()];
    let (series, now) = fold_adoption(&pairs, &weekly, &last30, &features, today);
    let s = series.get("search").cloned().unwrap_or_default();
    assert!(
        s.last().map_or(0.0, |x| x.1) < s.first().map_or(0.0, |x| x.1),
        "1 of 3 this week, 1 of 1 the week before: {s:?}"
    );
    assert!(now.get("search").copied().unwrap_or_default() > 0.0);
    assert_eq!(
        now.get("invite").copied(),
        Some(0.0),
        "unused: 0, not unknown"
    );
}

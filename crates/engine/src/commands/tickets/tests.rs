use super::*;
use crate::FlagCohort;

fn t(baseline: f64, n: i64, up: bool) -> Ticket {
    Ticket {
        fp: "f".into(),
        kind: "error".into(),
        subject: "s".into(),
        title: "t".into(),
        metric: Some("error_rate".into()),
        baseline: Some(baseline),
        baseline_n: n,
        target: None,
        up,
        created: "2026-11-01".into(),
        issue: None,
        closed: None,
        shipped_in: None,
        outcome: None,
        flag: None,
        method: None,
    }
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// Shipped on Monday 2026-11-02: `before` are the weeks just before
/// it (oldest first), `after` the weeks from it.
fn series(before: &[f64], after: &[f64]) -> Vec<(String, f64)> {
    let ship = d("2026-11-02");
    let n = before.len() as i64;
    before
        .iter()
        .enumerate()
        .map(|(i, v)| (ship - chrono::Duration::weeks(n - i as i64), *v))
        .chain(
            after
                .iter()
                .enumerate()
                .map(|(i, v)| (ship + chrono::Duration::weeks(i as i64), *v)),
        )
        .map(|(m, v)| (m.format("%Y-%m-%d").to_string(), v))
        .collect()
}

fn on(weekly: &[(String, f64)], after: f64, n: i64, share: bool) -> Measures<'_> {
    Measures {
        after,
        after_n: n,
        share,
        weekly,
        shipped: Some(d("2026-11-02")),
        today: d("2026-11-16"),
        cohort: None,
    }
}

fn cohort(with: f64, with_n: i64, without: f64, without_n: i64) -> Cohort {
    Cohort {
        flag: "new-checkout".into(),
        with,
        with_n,
        without,
        without_n,
        p: crate::stats::two_proportions(with, with_n, without, without_n),
    }
}

const NORMAL: [f64; 8] = [100.0, 110.0, 90.0, 105.0, 95.0, 100.0, 108.0, 92.0];

#[test]
fn a_change_within_normal_weekly_variation_is_inconclusive() {
    let w = series(&NORMAL, &[104.0, 106.0]);
    let v = verdict(&t(100.0, 500, true), &on(&w, 105.0, 500, false));
    assert_eq!(
        (v.word.as_str(), v.method.as_str()),
        (INCONCLUSIVE, "control_chart")
    );
    assert!(
        v.evidence.contains("3σ") && v.evidence.contains("8 weeks before"),
        "{}",
        v.evidence
    );
}

#[test]
fn a_step_outside_the_band_is_judged_in_the_metrics_direction() {
    let up = series(&NORMAL, &[160.0, 165.0]);
    let down = series(&NORMAL, &[40.0, 45.0]);
    let judge = |w: &[(String, f64)], better_up: bool| {
        verdict(&t(100.0, 500, better_up), &on(w, 0.0, 500, false)).word
    };
    assert_eq!(judge(&up, true), IMPROVED);
    assert_eq!(judge(&up, false), WORSE);
    assert_eq!(judge(&down, true), WORSE);
    assert_eq!(judge(&down, false), IMPROVED);
    // Flat weeks before: a 5% band around their mean.
    let flat = [100.0; 6];
    assert_eq!(judge(&series(&flat, &[103.0, 104.0]), true), INCONCLUSIVE);
    assert_eq!(judge(&series(&flat, &[110.0, 112.0]), true), IMPROVED);
}

#[test]
fn the_chart_needs_four_weeks_before_and_waits_for_two_after() {
    // Three weeks of history: not a chart.
    let short = series(&[100.0, 110.0, 90.0], &[160.0, 165.0]);
    let v = verdict(&t(100.0, 500, true), &on(&short, 160.0, 500, false));
    assert_ne!(v.method, "control_chart");
    // Shipped on a Wednesday, checked 14 days later: one full week
    // after, so the check waits; a few days on, it can run.
    let w = series(&NORMAL, &[]);
    assert!(waiting(&w, d("2026-11-04"), d("2026-11-18")));
    assert!(!waiting(&w, d("2026-11-04"), d("2026-11-23")));
    assert!(
        !waiting(&[], d("2026-11-04"), d("2026-11-18")),
        "no series: no wait"
    );
}

#[test]
fn a_flagged_ticket_is_judged_on_its_cohorts() {
    let mut tk = t(0.2, 500, false);
    tk.flag = Some("new-checkout".into());
    // The weekly series shows nothing, the cohorts a clear drop in errors.
    let w = series(&NORMAL, &[100.0, 101.0]);
    let mut m = on(&w, 0.2, 500, true);
    m.cohort = Some(cohort(0.05, 2000, 0.10, 2000));
    let v = verdict(&tk, &m);
    assert_eq!((v.word.as_str(), v.method.as_str()), (IMPROVED, "flag"));
    assert!(v.evidence.contains("2000 sessions") && v.evidence.contains("p = "));
    // More errors with the flag: worse.
    m.cohort = Some(cohort(0.10, 2000, 0.05, 2000));
    assert_eq!(verdict(&tk, &m).word, WORSE);
    // The same gap on a handful of sessions: no test, inconclusive.
    m.cohort = Some(cohort(0.05, 10, 0.10, 2000));
    assert_eq!(verdict(&tk, &m).word, INCONCLUSIVE);
    // Not seen on any session: judged without the flag.
    m.cohort = Some(cohort(0.0, 0, 0.10, 2000));
    assert_ne!(verdict(&tk, &m).method, "flag");
}

#[test]
fn cohorts_measure_errors_and_tasks_only() {
    let c = FlagCohort {
        flag: "f".into(),
        with: crate::CohortStats {
            sessions: 40,
            error_rate: 0.1,
            ..Default::default()
        },
        without: crate::CohortStats::default(),
        p_errors: Some(0.5),
        p_tasks: None,
    };
    assert!(cohort_for(&c, "error_rate").is_some_and(|x| x.with_n == 40 && x.p == c.p_errors));
    assert!(cohort_for(&c, "avg_secs").is_none());
}

#[test]
fn without_history_shares_keep_the_test_and_other_metrics_are_inconclusive() {
    // Errors 20% → 10% on 500 sessions each: improved, significant.
    let v = verdict(&t(0.2, 500, false), &on(&[], 0.1, 500, true));
    assert_eq!(
        (v.word.as_str(), v.method.as_str()),
        (IMPROVED, "before_after")
    );
    assert!(v.evidence.contains("500 sessions") && v.evidence.contains("p = "));
    // 20% → 19%: within chance.
    assert_eq!(
        verdict(&t(0.2, 500, false), &on(&[], 0.19, 500, true)).word,
        INCONCLUSIVE
    );
    // Errors up: worse.
    assert_eq!(
        verdict(&t(0.1, 500, false), &on(&[], 0.2, 500, true)).word,
        WORSE
    );
    // Too few sessions for a test: no 5% rule any more.
    assert_eq!(
        verdict(&t(0.2, 10, false), &on(&[], 0.1, 10, true)).word,
        INCONCLUSIVE
    );
    // Not a share, no weekly series: inconclusive, even on a large move.
    let v = verdict(&t(100.0, 0, true), &on(&[], 150.0, 0, false));
    assert_eq!((v.word.as_str(), v.method.as_str()), (INCONCLUSIVE, "none"));
}

#[test]
fn the_learning_log_counts_the_new_words_and_reads_the_old_one() {
    let with = |w: &str| Ticket {
        outcome: Some((w.into(), String::new())),
        ..t(0.1, 1, false)
    };
    let mut all = vec![
        with(IMPROVED),
        with(WORSE),
        with("no effect"),
        with("no metric"),
        t(0.1, 1, false),
    ];
    read_old(&mut all);
    let l = Learning::of(&all);
    assert_eq!(l.inconclusive, 1, "no effect reads as inconclusive");
    assert_eq!(l.measured, l.improved + l.worse + l.inconclusive);
    assert!(
        l.measured < all.len(),
        "no metric and waiting are not measured"
    );
}

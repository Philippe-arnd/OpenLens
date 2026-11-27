//! Job check-ins (#609, D-086): the schedule learned from hourly, daily
//! and irregular jobs; a missed run alerts once and closes on the next
//! check-in; failed and slower runs; `expectEvery` instead of learning; a
//! removed job's incident closes quietly; the availability switch.

use openlens_engine::commands::alerts::{self, ProjectInput};
use openlens_engine::commands::jobs::{
    self, AvailabilityInput, CheckInInput, RemoveInput, alert_pass_at, learned_gap, record,
};
use openlens_engine::commands::{Caller, Command, Host};
use openlens_meta::jobs::JobRun;

use crate::maintenance::TestHost;

/// Runs from `start`, each gap after the one before.
fn runs(start: i64, gaps: &[i64]) -> Vec<JobRun> {
    let mut at = start;
    let mut out = vec![JobRun {
        at,
        ok: true,
        ms: None,
    }];
    for g in gaps {
        at += g;
        out.push(JobRun {
            at,
            ok: true,
            ms: None,
        });
    }
    out
}

fn input(slug: &str) -> CheckInInput {
    CheckInInput {
        slug: slug.into(),
        ..CheckInInput::default()
    }
}

fn open(h: &TestHost, project: &str, rule: &str) -> bool {
    h.meta().open_incident(project, rule).unwrap().is_some()
}

#[test]
fn the_schedule_is_learned_from_hourly_daily_and_irregular_jobs() {
    let t0 = 1_800_000_000;
    // Hourly, a few seconds late or early each time.
    let hourly = learned_gap(&runs(t0, &[3610, 3590, 3605, 3600, 3595])).unwrap();
    assert!((3500..=3700).contains(&hourly), "{hourly}");
    // Daily, minutes of jitter.
    let daily = learned_gap(&runs(t0, &[86_700, 86_100, 86_400, 86_520])).unwrap();
    assert!((85_800..=87_000).contains(&daily), "{daily}");
    // Irregular: a weekend pause and a manual rerun do not move it.
    let gaps = [3600, 3600, 2 * 86_400, 3600, 300];
    let irregular = learned_gap(&runs(t0, &gaps)).unwrap();
    let mean = gaps.iter().sum::<i64>() / gaps.len() as i64;
    assert!(irregular < mean, "the median, not the mean");
    assert!(irregular > 300 && irregular <= 3600, "{irregular}");
    // Too few check-ins: still learning.
    assert!(learned_gap(&runs(t0, &[3600, 3600])).is_none());
}

#[tokio::test]
async fn a_missed_run_alerts_once_and_closes_on_the_next_check_in() {
    let (h, p) = TestHost::new();
    let t0 = chrono::Utc::now().timestamp() - 20 * 3600;
    for k in 0..5 {
        record(h.meta(), &p.id, &input("hourly"), t0 + k * 3600).unwrap();
    }
    let last = t0 + 4 * 3600;
    let rule = "job:hourly:missed";
    alert_pass_at(&h, &p, last + 1800).await;
    assert!(!open(&h, &p.id, rule), "half an hour late is not missed");
    let late = last + 3600 + 1900;
    alert_pass_at(&h, &p, late).await;
    assert!(open(&h, &p.id, rule));
    alert_pass_at(&h, &p, late + 60).await;
    alert_pass_at(&h, &p, late + 2 * 3600).await;
    alert_pass_at(&h, &p, late + 5 * 3600).await;
    let sent = h.sent();
    assert_eq!(sent.len(), 1, "announced once, never reminded: {sent:?}");
    assert!(sent[0].0.contains("did not run") && sent[0].1.contains("hourly"));
    let back = late + 5 * 3600 + 30;
    record(h.meta(), &p.id, &input("hourly"), back).unwrap();
    alert_pass_at(&h, &p, back + 10).await;
    assert!(!open(&h, &p.id, rule), "the next check-in closes it");
    assert_eq!(h.sent().len(), 2, "and says so once");
}

#[tokio::test]
async fn a_failed_run_alerts_and_a_good_one_closes_it() {
    let (h, p) = TestHost::new();
    let t = chrono::Utc::now().timestamp() - 3600;
    let failed = CheckInInput {
        status: Some("error".into()),
        ..input("backup")
    };
    record(h.meta(), &p.id, &failed, t).unwrap();
    alert_pass_at(&h, &p, t + 10).await;
    alert_pass_at(&h, &p, t + 70).await;
    let sent = h.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].0.contains("failed"));
    record(h.meta(), &p.id, &input("backup"), t + 600).unwrap();
    alert_pass_at(&h, &p, t + 610).await;
    assert!(!open(&h, &p.id, "job:backup:failed"));
    assert_eq!(h.meta().incidents(&p.id, 0).unwrap().len(), 1);
}

#[tokio::test]
async fn a_run_twice_its_usual_time_alerts_as_slower() {
    let (h, p) = TestHost::new();
    let t0 = chrono::Utc::now().timestamp() - 10 * 3600;
    let timed = |ms| CheckInInput {
        ms: Some(ms),
        ..input("export")
    };
    for k in 0..4 {
        record(h.meta(), &p.id, &timed(1000 + k * 10), t0 + k * 3600).unwrap();
    }
    // Under twice the usual time: not slower.
    record(h.meta(), &p.id, &timed(1800), t0 + 4 * 3600).unwrap();
    let now = t0 + 4 * 3600 + 10;
    let listed = jobs::list(h.meta(), &p.id, now).unwrap();
    assert_eq!(listed.jobs[0].state, "ok");
    record(h.meta(), &p.id, &timed(2600), t0 + 5 * 3600).unwrap();
    let now = t0 + 5 * 3600 + 10;
    alert_pass_at(&h, &p, now).await;
    alert_pass_at(&h, &p, now + 60).await;
    let sent = h.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].0.contains("slower"));
    record(h.meta(), &p.id, &timed(1000), t0 + 6 * 3600).unwrap();
    alert_pass_at(&h, &p, t0 + 6 * 3600 + 10).await;
    assert!(!open(&h, &p.id, "job:export:slower"));
}

#[tokio::test]
async fn a_declared_gap_is_watched_from_the_first_check_in() {
    let (h, p) = TestHost::new();
    let t = chrono::Utc::now().timestamp() - 3600;
    let declared = CheckInInput {
        expect_every: Some(600),
        ..input("sync")
    };
    record(h.meta(), &p.id, &declared, t).unwrap();
    let before = jobs::list(h.meta(), &p.id, t + 800).unwrap();
    assert_eq!(before.jobs[0].state, "ok");
    assert!(before.jobs[0].declared);
    let after = jobs::list(h.meta(), &p.id, t + 1000).unwrap();
    assert_eq!(after.jobs[0].state, "missed");
    // A gap out of bounds is refused.
    let bad = CheckInInput {
        expect_every: Some(5),
        ..input("sync")
    };
    assert!(record(h.meta(), &p.id, &bad, t + 10).is_err());
}

#[tokio::test]
async fn a_removed_job_closes_its_incident_without_a_message() {
    let (h, p) = TestHost::new();
    let t = chrono::Utc::now().timestamp() - 3600;
    let failed = CheckInInput {
        status: Some("error".into()),
        ..input("old-job")
    };
    record(h.meta(), &p.id, &failed, t).unwrap();
    alert_pass_at(&h, &p, t + 10).await;
    alert_pass_at(&h, &p, t + 70).await;
    assert_eq!(h.sent().len(), 1);
    let remove = RemoveInput {
        project: p.id.clone(),
        slug: "old-job".into(),
    };
    jobs::Remove::run(&h, &Caller::default(), remove)
        .await
        .unwrap();
    assert!(!open(&h, &p.id, "job:old-job:failed"));
    assert_eq!(h.sent().len(), 1, "no 'back' message for a removed job");
    assert!(
        jobs::list(h.meta(), &p.id, t + 100)
            .unwrap()
            .jobs
            .is_empty()
    );
}

/// The Uptime page's availability over the last 24 hours.
async fn day_share(h: &TestHost, project: &str) -> Option<f64> {
    let input = ProjectInput {
        project: project.to_owned(),
    };
    alerts::GetUptime::run(h, &Caller::default(), input)
        .await
        .unwrap()
        .avail
        .0
}

#[tokio::test]
async fn jobs_count_in_availability_only_when_switched_on() {
    let (h, p) = TestHost::new();
    let now = chrono::Utc::now().timestamp();
    for k in 0..4 {
        let i = CheckInInput {
            status: Some(if k == 0 { "error" } else { "ok" }.into()),
            ..input("hourly")
        };
        record(h.meta(), &p.id, &i, now - 3600 * (k + 1)).unwrap();
    }
    assert!(day_share(&h, &p.id).await.is_none(), "off by default");
    let on = AvailabilityInput {
        project: p.id.clone(),
        on: true,
    };
    jobs::SetAvailability::run(&h, &Caller::default(), on)
        .await
        .unwrap();
    let a = day_share(&h, &p.id).await.unwrap();
    assert!(a < 1.0 && a > 0.5, "one failed run of four: {a}");
}

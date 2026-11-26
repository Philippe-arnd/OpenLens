//! Availability windows and the SLO (#560, D-078): a month with 50
//! minutes down spends a 99.9 % budget and says so once; hours with no
//! check are neither up nor down; planned maintenance is left out.

use chrono::{DateTime, Utc};
use openlens_engine::commands::alerts::Settings;
use openlens_engine::commands::uptime::{SetTarget, TargetInput, close_days};
use openlens_engine::commands::{Caller, Command, Error, Host};
use openlens_engine::slo::{self, Report, WindowFigure};
use openlens_meta::maintenance::NewWindow;
use openlens_meta::{Meta, Probe};

use crate::maintenance::TestHost;

const DAY: i64 = 86_400;

fn at(t: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(t, 0).unwrap()
}

/// Now, on a whole minute.
fn now() -> i64 {
    Utc::now().timestamp() / 60 * 60
}

/// A probe every minute over `[from, to)`, failing in `bad`; the days are
/// written as the server would, once they end.
fn probe(m: &Meta, project: &str, from: i64, to: i64, bad: std::ops::Range<i64>) {
    let mut t = from;
    while t < to {
        let ok = !bad.contains(&t);
        let p = Probe {
            ok,
            status: Some(if ok { 200 } else { 503 }),
            ms: Some(80 + (t / 60) % 40),
            error: (!ok).then_some("HTTP 503"),
        };
        m.record_check_in(project, "production", t, &p).unwrap();
        t += 60;
        if t % DAY == 600 {
            close_days(m, project, at(t));
        }
    }
    close_days(m, project, at(to));
}

fn down(m: &Meta, project: &str, from: i64, to: i64) {
    let id = m.start_incident(project, "down", from, "HTTP 503").unwrap();
    m.end_incident(id, to).unwrap();
}

fn window<'a>(r: &'a Report, label: &str) -> &'a WindowFigure {
    r.windows.iter().find(|w| w.label == label).unwrap()
}

#[tokio::test]
async fn a_month_with_50_minutes_down_spends_a_999_budget_and_alerts_once() {
    let (h, p) = TestHost::new();
    let m = h.meta();
    let now = now();
    let start = now - 30 * DAY + 3600;
    // 50 minutes down twelve days ago (counted from the daily rows).
    let (s, e) = (now - 12 * DAY, now - 12 * DAY + 50 * 60);
    // Recorded as it happens: before the days are written.
    down(m, &p.id, s, e);
    probe(m, &p.id, start, now, s..e);
    let input = TargetInput {
        project: p.id.clone(),
        environment: None,
        target: Some(0.999),
    };
    SetTarget::run(&h, &Caller::default(), input).await.unwrap();

    let r = slo::report(m, &p.id, "production", &p.timezone, at(now)).unwrap();
    let o = r.slo.clone().unwrap();
    assert!(o.spent(), "50 min down spends the 43 min of 99.9%: {o:?}");
    assert!(o.spent_seconds > o.budget_seconds);
    assert_eq!(o.left_seconds, 0);
    assert!(o.availability.unwrap() < o.target);
    assert!(!o.burning_fast(), "nothing went down in the last day");
    let d30 = window(&r, "30d");
    assert_eq!(
        d30.availability, o.availability,
        "the SLO reads the 30 days"
    );
    assert_eq!(d30.incidents, 1);
    assert!(window(&r, "7d").availability > d30.availability);
    assert_eq!(window(&r, "7d").incidents, 0);

    let s = Settings::default();
    for minutes in [0, 5, 10, 70] {
        slo::alert_pass(&h, &p, &s, at(now + minutes * 60)).await;
    }
    let spent: Vec<_> = h
        .sent()
        .into_iter()
        .filter(|(t, _)| t.contains("error budget"))
        .collect();
    assert_eq!(spent.len(), 1, "one alert, no reminder: {spent:?}");
    assert!(spent[0].0.contains("spent"));

    // No target, no SLO block.
    let clear = TargetInput {
        project: p.id.clone(),
        environment: None,
        target: None,
    };
    SetTarget::run(&h, &Caller::default(), clear).await.unwrap();
    let r = slo::report(m, &p.id, "production", &p.timezone, at(now)).unwrap();
    assert!(r.slo.is_none());
}

#[tokio::test]
async fn hours_with_no_check_are_neither_up_nor_down() {
    let (h, p) = TestHost::new();
    let m = h.meta();
    let now = now();
    let gap = (now - 14 * 3600, now - 8 * 3600);
    // Up, then failing for 3 minutes as OpenLens stops; back 6 hours later.
    // The incident stayed open over the gap, until the first check after.
    down(m, &p.id, gap.0 - 120, gap.1 + 60);
    probe(m, &p.id, now - DAY, gap.0, gap.0 - 180..gap.0);
    probe(m, &p.id, gap.1, now, 0..0);

    let r = slo::report(m, &p.id, "production", &p.timezone, at(now)).unwrap();
    let d = window(&r, "24h");
    assert!(
        d.unmeasured_seconds >= gap.1 - gap.0,
        "the gap is not measured: {d:?}"
    );
    assert!(d.measured_seconds + d.unmeasured_seconds <= DAY);
    let a = d.availability.unwrap();
    assert!(a < 1.0, "the minutes down that were checked count");
    assert!(
        a > 0.99,
        "the six hours with no check do not count as down: {a}"
    );
}

#[tokio::test]
async fn planned_maintenance_is_left_out_and_said() {
    let (h, p) = TestHost::new();
    let m = h.meta();
    let now = now();
    let (s, e) = (now - 5 * 3600, now - 4 * 3600);
    down(m, &p.id, s, e);
    probe(m, &p.id, now - DAY, now, s..e);
    let without = slo::report(m, &p.id, "production", &p.timezone, at(now)).unwrap();
    let comps = vec!["production".to_owned()];
    m.add_maintenance(
        &p.id,
        &NewWindow {
            starts_at: s - 60,
            ends_at: e + 60,
            components: &comps,
            text: "Database migration",
            created_by: "test",
            now: s - 3600,
        },
    )
    .unwrap();
    let with = slo::report(m, &p.id, "production", &p.timezone, at(now)).unwrap();
    let (a, b) = (window(&without, "24h"), window(&with, "24h"));
    assert!(b.availability > a.availability);
    assert_eq!(b.availability, Some(1.0));
    assert!(b.maintenance_seconds > 0 && a.maintenance_seconds == 0);
    assert!(b.measured_seconds < a.measured_seconds);
    assert_eq!(b.incidents, 0, "an incident of planned work is not counted");
}

#[tokio::test]
async fn a_target_is_bounded() {
    let (h, p) = TestHost::new();
    for t in [0.4, 1.0, 99.9, f64::NAN] {
        let input = TargetInput {
            project: p.id.clone(),
            environment: Some("staging".into()),
            target: Some(t),
        };
        let r = SetTarget::run(&h, &Caller::default(), input).await;
        assert!(matches!(r, Err(Error::Invalid(_))), "{t} is refused");
    }
    let input = TargetInput {
        project: p.id.clone(),
        environment: Some("bad env!".into()),
        target: Some(0.99),
    };
    assert!(SetTarget::run(&h, &Caller::default(), input).await.is_err());
    assert!(slo::targets(h.meta(), &p.id).is_empty());
}

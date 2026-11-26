//! Uptime history (#559, D-075): the days a range shows, and the days
//! written around a restart.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use openlens_engine::commands::uptime::{close_days, days, range, rebuild_recent};
use openlens_meta::{Meta, Probe};

const DAY: i64 = 86_400;

fn at(t: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(t, 0).unwrap()
}

fn midnight() -> i64 {
    Utc.with_ymd_and_hms(2026, 5, 10, 0, 0, 0)
        .unwrap()
        .timestamp()
}

/// A probe every minute of `env` over `[from, to)`, failing in `bad`.
fn probe(m: &Meta, env: &str, from: i64, to: i64, bad: std::ops::Range<i64>) {
    let mut t = from;
    while t < to {
        let ok = !bad.contains(&t);
        let p = Probe {
            ok,
            status: Some(if ok { 200 } else { 503 }),
            ms: Some(80 + (t / 60) % 40),
            error: (!ok).then_some("HTTP 503"),
        };
        m.record_check_in("p", env, t, &p).unwrap();
        t += 60;
    }
}

#[test]
fn a_restart_in_the_middle_of_a_day_neither_loses_nor_doubles_it() {
    let m = Meta::in_memory().unwrap();
    let t0 = midnight();
    let day = NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
    probe(&m, "production", t0, t0 + DAY / 2, 0..0);
    // Started at noon, then restarted at once.
    rebuild_recent(&m, "p", at(t0 + DAY / 2));
    let half = m.uptime_days("p", "production", day, day).unwrap();
    rebuild_recent(&m, "p", at(t0 + DAY / 2 + 60));
    probe(&m, "production", t0 + DAY / 2, t0 + DAY, 0..0);
    // The day closes after midnight.
    close_days(&m, "p", at(t0 + DAY + 300));
    let rows = m.uptime_days("p", "production", day, day).unwrap();
    assert_eq!(rows.len(), 1, "one row for the day");
    assert!(rows[0].checks > half[0].checks, "the afternoon is counted");
    let raw = m.checks("p", t0).unwrap();
    let raw_day = raw.iter().filter(|c| c.0 < t0 + DAY).count() as i64;
    assert_eq!(rows[0].checks, raw_day, "every check once");
}

#[test]
fn days_before_the_first_check_are_not_green_and_down_time_colours_a_day() {
    let m = Meta::in_memory().unwrap();
    let t0 = midnight();
    // Two days of checks; a 2-hour incident on the second.
    probe(&m, "production", t0, t0 + DAY, 0..0);
    let (s, e) = (t0 + DAY + 3600, t0 + DAY + 3 * 3600);
    probe(&m, "production", t0 + DAY, t0 + 2 * DAY, s..e);
    let id = m.start_incident("p", "down", s, "HTTP 503").unwrap();
    m.end_incident(id, e).unwrap();
    let now = at(t0 + 2 * DAY + 600);
    close_days(&m, "p", now);
    let (from, to) = range(Some("2026-05-01"), Some("2026-05-11"), now.date_naive()).unwrap();
    let d = days(&m, "p", "production", from, to, now).unwrap();
    assert_eq!(d.days.len(), 11, "every day of the range is listed");
    let before = &d.days[..9];
    assert!(
        before
            .iter()
            .all(|x| x.state == "none" && x.stats.is_none())
    );
    assert_eq!(d.days[9].state, "up");
    assert_eq!(d.days[10].state, "down");
    let day_avail = d.days[10].availability.unwrap();
    assert!(day_avail < d.days[9].availability.unwrap());
    let raw: Vec<_> = m
        .checks("p", t0 + DAY)
        .unwrap()
        .into_iter()
        .filter(|c| c.0 < t0 + 2 * DAY)
        .collect();
    let ok = raw.iter().filter(|c| c.1).count() as f64;
    assert_eq!(
        Some(day_avail),
        Some(ok / raw.len() as f64),
        "a day's availability is its raw checks'"
    );
    assert!(d.up_seconds < d.measured_seconds);
}

#[test]
fn an_environment_keeps_its_own_days_and_ranges_are_bounded() {
    let m = Meta::in_memory().unwrap();
    let t0 = midnight();
    probe(&m, "staging", t0, t0 + DAY, t0..t0 + 600);
    let id = m
        .start_incident("p", "env:staging:down", t0 + 60, "HTTP 503")
        .unwrap();
    m.end_incident(id, t0 + 600).unwrap();
    let now = at(t0 + DAY + 60);
    close_days(&m, "p", now);
    assert!(
        m.checks("p", t0).unwrap().is_empty(),
        "production's checks are its own"
    );
    let day = now.date_naive().pred_opt().unwrap();
    let d = days(&m, "p", "staging", day, day, now).unwrap();
    assert_eq!(d.days[0].state, "partial");
    let today = now.date_naive();
    assert!(range(Some("2025-01-01"), Some("2026-05-10"), today).is_err());
    assert!(range(Some("2026-05-10"), Some("2026-05-01"), today).is_err());
    assert!(range(Some("0000-01-01"), None, today).is_err());
    assert!(range(None, Some("not-a-day"), today).is_err());
}

//! Uptime history (#559, D-075): raw checks for a week, the checks around
//! an incident for a year, one row per day for good.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use openlens_meta::Meta;

const DAY: i64 = 86_400;

fn day_of(t: i64) -> NaiveDate {
    DateTime::from_timestamp(t, 0).unwrap().date_naive()
}

/// Eight days of a probe every minute from a UTC midnight, each day
/// closed as it ends, with a 30-minute *down* incident at noon on the
/// first day. Returns the store, the first minute and the incident.
fn eight_days() -> (Meta, i64, (i64, i64)) {
    let m = Meta::in_memory().unwrap();
    let t0 = Utc
        .with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
        .unwrap()
        .timestamp();
    let (start, end) = (t0 + 12 * 3600, t0 + 12 * 3600 + 1800);
    let mut id = None;
    for i in 0..8 * 1440 {
        let at = t0 + i * 60;
        if at > t0 && at % DAY == 0 {
            m.rebuild_uptime_day("p", "production", day_of(at - 1), at)
                .unwrap();
        }
        let down = (start..end).contains(&at);
        let err = down.then_some("HTTP 503");
        let status = Some(if down { 503 } else { 200 });
        m.record_check("p", at, !down, status, Some(100 + i % 50), err)
            .unwrap();
        if at == start {
            id = Some(m.start_incident("p", "down", at, "HTTP 503").unwrap());
        }
        if at == end {
            m.end_incident(id.unwrap(), at).unwrap();
        }
    }
    (m, t0, (start, end))
}

#[test]
fn after_eight_days_only_the_checks_around_the_incident_and_the_days_remain() {
    let (m, t0, (start, end)) = eight_days();
    let last = t0 + 8 * DAY - 60;
    let purged_before = last - 7 * DAY;
    let old: Vec<i64> = m
        .checks("p", t0)
        .unwrap()
        .into_iter()
        .map(|c| c.0)
        .filter(|at| *at < purged_before)
        .collect();
    assert!(!old.is_empty(), "the checks around the incident are kept");
    assert!(
        old.iter()
            .all(|at| *at >= start - 3600 && *at <= end + 3600),
        "every older check left is within an hour of the incident"
    );
    assert!(
        old.contains(&(start - 3600)) && old.contains(&(end + 3600)),
        "the checks one hour before and one hour after are still readable"
    );
    let days = m
        .uptime_days("p", "production", day_of(t0), day_of(last))
        .unwrap();
    assert!(days.len() >= 7, "every closed day has its row");
    let first = days.first().unwrap();
    assert_eq!(first.day, day_of(t0).format("%Y-%m-%d").to_string());
    assert!(first.failed > 0 && first.down_seconds > 0 && first.incidents > 0);
    assert!(
        days.iter()
            .skip(1)
            .all(|d| d.down_seconds == 0 && d.availability() > first.availability()),
        "only the incident's day lost time"
    );
}

#[test]
fn a_closed_day_matches_its_raw_checks() {
    let (m, t0, _) = eight_days();
    let day = t0 + 6 * DAY;
    let raw: Vec<_> = m
        .checks("p", day)
        .unwrap()
        .into_iter()
        .filter(|c| c.0 < day + DAY)
        .collect();
    let row = m
        .uptime_days("p", "production", day_of(day), day_of(day))
        .unwrap()
        .pop()
        .unwrap();
    let ok = raw.iter().filter(|c| c.1).count() as f64;
    assert_eq!(row.checks, raw.len() as i64);
    assert_eq!(row.availability(), Some(ok / raw.len() as f64));
    let max = raw.iter().filter_map(|c| c.3).max();
    assert_eq!(row.max_ms, max);
    assert!(row.p50_ms <= row.p95_ms && row.p95_ms <= row.max_ms);
}

#[test]
fn incidents_page_by_id_and_a_deleted_project_loses_its_days() {
    let m = Meta::in_memory().unwrap();
    let p = m.create_project("Uptime App", None).unwrap();
    for i in 0..5 {
        m.start_incident(&p.id, "down", 1000 + i, "x").unwrap();
    }
    let first = m.incidents_page(&p.id, None, 3).unwrap();
    assert_eq!(first.len(), 3);
    let next = m
        .incidents_page(&p.id, first.last().map(|i| i.id), 3)
        .unwrap();
    assert_eq!(next.len(), 2, "the second page holds the rest");
    assert!(next.iter().all(|i| first.iter().all(|f| f.id > i.id)));
    m.record_check(&p.id, 1000, true, Some(200), Some(5), None)
        .unwrap();
    m.rebuild_uptime_day(&p.id, "production", day_of(1000), 2000)
        .unwrap();
    m.delete_project(&p.id).unwrap();
    assert!(
        m.uptime_days(&p.id, "production", day_of(0), day_of(2000))
            .unwrap()
            .is_empty()
    );
}

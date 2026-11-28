//! The route-stats window follows the host too (#628, D-091): 1, 5 or 10
//! minutes, chosen with the capacity (hourly), advertised to the server
//! SDKs in the capture response (`x-openlens-stats-window`).
//!
//! The window is the length of one `$request_stats` and `$client_stats`
//! data point (`$runtime_stats` come every 10 minutes whatever it is,
//! #726, D-100). A finer window stores more points for the
//! same requests, so it is chosen like extra recordings are: the bytes the
//! stat events would take over the fixed 400 days, beyond what 10-minute
//! windows take, must fit in the room the budget leaves after the floor.
//! And however large the disk, a finer window never stores more points a
//! day per project than 10-minute windows do at the SDK's bound of 500
//! keys ([`MAX_POINTS_PER_DAY`]), so the reads (up to 90 days of points)
//! never get heavier than they could already be: many active routes keep
//! a coarser window.
//!
//! 10 minutes is the default and the floor: a small host, or one whose
//! route stats are not counted yet, keeps today's behaviour.

use openlens_query::QueryEngine;
use serde::{Deserialize, Serialize};

use super::{Cap, EVENT_RETENTION_DAYS, ProjectUse, bytes_text};

/// The windows the server may advertise, seconds, finest first.
pub const WINDOWS_S: [i64; 3] = [60, 300, 600];
/// The default and coarsest window, seconds: the SDK's `STATS_WINDOW_MS`.
pub const DEFAULT_WINDOW_S: i64 = 600;
/// The finest window, seconds.
pub const FINEST_WINDOW_S: i64 = 60;
/// Stats points a project may store a day at most with a finer window:
/// 500 keys (the SDK's bound per window) in 10-minute windows.
pub const MAX_POINTS_PER_DAY: f64 = 500.0 * 144.0;
/// The response header that carries the window to the server SDKs.
pub const HEADER: &str = "x-openlens-stats-window";

const DAY_S: f64 = 86_400.0;

/// What a project's route stats were over the last 24 hours (counted on the
/// maintenance pass, with the disk measurement).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsLoad {
    /// Stats series open at once, on average: the sum of the stats points'
    /// `$window_s` over the day, divided by a day. Routes × status codes,
    /// external hosts and processes; the same whatever the window.
    pub keys: u64,
    /// Stats points stored in the day (at the window then in force).
    pub points_per_day: u64,
    /// Events stored in the day, stats points included.
    pub events_per_day: u64,
}

/// Is `w` a window the server may advertise (and `statsWindowSeconds` may set)?
#[must_use]
pub fn valid(w: i64) -> bool {
    WINDOWS_S.contains(&w)
}

/// `1-minute`, `5-minute` or `10-minute`.
#[must_use]
pub fn text(w: i64) -> &'static str {
    match w {
        60 => "1-minute",
        300 => "5-minute",
        _ => "10-minute",
    }
}

/// The SQL of a stored stats point's `$window_s` (seconds, 0 when absent),
/// read from the `properties` column named `col`.
#[must_use]
pub fn window_s_sql(col: &str) -> String {
    format!("COALESCE(CAST(regexp_match({col}, '\"\\$window_s\":([0-9]{{1,7}})')[1] AS BIGINT), 0)")
}

/// Stats points a day for `keys` series at window `w`.
fn points_a_day(keys: u64, w: i64) -> f64 {
    keys as f64 * DAY_S / w as f64
}

/// Bytes one event of the project takes on the disk, on average.
fn bytes_per_event(p: &ProjectUse) -> f64 {
    match p.stats {
        Some(l) if l.events_per_day > 0 => p.event_bytes_per_day as f64 / l.events_per_day as f64,
        _ => 0.0,
    }
}

/// The project's event bytes a day as they would be with 10-minute
/// windows: what the floor counts, so a finer window in force does not
/// raise the floor and push itself back out (no feedback).
pub(super) fn event_bytes_at_default(p: &ProjectUse) -> f64 {
    let Some(l) = p.stats else {
        return p.event_bytes_per_day as f64;
    };
    let over = (l.points_per_day as f64 - points_a_day(l.keys, DEFAULT_WINDOW_S)).max(0.0);
    (p.event_bytes_per_day as f64 - bytes_per_event(p) * over).max(0.0)
}

/// The window when it cannot be derived: 10 minutes, for `why`.
pub(super) fn default_cap(why: &str) -> Cap {
    Cap {
        value: DEFAULT_WINDOW_S,
        floor: FINEST_WINDOW_S,
        ceiling: DEFAULT_WINDOW_S,
        derived: false,
        reason: format!("10-minute route stats, the default: {why}"),
    }
}

/// The window for these projects, with `room` bytes left in the data
/// budget after the floor, and the bytes it takes beyond 10-minute stats
/// over the 400 days.
pub(super) fn choose(projects: &[ProjectUse], room: f64) -> (Cap, f64) {
    if projects.iter().any(|p| p.stats.is_none()) {
        return (
            default_cap(
                "the route stats are not counted yet (the maintenance pass counts them hourly)",
            ),
            0.0,
        );
    }
    let keys = projects
        .iter()
        .filter_map(|p| p.stats.map(|l| l.keys))
        .max()
        .unwrap_or(0);
    if keys == 0 {
        return (
            default_cap("no route stats in the last 24 hours, so their size is not known"),
            0.0,
        );
    }
    let cost = |w: i64| -> f64 {
        projects
            .iter()
            .filter_map(|p| {
                p.stats.map(|l| {
                    bytes_per_event(p)
                        * (points_a_day(l.keys, w) - points_a_day(l.keys, DEFAULT_WINDOW_S))
                        * EVENT_RETENTION_DAYS as f64
                })
            })
            .sum()
    };
    let cap = |value: i64, derived: bool, reason: String| Cap {
        value,
        floor: FINEST_WINDOW_S,
        ceiling: DEFAULT_WINDOW_S,
        derived,
        reason,
    };
    let mut why = String::new();
    for w in [FINEST_WINDOW_S, 300] {
        let c = cost(w);
        if points_a_day(keys, w) > MAX_POINTS_PER_DAY {
            why = format!(
                "{} ones would store more than {MAX_POINTS_PER_DAY:.0} points a day for the {keys} active routes and status codes of the busiest project",
                text(w)
            );
            continue;
        }
        if c > 0.0 && c > room {
            why = format!(
                "{} ones would take {} more over 400 days, and {} is left after the floor",
                text(w),
                bytes_text(c as u64),
                bytes_text(room.max(0.0) as u64)
            );
            continue;
        }
        let not_finer = if why.is_empty() {
            String::new()
        } else {
            format!(" (not finer: {why})")
        };
        let reason = format!(
            "{} route stats: {keys} active routes and status codes in the busiest project, {} more over 400 days than 10-minute ones, within the {} left after the floor{not_finer}",
            text(w),
            bytes_text(c as u64),
            bytes_text(room.max(0.0) as u64)
        );
        return (cap(w, true, reason), c);
    }
    (
        cap(
            DEFAULT_WINDOW_S,
            true,
            format!("10-minute route stats: {why}"),
        ),
        0.0,
    )
}

/// Count a project's route stats over the last 24 hours, for the window.
///
/// # Errors
/// A failed query.
pub async fn stats_load(q: &QueryEngine, project: &str) -> crate::Result<StatsLoad> {
    let since = (chrono::Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%d");
    let win = window_s_sql("properties");
    let stats = format!(
        "event IN ('{}', '{}', '{}')",
        openlens_event::names::REQUEST_STATS,
        openlens_event::names::CLIENT_STATS,
        openlens_event::names::RUNTIME_STATS,
    );
    let sql = format!(
        "SELECT count(*) AS events, \
         sum(CASE WHEN {stats} AND {win} > 0 THEN 1 ELSE 0 END) AS points, \
         sum(CASE WHEN {stats} THEN {win} ELSE 0 END) AS window_s \
         FROM raw_events WHERE date >= '{since}' AND timestamp >= now() - INTERVAL '1 day'"
    );
    let r = q.sql(project, &sql).await?;
    let get = |k: &str| -> u64 {
        r.rows
            .first()
            .map_or(0, |x| u64::try_from(crate::int(x, k)).unwrap_or(0))
    };
    Ok(StatsLoad {
        keys: get("window_s").div_ceil(86_400),
        points_per_day: get("points"),
        events_per_day: get("events"),
    })
}

#[cfg(test)]
mod tests {
    use super::super::{Inputs, Usage, Volume, derive};
    use super::*;

    const GB: u64 = 1_000_000_000;
    /// Other events a day, and the bytes one event takes.
    const EVENTS: u64 = 500_000;
    const BYTES: u64 = 100;
    const RECORDING: u64 = 1_000_000;

    /// A host with a volume of `total` bytes and one project with `keys`
    /// stats series, measured while `measured` was the window in force
    /// (`None`: its stats are not counted yet).
    fn host(total: u64, keys: u64, measured: Option<i64>) -> Inputs {
        let points = measured.map_or(0, |w| keys * 86_400 / w as u64);
        let per_day = (EVENTS + points) * BYTES;
        let replays_day = RECORDING * 300;
        let event_bytes = per_day * 400;
        let replay_bytes = replays_day * 30;
        let used = event_bytes + replay_bytes;
        Inputs {
            volume: Some(Volume {
                total,
                available: total.saturating_sub(used),
            }),
            usage: Some(Usage {
                used,
                upgrade_copy: 0,
                projects: vec![ProjectUse {
                    project: "p".into(),
                    event_bytes,
                    replay_bytes,
                    event_bytes_per_day: per_day,
                    replay_bytes_per_day: replays_day,
                    replay_bytes_per_recording: RECORDING,
                    stats: measured.map(|_| StatsLoad {
                        keys,
                        points_per_day: points,
                        events_per_day: EVENTS + points,
                    }),
                }],
                measured_at: "2026-11-27T00:00:00Z".into(),
            }),
            budget_override: None,
        }
    }

    fn window(i: &Inputs) -> Cap {
        derive(i).stats_window
    }

    #[test]
    fn a_small_disk_or_many_routes_keep_10_minutes_and_a_large_disk_with_few_routes_gets_1() {
        // A disk that only just holds the floor.
        let small = window(&host(40 * GB, 20, Some(600)));
        assert_eq!(small.value, DEFAULT_WINDOW_S, "{}", small.reason);
        assert!(small.derived && small.reason.contains("left after the floor"));
        // A large disk and a handful of routes: the finest.
        let large = window(&host(2_000 * GB, 20, Some(600)));
        assert_eq!(large.value, FINEST_WINDOW_S, "{}", large.reason);
        assert!(large.reason.starts_with("1-minute route stats"));
        // The same disk, more routes: coarser, never finer.
        let more = window(&host(2_000 * GB, 100, Some(600)));
        assert!(more.value > large.value, "{}", more.reason);
        let many = window(&host(2_000 * GB, 2_000, Some(600)));
        assert!(many.value >= more.value);
        assert_eq!(many.value, DEFAULT_WINDOW_S, "{}", many.reason);
        // Whatever it is, it is one the SDKs accept.
        for c in [&small, &large, &more, &many] {
            assert!(valid(c.value) && c.floor <= c.value && c.value <= c.ceiling);
        }
    }

    #[test]
    fn unknown_keeps_10_minutes_and_says_why() {
        let not_counted = window(&host(2_000 * GB, 20, None));
        assert_eq!(not_counted.value, DEFAULT_WINDOW_S);
        assert!(!not_counted.derived && not_counted.reason.contains("not counted"));
        let mut h = host(2_000 * GB, 20, Some(600));
        h.volume = None;
        let no_volume = window(&h);
        assert_eq!(no_volume.value, DEFAULT_WINDOW_S);
        assert!(!no_volume.derived);
        let quiet = window(&host(2_000 * GB, 0, Some(600)));
        assert_eq!(
            quiet.value, DEFAULT_WINDOW_S,
            "no route stats: nothing to refine"
        );
        let mut h = host(2_000 * GB, 20, Some(600));
        h.usage = None;
        assert_eq!(window(&h).value, DEFAULT_WINDOW_S);
    }

    #[test]
    fn a_finer_window_in_force_does_not_push_itself_out() {
        // Measured at 10 minutes or at 1 minute, the same host gets the
        // same window and the same recordings a day.
        let total = 2_000 * GB;
        let at_10 = derive(&host(total, 20, Some(600)));
        let at_1 = derive(&host(total, 20, Some(60)));
        assert_eq!(at_10.stats_window.value, at_1.stats_window.value);
        let diff = (at_10.replays_per_day.value - at_1.replays_per_day.value).abs();
        assert!(
            diff <= 1,
            "{} vs {}",
            at_10.replays_per_day.value,
            at_1.replays_per_day.value
        );
        // A finer window takes its bytes from the recordings' room.
        let none = derive(&host(total, 0, Some(600)));
        assert!(at_10.replays_per_day.value <= none.replays_per_day.value);
    }
}

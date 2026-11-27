//! The board's release window (#323, D-055): the board pages read their
//! period blocks (sessions, tasks, errors, the daily active series,
//! features, roles and role × feature) since the current release of the
//! environment, and compare them with the same length before it.
//!
//! - **Current release**: the newest live release of the environment
//!   (D-049, `releases.list`), or the newest rolling out when none is live.
//! - **Window**: from its first event to now, 90 days at most.
//! - **Comparison**: the same length just before the window starts.
//! - **No release**: the last 14 days against the 14 before.
//!
//! Alerts, goals, the digest, Ask and shared boards keep the fixed windows
//! ([`Window::Fixed`]), so their numbers do not move with each deploy.
//!
//! A board page or the API may also ask for a date range or a past
//! release instead (#513, D-060, [`crate::board_range`]).

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};

use super::*;
use crate::releases::{Release, State};

/// The longest release window, in days.
pub const MAX_DAYS: i64 = 90;
/// The window when the environment has no release, in days.
pub const FALLBACK_DAYS: i64 = 14;

pub(crate) const DAY: i64 = 86_400;

/// The numbers compared between the window and the period before it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Period {
    /// Distinct users with an event in the period (identified ones, as
    /// the user KPIs count them).
    pub users: i64,
    /// Session numbers of the period.
    pub sessions: Sessions,
}

/// The board's release window and its comparison (D-055).
#[derive(Debug, Clone, Default, Serialize)]
pub struct BoardWindow {
    /// The release the window starts at; none: the environment has no
    /// release, and the window is the last 14 days.
    pub release: Option<String>,
    /// When the window starts (UTC, `YYYY-MM-DD HH:MM`).
    pub since: String,
    /// When it ends (UTC, `YYYY-MM-DD HH:MM`); none: now (D-060).
    pub until: Option<String>,
    /// Its length in days, rounded up.
    pub days: i64,
    /// The release is at least as old as the longest window (90 days),
    /// which the window was cut to.
    pub capped: bool,
    /// As shown: "Since v1.2 (5 days)", or "Last 14 days".
    pub label: String,
    /// The window's numbers.
    pub current: Period,
    /// The same length just before the window.
    pub previous: Period,
    /// The environment's releases a window can be asked since, newest
    /// first (`releases.list`): the board page's picker (D-060).
    pub releases: Vec<String>,
    /// What a date range or a past release read (D-061): days from the
    /// rollups and from the events, bytes and a rough time; none for the
    /// current release's window.
    pub estimate: Option<crate::Estimate>,
}

/// A window with the spans its queries read.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    /// What the board returns (the periods are filled by the board).
    pub window: BoardWindow,
    /// From the window's start to now.
    pub current: Span,
    /// The same length before it.
    pub previous: Span,
}

/// The window of the period blocks `f` asks for at `now`; none: the
/// fixed windows. The release window (and a past release's) needs no
/// release filter: a board filtered on one release already reads that
/// release, and keeps the fixed windows (D-055). A date range applies
/// with any filter (D-060).
///
/// # Errors
/// A failed query, an invalid time zone, a range that is not allowed or
/// a release the environment does not show.
pub(crate) async fn resolve(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    now: DateTime<Utc>,
) -> Result<Option<Resolved>> {
    // A range is checked before any query runs.
    let range = match &f.window {
        Window::Fixed => return Ok(None),
        Window::Release | Window::Since { .. } if f.release.is_some() => return Ok(None),
        Window::Range { from, to } => Some(crate::board_range::range(&f.tz, *from, *to, now)?),
        Window::Release | Window::Since { .. } => None,
    };
    let rs = crate::releases::releases(q, project, &f.tz, Some(&f.environment), &[], &[]).await?;
    let mut r = match (range, &f.window) {
        (Some(r), _) => r,
        (None, Window::Since { release }) => {
            crate::board_range::since(&rs.releases, &f.environment, &f.tz, release, now)?
        }
        (None, _) => window(current_release(&rs.releases, &f.environment, &f.tz), now),
    };
    r.window.releases = rs
        .releases
        .iter()
        .filter(|x| x.environment == f.environment)
        .map(|x| x.release.clone())
        .collect();
    Ok(Some(r))
}

/// The current release of `environment` and its first event: the newest
/// live release, or the newest rolling out when none is live. `tz` is the
/// zone `releases` wrote its times in.
pub(crate) fn current_release(
    releases: &[Release],
    environment: &str,
    tz: &str,
) -> Option<(String, DateTime<Utc>)> {
    let newest = |state: State| {
        releases
            .iter()
            .filter(|r| r.environment == environment && r.state == state)
            .filter_map(|r| Some((r.release.clone(), parse_local(&r.first_seen, tz)?)))
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
    };
    newest(State::Live).or_else(|| newest(State::RollingOut))
}

/// A `YYYY-MM-DD HH:MM` wall-clock time of the zone `tz` (UTC when it is
/// not a zone), as UTC; the earlier one when the clock was set back.
pub(crate) fn parse_local(s: &str, tz: &str) -> Option<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").ok()?;
    let zone = tz.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC);
    zone.from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
}

/// `d` in whole seconds, rounded up: a span of that many seconds before
/// `now` starts at or before the instant `d` before it (#568).
pub(crate) fn whole_seconds_up(d: Duration) -> i64 {
    let whole = d.num_seconds();
    whole + i64::from(d > Duration::seconds(whole))
}

/// The window at `now` from the current release `(name, first event)`, or
/// the last 14 days without one.
pub(crate) fn window(current: Option<(String, DateTime<Utc>)>, now: DateTime<Utc>) -> Resolved {
    let (release, len, capped) = match current {
        None => (None, FALLBACK_DAYS * DAY, false),
        Some((name, start)) => {
            // At least a minute, so a release first seen "now" (or a clock
            // ahead of ours) still gives a span. Rounded up to the second,
            // so the window starts at or before the release's first event
            // and holds it (#568).
            let age = whole_seconds_up(now - start).max(60);
            let capped = age >= MAX_DAYS * DAY;
            (Some(name), age.min(MAX_DAYS * DAY), capped)
        }
    };
    let days = len / DAY + i64::from(len % DAY != 0);
    let plural = if days == 1 { "day" } else { "days" };
    let label = match &release {
        None => format!("Last {days} days"),
        Some(r) if capped => format!("Since {r} (the last {days} days)"),
        Some(r) => format!("Since {r} ({days} {plural})"),
    };
    Resolved {
        window: BoardWindow {
            release,
            since: (now - Duration::seconds(len))
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            days,
            capped,
            label,
            ..BoardWindow::default()
        },
        current: Span { start: len, end: 0 },
        previous: Span {
            start: 2 * len,
            end: len,
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn rel(name: &str, env: &str, state: State, first_seen: &str) -> Release {
        Release {
            release: name.into(),
            environment: env.into(),
            state,
            share: 0.0,
            first_seen: first_seen.into(),
            last_seen: String::new(),
            users: 1,
            events: 1,
            staging_secs: None,
            staged_as: None,
            deployed_at: None,
            deploy: None,
            verdict: None,
            reasons: Vec::new(),
            runtime: None,
        }
    }

    #[test]
    fn the_newest_live_release_of_the_environment_starts_the_window() {
        let rs = [
            rel("v1", "production", State::Superseded, "2026-01-01 10:00"),
            rel("v2", "production", State::Live, "2026-01-05 10:00"),
            rel("v3", "production", State::RollingOut, "2026-01-08 10:00"),
            rel("v9", "staging", State::Live, "2026-01-09 10:00"),
        ];
        let (name, start) = current_release(&rs, "production", "UTC").unwrap_or_default();
        assert_eq!(name, "v2", "live wins over a newer canary");
        assert_eq!(
            start.format("%Y-%m-%d %H:%M").to_string(),
            "2026-01-05 10:00"
        );
        // Nothing live: the newest rolling out.
        let rs = [
            rel("v1", "production", State::RolledBack, "2026-01-01 10:00"),
            rel("v2", "production", State::RollingOut, "2026-01-03 10:00"),
            rel("v3", "production", State::RollingOut, "2026-01-06 10:00"),
        ];
        assert_eq!(
            current_release(&rs, "production", "UTC").map(|x| x.0),
            Some("v3".into())
        );
        assert!(current_release(&rs, "staging", "UTC").is_none());
        // Times written in the project's zone are read back as UTC.
        let rs = [rel("v1", "production", State::Live, "2026-01-05 10:00")];
        let paris = current_release(&rs, "production", "Europe/Paris").unwrap_or_default();
        assert!(
            paris.1
                < current_release(&rs, "production", "UTC")
                    .unwrap_or_default()
                    .1
        );
    }

    #[test]
    fn the_window_holds_the_release_s_first_event_to_the_microsecond() {
        // The release's first event is on a whole second, `now` is not.
        let start = Utc::now() - Duration::hours(70);
        let start = start - Duration::nanoseconds(i64::from(start.timestamp_subsec_nanos()));
        let now = start + Duration::hours(70) + Duration::milliseconds(400);
        let w = window(Some(("v2".into(), start)), now);
        assert!(now - Duration::seconds(w.current.start) <= start);
        assert!(now - Duration::seconds(w.previous.end) <= start);
    }

    #[test]
    fn the_window_runs_from_the_release_and_compares_the_same_length_before() {
        let now = Utc::now();
        let w = window(Some(("v2".into(), now - Duration::hours(80))), now);
        assert_eq!(w.current.end, 0);
        assert_eq!(w.current.start, 80 * 3600);
        assert_eq!(w.previous.end, w.current.start);
        assert_eq!(w.previous.start - w.previous.end, w.current.start);
        assert_eq!(w.window.days, 4, "rounded up");
        assert_eq!(w.window.label, "Since v2 (4 days)");
        assert!(!w.window.capped);
    }

    #[test]
    fn without_a_release_it_is_14_days_against_the_14_before() {
        let now = Utc::now();
        let w = window(None, now);
        assert_eq!(w.current, Span::days(FALLBACK_DAYS));
        assert_eq!(
            w.previous,
            Span {
                start: 2 * FALLBACK_DAYS * DAY,
                end: FALLBACK_DAYS * DAY
            }
        );
        assert!(w.window.release.is_none());
        assert_eq!(w.window.label, "Last 14 days");
    }

    #[test]
    fn an_old_release_is_cut_to_90_days() {
        let now = Utc::now();
        let w = window(Some(("v1".into(), now - Duration::days(200))), now);
        assert_eq!(w.current, Span::days(MAX_DAYS));
        assert_eq!(w.previous.start, 2 * MAX_DAYS * DAY);
        assert!(w.window.capped);
        assert_eq!(w.window.days, MAX_DAYS);
        // A release first seen now still has a span.
        let w = window(Some(("v3".into(), now + Duration::minutes(5))), now);
        assert!(w.current.start > 0);
        assert_eq!(w.window.days, 1);
        assert_eq!(w.window.label, "Since v3 (1 day)");
    }
}

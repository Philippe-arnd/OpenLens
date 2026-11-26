//! An explicit window on the board (#513, D-060): a date range or a past
//! release, picked on the board page or asked of the API, instead of the
//! release window of D-055.
//!
//! - **Range**: from the first day's midnight to the midnight after the
//!   last day (now, for today), in the project's zone (D-017), so a day
//!   with a change of clock lasts 23 or 25 hours. At most
//!   [`MAX_RANGE_DAYS`] days, none in the future, and none before the
//!   400 days the events are kept (D-015).
//! - **Since a release**: from its first event in the environment to the
//!   first event of the next release that took over (any state but
//!   *rolling out*: a canary does not end it), or now.
//!
//! Both are compared with the same length just before, and read through
//! the same spans as the release window, so the per-day choice between
//! the rollups and the events (D-040, D-047) holds.

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};

use super::*;
use crate::board_window::{DAY, MAX_DAYS, Resolved, parse_local};
use crate::releases::{Release, State};

/// The longest date range, in days: the events kept (D-015).
pub const MAX_RANGE_DAYS: i64 = 400;

/// The longest release name the window accepts (it is only compared with
/// the names `releases.list` returns, never put in a query).
const MAX_NAME: usize = 200;

impl Window {
    /// The window asked for by a page's or the API's query: `since` (a
    /// release), or `from` and `to` (`YYYY-MM-DD`, the project's days in
    /// `tz`; `to` missing is today). The dates win when both are given.
    /// None: nothing was asked, and the caller keeps its default. The
    /// range itself is checked when the board resolves it.
    ///
    /// # Errors
    /// A date that is not `YYYY-MM-DD`, a range without its first day, a
    /// release name too long, or an invalid time zone.
    pub fn asked(
        since: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        tz: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Self>> {
        fn given(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|s| !s.is_empty())
        }
        let date = |s: &str| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|_| EngineError::InvalidFilter("a day is written YYYY-MM-DD"))
        };
        match (given(from), given(to)) {
            (Some(from), to) => {
                let to = match to {
                    Some(t) => date(t)?,
                    None => now.with_timezone(&zone(tz)?).date_naive(),
                };
                Ok(Some(Self::Range {
                    from: date(from)?,
                    to,
                }))
            }
            (None, Some(_)) => Err(EngineError::InvalidFilter(
                "the date range needs its first day",
            )),
            (None, None) => match given(since) {
                Some(r) if r.len() > MAX_NAME => Err(EngineError::InvalidFilter("release")),
                Some(r) => Ok(Some(Self::Since {
                    release: r.to_owned(),
                })),
                None => Ok(None),
            },
        }
    }
}

impl Window {
    /// The UTC days, first and last, that a date range and its comparison
    /// read, when the range ends before today in the zone `tz` at `now`:
    /// every day it covers is over, so its period blocks change only when
    /// something is written to one of those days (#516, D-064). None for
    /// any other window, a range that reaches today, or one the board
    /// would refuse to start with.
    #[must_use]
    pub fn closed_utc_days(&self, tz: &str, now: DateTime<Utc>) -> Option<(NaiveDate, NaiveDate)> {
        let Self::Range { from, to } = self else {
            return None;
        };
        let zone = zone(tz).ok()?;
        if from > to || *to >= now.with_timezone(&zone).date_naive() {
            return None;
        }
        let start = start_of(zone, *from);
        let end = start_of(zone, to.succ_opt()?);
        // The comparison is the same length just before (`spans`).
        let len = end - start;
        Some((
            (start - len).date_naive(),
            (end - Duration::microseconds(1)).date_naive(),
        ))
    }
}

fn zone(tz: &str) -> Result<chrono_tz::Tz> {
    tz.parse::<chrono_tz::Tz>()
        .map_err(|_| EngineError::InvalidFilter("timezone"))
}

/// The instant the day `d` starts in `zone`: its midnight, or the first
/// hour that exists when the clock skipped midnight.
fn start_of(zone: chrono_tz::Tz, d: NaiveDate) -> DateTime<Utc> {
    (0..3)
        .filter_map(|h| NaiveTime::from_hms_opt(h, 0, 0))
        .find_map(|t| zone.from_local_datetime(&d.and_time(t)).earliest())
        .map_or_else(
            || d.and_time(NaiveTime::MIN).and_utc(),
            |t| t.with_timezone(&Utc),
        )
}

/// A window and its comparison from the instant `start` to `end` (now at
/// the latest) at `now`.
fn spans(start: DateTime<Utc>, end: DateTime<Utc>, now: DateTime<Utc>) -> (Span, Span) {
    let back = (now - start).num_seconds().max(1);
    let len = (end.min(now) - start).num_seconds().clamp(1, back);
    (
        Span {
            start: back,
            end: back - len,
        },
        Span {
            start: back + len,
            end: back,
        },
    )
}

fn minute(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M").to_string()
}

/// The window from the day `from` to the day `to`, both included, in the
/// zone `tz`, at `now` (D-060).
///
/// # Errors
/// An invalid zone, or a range that ends before it starts, ends after
/// today, is longer than [`MAX_RANGE_DAYS`] or starts before the days kept.
pub(crate) fn range(
    tz: &str,
    from: NaiveDate,
    to: NaiveDate,
    now: DateTime<Utc>,
) -> Result<Resolved> {
    let zone = zone(tz)?;
    let today = now.with_timezone(&zone).date_naive();
    if from > to {
        return Err(EngineError::InvalidFilter(
            "the date range ends before it starts",
        ));
    }
    if to > today {
        return Err(EngineError::InvalidFilter(
            "the date range ends in the future",
        ));
    }
    let days = (to - from).num_days() + 1;
    if days > MAX_RANGE_DAYS {
        return Err(EngineError::InvalidFilter(
            "the date range is longer than 400 days",
        ));
    }
    if (today - from).num_days() >= MAX_RANGE_DAYS {
        return Err(EngineError::InvalidFilter(
            "the date range starts before the 400 days kept",
        ));
    }
    let start = start_of(zone, from);
    let end = to.succ_opt().map_or(now, |d| start_of(zone, d)).min(now);
    let (current, previous) = spans(start, end, now);
    let label = if days == 1 {
        format!("On {from} (1 day)")
    } else {
        format!("From {from} to {to} ({days} days)")
    };
    Ok(Resolved {
        window: BoardWindow {
            since: minute(start),
            until: (end < now).then(|| minute(end)),
            days,
            label,
            ..BoardWindow::default()
        },
        current,
        previous,
    })
}

/// The window while `name` was the newest release of `environment` in
/// `releases` (`releases.list`, times in `tz`), at `now` (D-060).
///
/// # Errors
/// The environment shows no release of that name.
pub(crate) fn since(
    releases: &[Release],
    environment: &str,
    tz: &str,
    name: &str,
    now: DateTime<Utc>,
) -> Result<Resolved> {
    let ours = releases
        .iter()
        .filter(|r| r.environment == environment)
        .collect::<Vec<_>>();
    let start = ours
        .iter()
        .find(|r| r.release == name)
        .and_then(|r| parse_local(&r.first_seen, tz))
        .ok_or(EngineError::InvalidFilter(
            "no such release in this environment in the last 90 days",
        ))?;
    let next = ours
        .iter()
        .filter(|r| r.release != name && r.state != State::RollingOut)
        .filter_map(|r| parse_local(&r.first_seen, tz))
        .filter(|t| *t > start)
        .min()
        .filter(|t| *t < now);
    let capped = now - start >= Duration::days(MAX_DAYS);
    let start = start.max(now - Duration::days(MAX_DAYS));
    let end = next.unwrap_or(now);
    let (current, previous) = spans(start, end, now);
    let len = current.start - current.end;
    let days = (len / DAY + i64::from(len % DAY != 0)).max(1);
    let plural = if days == 1 { "day" } else { "days" };
    let label = match next {
        Some(_) => format!("During {name} ({days} {plural})"),
        None if capped => format!("Since {name} (the last {days} days)"),
        None => format!("Since {name} ({days} {plural})"),
    };
    Ok(Resolved {
        window: BoardWindow {
            release: Some(name.to_owned()),
            since: minute(start),
            until: next.map(minute),
            days,
            capped,
            label,
            ..BoardWindow::default()
        },
        current,
        previous,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board_window::tests::rel;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_default()
    }

    #[test]
    fn a_range_is_refused_backwards_in_the_future_or_too_long() {
        let now = at("2026-11-26T12:00:00Z");
        let err = |from: &str, to: &str| {
            range("UTC", day(from), day(to), now)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        assert!(err("2026-11-10", "2026-11-01").contains("before it starts"));
        assert!(err("2026-11-20", "2026-11-27").contains("future"));
        assert!(err("2025-01-01", "2026-11-01").contains("longer than 400"));
        assert!(err("2025-09-01", "2025-09-30").contains("400 days kept"));
        // Today is allowed, and ends now.
        let r = range("UTC", day("2026-11-20"), day("2026-11-26"), now);
        assert!(r.as_ref().is_ok_and(|r| r.current.end == 0));
        // Today in Tokyo is already tomorrow in UTC terms.
        assert!(
            range(
                "Asia/Tokyo",
                day("2026-11-27"),
                day("2026-11-27"),
                at("2026-11-26T20:00:00Z")
            )
            .is_ok()
        );
        assert!(
            range(
                "UTC",
                day("2026-11-27"),
                day("2026-11-27"),
                at("2026-11-26T20:00:00Z")
            )
            .is_err()
        );
    }

    #[test]
    fn a_range_over_a_change_of_clock_follows_the_zone_days() {
        // Paris leaves summer time on 2026-10-25: that day lasts 25 hours.
        let now = at("2026-11-26T12:00:00Z");
        let r = range("Europe/Paris", day("2026-10-24"), day("2026-10-25"), now);
        let Ok(r) = r else {
            panic!("a valid range");
        };
        assert_eq!(r.current.start - r.current.end, 49 * 3600);
        assert_eq!(r.previous.end, r.current.start);
        assert_eq!(r.previous.start - r.previous.end, 49 * 3600);
        assert_eq!(r.window.days, 2);
        // Midnight in Paris on the 24th is 22:00 UTC on the 23rd.
        assert_eq!(r.window.since, "2026-10-23 22:00");
        assert_eq!(r.window.until.as_deref(), Some("2026-10-25 23:00"));
        let f = Filters {
            tz: "Europe/Paris".into(),
            ..Filters::default()
        };
        let w = f.clause_span(r.current, now).unwrap_or_default();
        assert!(w.contains("date >= '2026-10-23'"), "{w}");
        assert!(w.contains("date <= '2026-10-25'"), "{w}");
        assert!(w.contains("timestamp < now() - INTERVAL"), "{w}");
        // The same two days in UTC are 48 hours.
        let u = range("UTC", day("2026-10-24"), day("2026-10-25"), now);
        assert!(u.is_ok_and(|u| u.current.start - u.current.end == 48 * 3600));
    }

    #[test]
    fn only_a_range_before_today_has_closed_days() {
        let now = at("2026-11-26T12:00:00Z");
        let range = |tz: &str, from: &str, to: &str| {
            Window::Range {
                from: day(from),
                to: day(to),
            }
            .closed_utc_days(tz, now)
        };
        // Two UTC days, compared with the two before.
        assert_eq!(
            range("UTC", "2026-11-20", "2026-11-21"),
            Some((day("2026-11-18"), day("2026-11-21")))
        );
        // Yesterday is closed; today is not, nor anything but a range.
        assert!(range("UTC", "2026-11-25", "2026-11-25").is_some());
        assert!(range("UTC", "2026-11-20", "2026-11-26").is_none());
        assert!(Window::Release.closed_utc_days("UTC", now).is_none());
        // It is already the 27th in Kiritimati (UTC+14): its 26th is over,
        // at 10:00 UTC today, so it reaches today's UTC day.
        assert!(
            range("Pacific/Kiritimati", "2026-11-26", "2026-11-26")
                .is_some_and(|(_, last)| last == day("2026-11-26"))
        );
    }

    #[test]
    fn a_past_release_runs_until_the_next_one_took_over() {
        let now = at("2026-01-20T12:00:00Z");
        let rs = [
            rel("v1", "production", State::Superseded, "2026-01-01 10:00"),
            rel("v2", "production", State::Live, "2026-01-05 10:00"),
            rel("v3", "production", State::RollingOut, "2026-01-18 10:00"),
        ];
        let w = since(&rs, "production", "UTC", "v1", now);
        assert!(
            w.as_ref()
                .is_ok_and(|w| w.window.label == "During v1 (4 days)")
        );
        // A canary does not end the live release's window.
        let w = since(&rs, "production", "UTC", "v2", now);
        assert!(
            w.as_ref()
                .is_ok_and(|w| w.current.end == 0 && w.window.until.is_none())
        );
        assert!(since(&rs, "staging", "UTC", "v1", now).is_err());
        assert!(since(&rs, "production", "UTC", "v9", now).is_err());
    }

    #[test]
    fn the_query_picks_the_dates_over_the_release() {
        let now = at("2026-11-26T12:00:00Z");
        let w = Window::asked(Some("v1"), Some("2026-11-01"), None, "UTC", now);
        assert!(matches!(
            w,
            Ok(Some(Window::Range { to, .. })) if to == day("2026-11-26")
        ));
        let w = Window::asked(Some(" v1 "), Some(""), Some(""), "UTC", now);
        assert!(matches!(w, Ok(Some(Window::Since { release })) if release == "v1"));
        assert!(matches!(
            Window::asked(None, None, None, "UTC", now),
            Ok(None)
        ));
        assert!(Window::asked(None, Some("01/11/2026"), None, "UTC", now).is_err());
        assert!(Window::asked(None, None, Some("2026-11-01"), "UTC", now).is_err());
    }
}

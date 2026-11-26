//! What a board window costs before it runs (#514, D-061): how many days
//! it reads, how many of them come from the daily rollups and how many
//! from the events, the bytes and rows of those events, and a rough time.
//!
//! - **Days**: the UTC days the window and its comparison touch. A day
//!   comes from the rollups when the board would read it there (D-040,
//!   D-047): closed, rolled up in `person_day` and `session_day` with the
//!   map in force, and strictly inside the span (its first and last days
//!   are cut by time, so they are read from the events). In a zone off UTC
//!   the daily series and the features block read the window's events
//!   (D-040), so every day of the window counts as read from the events.
//! - **Bytes and rows**: those of the stored segments of the days read
//!   from the events, from the file sizes and the Parquet footers
//!   (`QueryEngine::day_sizes`); a day with no events costs nothing.
//! - **Time**: bytes / [`READ_BYTES_PER_SEC`], a rough figure.
//!
//! A date range or a past release asks for confirmation above
//! [`CONFIRM_DAYS`] days read from the events; the current release's
//! window never asks, and the fixed windows have no estimate.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};
use openlens_query::DaySize;

use super::*;
use crate::board_window::Resolved;

/// Above this many days read from the events, a date range or a past
/// release runs only once confirmed (#325, option A).
pub const CONFIRM_DAYS: usize = 30;

/// The read speed the time is estimated with, in bytes per second: the
/// cold board at 63 M events read its 2,955 MB of Parquet in 22.0 s, about
/// 134 MB/s, before the rollups (docs/benchmarks.md), rounded down.
pub const READ_BYTES_PER_SEC: u64 = 130_000_000;

/// How [`Estimate::secs`] is computed, for the reader.
pub const HEURISTIC: &str = "rough: the bytes of the events to read at 130 MB/s, the cold board's speed in docs/benchmarks.md";

/// The cost of a board window (D-061).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Estimate {
    /// The window, as the board names it ("From 2026-01-01 to …").
    pub label: String,
    /// Days in the window, as the board counts them.
    pub days: i64,
    /// Days with something stored that the window and its comparison
    /// read: [`Estimate::rollup_days`] + [`Estimate::raw_days`].
    pub read_days: usize,
    /// Days read from the daily rollups.
    pub rollup_days: usize,
    /// Days read from the events.
    pub raw_days: usize,
    /// Bytes of the events to read (their segments on disk).
    pub bytes: u64,
    /// Rows of the events to read.
    pub rows: u64,
    /// Estimated seconds: rough, see [`Estimate::heuristic`].
    pub secs: f64,
    /// How `secs` is estimated.
    pub heuristic: String,
    /// The window runs only once confirmed: a date range or a past release
    /// reading more than [`CONFIRM_DAYS`] days from the events.
    pub confirm: bool,
}

/// The estimate of the board window `f` asks for in `project`, with the
/// feature map version `version` in force; none for the fixed windows
/// (and a board filtered on one release, which keeps them).
///
/// # Errors
/// A failed query, or a window that is not allowed (as the board would
/// refuse it).
pub async fn estimate(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    version: Option<i64>,
) -> Result<Option<Estimate>> {
    let q = &q.with_params(&f.params()?);
    let now = Utc::now();
    let Some(win) = crate::board_window::resolve(q, project, f, now).await? else {
        return Ok(None);
    };
    let back = (win.previous.start / 86_400 + 2).max(90);
    let (people, sessions) = futures::try_join!(
        crate::board_people::rolled_days_back(q, project, "person_day", version, now, back),
        crate::board_people::rolled_days_back(q, project, "session_day", version, now, back),
    )?;
    let sessions: BTreeSet<NaiveDate> = sessions.into_iter().collect();
    let rolled: BTreeSet<NaiveDate> = people
        .into_iter()
        .filter(|d| sessions.contains(d))
        .collect();
    let first = (now - Duration::seconds(win.previous.start)).date_naive();
    let sizes = q.day_sizes(project, first, now.date_naive()).await?;
    Ok(Some(cost(&win, f, &rolled, &sizes, now)))
}

/// The UTC days the span `s` touches at `now`, and whether each is read
/// from the rollups (`rolled`, strictly inside the span, as
/// `board_people::split_span` reads them).
fn days_of(s: Span, rolled: &BTreeSet<NaiveDate>, now: DateTime<Utc>) -> Vec<(NaiveDate, bool)> {
    let cutoff = (now - Duration::seconds(s.start)).date_naive();
    let until = (now - Duration::seconds(s.end)).date_naive();
    // The span ends before `until`'s first instant when it ends on a UTC
    // midnight: that day is not read.
    let last = if s.end > 0 {
        (now - Duration::seconds(s.end) - Duration::microseconds(1)).date_naive()
    } else {
        until
    };
    cutoff
        .iter_days()
        .take_while(|d| *d <= last)
        .map(|d| (d, rolled.contains(&d) && d > cutoff && d < until))
        .collect()
}

/// The estimate of the resolved window `win` with the filters `f`, the
/// days `rolled` up with the map in force and the stored `sizes`, at `now`.
pub(crate) fn cost(
    win: &Resolved,
    f: &Filters,
    rolled: &BTreeSet<NaiveDate>,
    sizes: &[DaySize],
    now: DateTime<Utc>,
) -> Estimate {
    // Off UTC, the daily series and the features block read the window's
    // events (D-040).
    let zone_days = crate::board_people::utc_days(
        &f.tz,
        (now - Duration::seconds(win.current.start)).date_naive(),
        now.date_naive(),
    );
    let mut raw = BTreeSet::new();
    let mut from_rollups = BTreeSet::new();
    for (span, whole) in [(win.current, zone_days), (win.previous, true)] {
        for (d, rollup) in days_of(span, rolled, now) {
            if rollup && whole {
                from_rollups.insert(d);
            } else {
                raw.insert(d);
            }
        }
    }
    let stored: std::collections::BTreeMap<NaiveDate, &DaySize> =
        sizes.iter().map(|s| (s.day, s)).collect();
    let size = |d: &NaiveDate| stored.get(d).copied();
    // A day read both ways (raw for one block) is read from the events.
    let rollup_days = from_rollups
        .iter()
        .filter(|d| !raw.contains(*d) && size(d).is_some())
        .count();
    let raw: Vec<&DaySize> = raw.iter().filter_map(size).collect();
    let bytes = raw.iter().map(|s| s.bytes).sum::<u64>();
    let asks = matches!(f.window, Window::Range { .. } | Window::Since { .. });
    Estimate {
        label: win.window.label.clone(),
        days: win.window.days,
        read_days: rollup_days + raw.len(),
        rollup_days,
        raw_days: raw.len(),
        bytes,
        rows: raw.iter().map(|s| s.rows).sum(),
        secs: bytes as f64 / READ_BYTES_PER_SEC as f64,
        heuristic: HEURISTIC.to_owned(),
        confirm: asks && raw.len() > CONFIRM_DAYS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_default()
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    /// One stored day of 1 MB and 1,000 rows each, from `from` to `to`.
    fn stored(from: &str, to: &str) -> Vec<DaySize> {
        day(from)
            .iter_days()
            .take_while(|d| *d <= day(to))
            .map(|d| DaySize {
                day: d,
                files: 1,
                bytes: 1_000_000,
                rows: 1_000,
            })
            .collect()
    }

    fn range(from: &str, to: &str, now: DateTime<Utc>) -> (Resolved, Filters) {
        let f = Filters {
            window: Window::Range {
                from: day(from),
                to: day(to),
            },
            ..Filters::default()
        };
        let r = crate::board_range::range("UTC", day(from), day(to), now);
        (
            r.unwrap_or_else(|_| crate::board_window::window(None, now)),
            f,
        )
    }

    #[test]
    fn rolled_days_inside_the_spans_are_not_read_from_the_events() {
        let now = at("2026-11-26T12:00:00Z");
        // Ten days, compared with the ten before: 20 days read.
        let (win, f) = range("2026-11-01", "2026-11-10", now);
        let sizes = stored("2026-10-01", "2026-11-26");
        let none = cost(&win, &f, &BTreeSet::new(), &sizes, now);
        assert_eq!(none.raw_days, 20, "nothing rolled: every day is raw");
        assert_eq!(none.rollup_days, 0);
        assert_eq!(none.bytes, 20 * 1_000_000);
        assert_eq!(none.rows, 20 * 1_000);
        assert!(none.secs > 0.0);
        // Every day rolled up: only each span's first day, cut by time, is
        // read from the events.
        let all: BTreeSet<NaiveDate> = sizes.iter().map(|s| s.day).collect();
        let rolled = cost(&win, &f, &all, &sizes, now);
        assert_eq!(rolled.raw_days, 2, "{rolled:?}");
        assert_eq!(rolled.rollup_days, 18);
        assert_eq!(rolled.read_days, none.read_days);
        assert!(rolled.bytes < none.bytes);
        // Some rolled: the others are read from the events.
        let some: BTreeSet<NaiveDate> = all
            .iter()
            .copied()
            .filter(|d| *d >= day("2026-11-05"))
            .collect();
        let part = cost(&win, &f, &some, &sizes, now);
        assert!(part.raw_days > rolled.raw_days && part.raw_days < none.raw_days);
        assert_eq!(part.raw_days + part.rollup_days, 20);
        // A day with no events costs nothing.
        let empty = cost(
            &win,
            &f,
            &BTreeSet::new(),
            &stored("2026-11-05", "2026-11-26"),
            now,
        );
        assert_eq!(empty.raw_days, 6, "{empty:?}");
    }

    #[test]
    fn a_range_asks_above_30_days_from_the_events_and_the_release_window_never() {
        let now = at("2026-11-26T12:00:00Z");
        let sizes = stored("2026-01-01", "2026-11-26");
        // 15 days against 15: 30 raw days, at the threshold.
        let (win, f) = range("2026-11-01", "2026-11-15", now);
        let at_30 = cost(&win, &f, &BTreeSet::new(), &sizes, now);
        assert_eq!(at_30.raw_days, CONFIRM_DAYS);
        assert!(!at_30.confirm, "at most 30 days runs at once");
        // 16 against 16: 32.
        let (win, f) = range("2026-11-01", "2026-11-16", now);
        let over = cost(&win, &f, &BTreeSet::new(), &sizes, now);
        assert!(over.raw_days > CONFIRM_DAYS && over.confirm);
        // The same range answered by the rollups runs at once.
        let all: BTreeSet<NaiveDate> = sizes.iter().map(|s| s.day).collect();
        assert!(!cost(&win, &f, &all, &sizes, now).confirm);
        // The current release's window never asks, however much it reads.
        let release = Filters {
            window: Window::Release,
            ..Filters::default()
        };
        let win = crate::board_window::window(Some(("v1".into(), now - Duration::days(200))), now);
        let e = cost(&win, &release, &BTreeSet::new(), &sizes, now);
        assert!(e.raw_days > CONFIRM_DAYS && !e.confirm, "{e:?}");
    }

    #[test]
    fn off_utc_the_window_is_read_from_the_events() {
        let now = at("2026-11-26T12:00:00Z");
        let sizes = stored("2026-10-01", "2026-11-26");
        let all: BTreeSet<NaiveDate> = sizes.iter().map(|s| s.day).collect();
        let (win, mut f) = range("2026-11-01", "2026-11-10", now);
        let utc = cost(&win, &f, &all, &sizes, now);
        f.tz = "Europe/Paris".into();
        let paris = cost(&win, &f, &all, &sizes, now);
        assert!(paris.raw_days > utc.raw_days, "{paris:?}");
        assert!(
            paris.rollup_days > 0,
            "the comparison still reads the rollups"
        );
    }
}

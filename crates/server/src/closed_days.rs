//! The period blocks of boards whose window is entirely before today,
//! kept until something is written to one of the days they read (#516,
//! D-064).
//!
//! A board's cache ([`crate::boards`]) drops every result of a project when
//! new data lands in it, which is right for the blocks that end now (DAU,
//! Web Vitals, the insight cards…) but makes a long date range in the past
//! read all its days again because of an event captured today. Here the
//! period blocks of such a range (`openlens_engine::PeriodBlocks`) are kept
//! per (project, filters, map version), with the UTC days the range and
//! its comparison read, and handed back to the engine, which reads only
//! the other blocks.
//!
//! - **Invalidation:** every writer of a project's days marks them (a
//!   flush or a late event, compaction, a relabel, a rollup written), or
//!   marks the whole project (an erasure, retention, the click trim, the
//!   cold tier). A kept result is served only while no day it read, and
//!   not its project, was marked after it started computing.
//! - **Bounds:** [`ENTRIES`] results and [`BYTES`] of them at most, oldest
//!   out first; [`MAX_AGE`] at most, which bounds what identity changes
//!   (an identify, an alias, an internal user) can leave out of a kept
//!   result; [`MARKED_DAYS`] marked days per project, beyond which the
//!   whole project is marked instead.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use openlens_engine::PeriodBlocks;

/// Results kept, all projects together.
const ENTRIES: usize = 32;
/// Bytes of results kept (as JSON), all projects together.
const BYTES: usize = 4 * 1024 * 1024;
/// Longest a result is kept, even with no day marked.
const MAX_AGE: Duration = Duration::from_secs(3600);
/// Days marked per project before the whole project is marked instead.
const MARKED_DAYS: usize = 1024;

/// (project, filters, map version), as the board cache's key.
pub(crate) type Key = (String, String, i64);

/// The first and last UTC days a result read.
pub(crate) type Days = (NaiveDate, NaiveDate);

struct Kept {
    days: Days,
    /// The mark when its computation started.
    since: u64,
    at: Instant,
    bytes: usize,
    blocks: Arc<PeriodBlocks>,
}

#[derive(Default)]
struct Marks {
    /// The last mark of the whole project.
    all: u64,
    /// The last mark of each day.
    days: BTreeMap<NaiveDate, u64>,
}

#[derive(Default)]
struct State {
    /// Bumped on every mark.
    seq: u64,
    /// The last mark of every project at once.
    everywhere: u64,
    marks: HashMap<String, Marks>,
    kept: HashMap<Key, Kept>,
    bytes: usize,
}

impl State {
    fn unchanged(&self, project: &str, (first, last): Days, since: u64) -> bool {
        self.everywhere <= since
            && self.marks.get(project).is_none_or(|m| {
                m.all <= since && m.days.range(first..=last).all(|(_, &v)| v <= since)
            })
    }

    fn remove(&mut self, key: &Key) {
        if let Some(k) = self.kept.remove(key) {
            self.bytes = self.bytes.saturating_sub(k.bytes);
        }
    }
}

/// Kept period blocks and the marks they are checked against; part of the
/// board cache, so of `AppState` (no static).
#[derive(Default)]
pub struct ClosedDays {
    state: Mutex<State>,
}

impl ClosedDays {
    /// The mark to give [`ClosedDays::put`]: taken before computing, so a
    /// write during the computation is seen as after it.
    pub(crate) fn mark(&self) -> u64 {
        self.state.lock().map_or(u64::MAX, |s| s.seq)
    }

    /// The blocks kept for `key` over `days`, if nothing was written to
    /// them since.
    pub(crate) fn get(&self, key: &Key, days: Days) -> Option<Arc<PeriodBlocks>> {
        let mut s = self.state.lock().ok()?;
        let k = s.kept.get(key)?;
        let (since, same, young) = (k.since, k.days == days, k.at.elapsed() < MAX_AGE);
        if same && young && s.unchanged(&key.0, days, since) {
            return s.kept.get(key).map(|k| k.blocks.clone());
        }
        s.remove(key);
        None
    }

    /// Keep `blocks` for `key` over `days`, computed from the mark `since`.
    /// Not kept when a day was written meanwhile, or when it alone would
    /// take a quarter of [`BYTES`].
    pub(crate) fn put(&self, key: Key, days: Days, since: u64, blocks: PeriodBlocks) {
        let bytes = serde_json::to_vec(&blocks).map_or(usize::MAX, |v| v.len());
        if bytes > BYTES / 4 {
            return;
        }
        let Ok(mut s) = self.state.lock() else {
            return;
        };
        if !s.unchanged(&key.0, days, since) {
            return;
        }
        s.remove(&key);
        while !s.kept.is_empty() && (s.kept.len() >= ENTRIES || s.bytes + bytes > BYTES) {
            let Some(oldest) = s
                .kept
                .iter()
                .min_by_key(|(_, k)| k.at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            s.remove(&oldest);
        }
        s.bytes += bytes;
        s.kept.insert(
            key,
            Kept {
                days,
                since,
                at: Instant::now(),
                bytes,
                blocks: Arc::new(blocks),
            },
        );
    }

    /// Something was written to these UTC days of `project`: a flush, a
    /// compaction, a relabel, a rollup.
    pub fn days_changed(&self, project: &str, days: impl IntoIterator<Item = NaiveDate>) {
        let Ok(mut s) = self.state.lock() else {
            return;
        };
        s.seq += 1;
        let seq = s.seq;
        let m = s.marks.entry(project.to_owned()).or_default();
        for d in days {
            m.days.insert(d, seq);
        }
        if m.days.len() > MARKED_DAYS {
            m.days.clear();
            m.all = seq;
        }
    }

    /// Any day of `project` may have changed: an erasure, retention, a
    /// rewrite of its files.
    pub fn project_changed(&self, project: &str) {
        let Ok(mut s) = self.state.lock() else {
            return;
        };
        s.seq += 1;
        let seq = s.seq;
        let m = s.marks.entry(project.to_owned()).or_default();
        m.days.clear();
        m.all = seq;
    }

    /// Any day of any project may have changed (the cold tier moved or
    /// erased days).
    pub fn all_changed(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.seq += 1;
            s.everywhere = s.seq;
        }
    }

    /// Results kept now.
    pub fn len(&self) -> usize {
        self.state.lock().map_or(0, |s| s.kept.len())
    }

    /// Whether no result is kept.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Every day from `first` to `last`, both included.
pub(crate) fn span(first: NaiveDate, last: NaiveDate) -> BTreeSet<NaiveDate> {
    first.iter_days().take_while(|d| *d <= last).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    fn key(map: i64) -> Key {
        ("p".into(), "range 2026-11-01..2026-11-20".into(), map)
    }

    /// A window of 2026-11-01 to 11-20 compared with the 20 days before.
    fn days() -> Days {
        (day("2026-10-12"), day("2026-11-20"))
    }

    fn kept() -> ClosedDays {
        let c = ClosedDays::default();
        let since = c.mark();
        c.put(key(1), days(), since, PeriodBlocks::default());
        c
    }

    #[test]
    fn a_past_window_is_served_after_data_for_today() {
        let c = kept();
        c.days_changed("p", [day("2026-11-26")]);
        // Another project's late day does not touch it either.
        c.days_changed("q", [day("2026-11-10")]);
        assert!(c.get(&key(1), days()).is_some());
    }

    #[test]
    fn a_late_event_on_a_covered_day_drops_it() {
        let c = kept();
        c.days_changed("p", [day("2026-10-20")]);
        assert!(c.get(&key(1), days()).is_none());
        assert!(c.is_empty(), "the stale result is gone");
    }

    #[test]
    fn an_erasure_drops_it() {
        let c = kept();
        c.project_changed("p");
        assert!(c.get(&key(1), days()).is_none());
        let c = kept();
        c.all_changed();
        assert!(c.get(&key(1), days()).is_none());
    }

    #[test]
    fn another_map_version_misses() {
        let c = kept();
        assert!(c.get(&key(2), days()).is_none());
        assert!(c.get(&key(1), days()).is_some());
    }

    #[test]
    fn a_write_during_the_computation_is_not_kept() {
        let c = ClosedDays::default();
        let since = c.mark();
        c.days_changed("p", [day("2026-11-02")]);
        c.put(key(1), days(), since, PeriodBlocks::default());
        assert!(c.get(&key(1), days()).is_none());
    }

    #[test]
    fn results_are_bounded() {
        let c = ClosedDays::default();
        for i in 0..ENTRIES as i64 * 2 {
            let since = c.mark();
            c.put(key(i), days(), since, PeriodBlocks::default());
        }
        assert!(c.len() <= ENTRIES);
        let c = ClosedDays::default();
        c.days_changed("p", span(day("2020-01-01"), day("2026-01-01")));
        let s = c
            .state
            .lock()
            .map(|s| s.marks.get("p").map(|m| m.days.len()));
        assert!(matches!(s, Ok(Some(n)) if n <= MARKED_DAYS));
    }
}

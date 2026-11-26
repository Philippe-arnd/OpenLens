//! Days a window cannot count (#489, D-070): a closed day whose events are
//! gone from the disk (moved to the cold tier, D-031, or past the
//! retention, D-015) and whose rollups were not made with the feature map
//! in force, or lack a table the numbers read. Such a day is read from its
//! events by the per-day rule (D-040), finds none, and counts nobody; the
//! board and the map say which days those are instead of hiding them.
//!
//! A day with neither events nor rollups is not listed: rollups are only
//! written from events, so nothing was ever stored for it.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};
use openlens_query::QueryEngine;

use crate::Result;

/// The days of `gone` (days with a rollup but no events) that are not in
/// every one of `rolled` (the days each table read gives with the map in
/// force), as `YYYY-MM-DD`, oldest first.
#[must_use]
pub(crate) fn pick(gone: &[NaiveDate], rolled: &[&[NaiveDate]]) -> Vec<String> {
    let sets: Vec<BTreeSet<NaiveDate>> =
        rolled.iter().map(|r| r.iter().copied().collect()).collect();
    let mut out: Vec<NaiveDate> = gone
        .iter()
        .copied()
        .filter(|d| !sets.iter().all(|s| s.contains(d)))
        .collect();
    out.sort_unstable();
    out.dedup();
    out.into_iter()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .collect()
}

/// The days from `from` to the last closed day at `now` that `project`
/// cannot count: no events on disk, and not in every one of `rolled`.
///
/// # Errors
/// The rollup or event directories could not be listed.
pub(crate) async fn days(
    q: &QueryEngine,
    project: &str,
    from: NaiveDate,
    rolled: &[&[NaiveDate]],
    now: DateTime<Utc>,
) -> Result<Vec<String>> {
    let to = openlens_query::last_closed(now);
    if from > to {
        return Ok(Vec::new());
    }
    let gone = q.days_without_events(project, from, to).await?;
    Ok(pick(&gone, rolled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_without_events_counts_only_when_every_table_has_it() {
        let d = |n: u32| NaiveDate::from_ymd_opt(2026, 1, n).unwrap_or_default();
        let gone = [d(3), d(1), d(2)];
        let people = [d(1), d(2)];
        let sessions = [d(1)];
        // Day 1 is rolled up in both tables; day 2 lacks its sessions, day 3
        // everything.
        assert_eq!(
            pick(&gone, &[&people, &sessions]),
            vec!["2026-01-02".to_owned(), "2026-01-03".to_owned()]
        );
        // Without a map version nothing is read from the rollups.
        assert_eq!(pick(&gone, &[&[]]).len(), 3);
        assert!(pick(&[], &[&people]).is_empty());
    }
}

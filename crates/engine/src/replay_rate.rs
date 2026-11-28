//! The share of sessions the SDK records for replay (#327, D-035).
//!
//! No setting: about a budget of recordings a day, so the rate is
//! `min(1, budget / yesterday's active users)`. The budget is
//! [`DAILY_BUDGET`] on a small disk and grows with a larger one
//! ([`crate::capacity`], #606, D-088). Active users are the distinct
//! people with events yesterday, in the project's time zone (D-017), in
//! production. With no data yet every session is recorded. Sessions left
//! out still keep the last minute or two in the browser's memory and send
//! it when an error or a rage click happens (see `sdk/js/src/replay.ts`).
//!
//! The same count gives the share of sessions that send the events which
//! only feed distributions (`$vital`, `$scroll`, `$field`; #702, D-097):
//! `min(1, EVENT_BUDGET / yesterday's active users)`, handed out with the
//! flags as `sample_rate`.

use chrono::{Duration, NaiveDate, Utc};
use serde::Serialize;

use crate::{Filters, Result, int};
use openlens_query::QueryEngine;

/// Recordings a day the rate aims for, at least: the floor of the budget
/// the disk gives ([`crate::capacity`], D-088).
pub const DAILY_BUDGET: i64 = 300;

/// Sessions a day, about, that send the distribution-only events
/// (`$vital`, `$scroll`, `$field`; #702, D-097). Fixed: the distributions
/// they feed (p75, scroll depth, form drop-off) are stable long before.
pub const EVENT_BUDGET: i64 = 2000;

/// The replay rate of a project and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplayRate {
    /// Share of sessions recorded from their start, 0 to 1.
    pub rate: f64,
    /// Distinct people with events on `day`.
    pub active_users: i64,
    /// The day counted (`YYYY-MM-DD`, the project's yesterday).
    pub day: String,
    /// Recordings a day the rate aims for (D-088).
    pub budget: i64,
    /// Share of sessions that send the distribution-only events, 0 to 1
    /// ([`EVENT_BUDGET`], #702, D-097). Every one of their events carries
    /// it as `$sample`.
    pub sample_rate: f64,
}

impl ReplayRate {
    /// Before anything is counted: every session.
    #[must_use]
    pub fn everything() -> Self {
        Self {
            rate: 1.0,
            active_users: 0,
            day: String::new(),
            budget: DAILY_BUDGET,
            sample_rate: 1.0,
        }
    }
}

/// `min(1, budget / active)`; 1 with nobody active. The budget is never
/// below [`DAILY_BUDGET`].
#[must_use]
pub fn rate_for(active: i64, budget: i64) -> f64 {
    let budget = budget.max(DAILY_BUDGET);
    if active <= budget {
        1.0
    } else {
        budget as f64 / active as f64
    }
}

/// The share of sessions that send the distribution-only events with
/// `active` users yesterday: `min(1, EVENT_BUDGET / active)`.
#[must_use]
pub fn sample_rate_for(active: i64) -> f64 {
    if active <= EVENT_BUDGET {
        1.0
    } else {
        EVENT_BUDGET as f64 / active as f64
    }
}

/// Today in the time zone `tz` (UTC when it is not a known zone).
#[must_use]
pub fn local_today(tz: &str) -> NaiveDate {
    tz.parse::<chrono_tz::Tz>().map_or_else(
        |_| Utc::now().date_naive(),
        |z| Utc::now().with_timezone(&z).date_naive(),
    )
}

/// Count yesterday's active users of `project` (time zone `tz`) and the
/// rate that follows for `budget` recordings a day. One query over the
/// last three days of events.
pub async fn replay_rate(
    q: &QueryEngine,
    project: &str,
    tz: &str,
    budget: i64,
) -> Result<ReplayRate> {
    let f = Filters {
        tz: tz.to_owned(),
        // The team's own sessions are recorded too: they count.
        internal: true,
        ..Filters::default()
    };
    let day = (local_today(tz) - Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let q = q.with_params(&f.params()?);
    // Yesterday started less than 49 h ago (a 25-hour day included).
    let sql = format!(
        "SELECT count(DISTINCT distinct_id) AS n FROM events WHERE {} AND {} = {}",
        f.clause(3)?,
        f.day()?,
        crate::lit(&day)?
    );
    let r = q.sql(project, &sql).await?;
    let active = r.rows.first().map_or(0, |row| int(row, "n"));
    let budget = budget.max(DAILY_BUDGET);
    Ok(ReplayRate {
        rate: rate_for(active, budget),
        active_users: active,
        day,
        budget,
        sample_rate: sample_rate_for(active),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_events_keep_a_larger_share_than_recordings() {
        for active in [0, 299, 300, 1_000, 2_000, 2_001, 50_000, 10_000_000] {
            let (s, r) = (sample_rate_for(active), rate_for(active, DAILY_BUDGET));
            assert!(s >= r && s <= 1.0 && s > 0.0, "{active}: {s} vs {r}");
            if active > EVENT_BUDGET {
                // About the budget of sessions a day, never more.
                assert!((s * active as f64 - EVENT_BUDGET as f64).abs() < 1.0);
            }
        }
    }
}

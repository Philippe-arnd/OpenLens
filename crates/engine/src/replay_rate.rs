//! The share of sessions the SDK records for replay (#327, D-035).
//!
//! No setting: about [`DAILY_BUDGET`] recordings a day, so the rate is
//! `min(1, 300 / yesterday's active users)`. Active users are the distinct
//! people with events yesterday, in the project's time zone (D-017), in
//! production. With no data yet every session is recorded. Sessions left
//! out still keep the last minute or two in the browser's memory and send
//! it when an error or a rage click happens (see `sdk/js/src/replay.ts`).

use chrono::{Duration, NaiveDate, Utc};
use serde::Serialize;

use crate::{Filters, Result, int};
use openlens_query::QueryEngine;

/// Recordings a day the rate aims for.
pub const DAILY_BUDGET: i64 = 300;

/// The replay rate of a project and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplayRate {
    /// Share of sessions recorded from their start, 0 to 1.
    pub rate: f64,
    /// Distinct people with events on `day`.
    pub active_users: i64,
    /// The day counted (`YYYY-MM-DD`, the project's yesterday).
    pub day: String,
}

impl ReplayRate {
    /// Before anything is counted: every session.
    #[must_use]
    pub fn everything() -> Self {
        Self {
            rate: 1.0,
            active_users: 0,
            day: String::new(),
        }
    }
}

/// `min(1, DAILY_BUDGET / active)`; 1 with nobody active.
#[must_use]
pub fn rate_for(active: i64) -> f64 {
    if active <= DAILY_BUDGET {
        1.0
    } else {
        DAILY_BUDGET as f64 / active as f64
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
/// rate that follows. One query over the last three days of events.
pub async fn replay_rate(q: &QueryEngine, project: &str, tz: &str) -> Result<ReplayRate> {
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
    Ok(ReplayRate {
        rate: rate_for(active),
        active_users: active,
        day,
    })
}

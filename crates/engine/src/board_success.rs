//! Where the board reads its success events from (#452, D-062): the
//! success events block (uses and users per success event) and the first
//! success of each journey count success events per person, so a closed
//! day can come from its `success_day` rows instead of its events.
//!
//! The rule is the people blocks' (D-040, [`board_people::split_span`]):
//! `success_day` for a closed day entirely inside the window, rolled up
//! with the feature map in force; the events, grouped the same way, for
//! every other day. Days never overlap, so an event is counted once.

use chrono::{DateTime, NaiveDate, Utc};

use super::*;

/// The board's success rows over a window: which days come from the
/// rollup, and how to read the rest from the events.
pub(crate) struct SuccessRows<'a> {
    /// The board's event source: the events, with the map's roles and features.
    pub src: &'a str,
    /// The canonical role of a raw `role`: the map's lookup, or `role`.
    pub role: String,
    /// Closed days whose `success_day` rows were rolled with the map in
    /// force, oldest first.
    pub rolled: Vec<NaiveDate>,
    /// The board's clock: windows and days are cut at this instant.
    pub now: DateTime<Utc>,
}

impl SuccessRows<'_> {
    /// A table of success rows for the filters `f` over `span`, for the
    /// events `events` (SQL string literals, comma-separated, at least
    /// one): `distinct_id`, `event`, `events` (how many) and `first_ts`
    /// (the first one), one row per person, event and UTC day.
    pub(crate) fn rows_in(&self, f: &Filters, span: Span, events: &str) -> Result<String> {
        let (skip, rolled) = board_people::split_span(&self.rolled, self.now, span, f, &self.role);
        let from_events = format!(
            "SELECT distinct_id, event, count(*) AS events, min(timestamp) AS first_ts \
             FROM {src} WHERE {w}{skip} AND event IN ({events}) \
             GROUP BY distinct_id, event, date",
            src = self.src,
            w = f.clause_span(span, self.now)?,
        );
        Ok(match rolled {
            None => format!("({from_events}) AS successes"),
            Some(w) => format!(
                "({from_events} UNION ALL SELECT person AS distinct_id, event, events, first_ts \
                 FROM success_day WHERE {w} AND event IN ({events})) AS successes"
            ),
        })
    }
}

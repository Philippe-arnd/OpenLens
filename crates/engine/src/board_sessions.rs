//! Where the board's sessions block and releases table read from (#498,
//! D-047): both count sessions, so a closed day can come from its
//! `session_day` rows (D-041) instead of its events.
//!
//! Each block reads one row per session, person, release and UTC day,
//! with the session's first and last time, interactions, errors and tasks
//! on that day: `session_day` for a day chosen as the people blocks choose
//! theirs ([`board_people::split`]), the events, grouped the same way, for
//! every other day. A session is then re-assembled across days (and
//! across both sources) before it is counted, so a session crossing
//! midnight is still one session.

use chrono::{DateTime, NaiveDate, Utc};

use super::*;

/// The board's per-session-day rows: which days come from the rollup, and
/// how to read the rest from the events.
pub(crate) struct SessionRows<'a> {
    /// The board's event source: the events, with the map's roles and features.
    pub src: &'a str,
    /// The canonical role of a raw `role`: the map's lookup, or `role`.
    pub role: String,
    /// Whether an event is a completed task, on the board's source.
    pub task: &'a str,
    /// Closed days whose `session_day` rows were rolled with the map in
    /// force, oldest first.
    pub rolled: Vec<NaiveDate>,
    /// The board's clock: windows and days are cut at this instant.
    pub now: DateTime<Utc>,
}

impl SessionRows<'_> {
    /// A table of per-session-day rows for the filters `f` over the span
    /// `span`: `session_id`, `distinct_id`, `release`, `first_ts`,
    /// `last_ts`, `n` (interactions), `errors` and `tasks`.
    pub(crate) fn rows(&self, f: &Filters, span: Span) -> Result<String> {
        let (skip, rolled) = board_people::split_span(&self.rolled, self.now, span, f, &self.role);
        let events = format!(
            "SELECT session_id, distinct_id, release, min(timestamp) AS first_ts, \
             max(timestamp) AS last_ts, sum(CASE WHEN {INTERACTION} THEN 1 ELSE 0 END) AS n, \
             sum({ERROR_COUNT}) AS errors, \
             sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS tasks \
             FROM {src} WHERE {w}{skip} AND session_id IS NOT NULL \
             GROUP BY session_id, distinct_id, release, date",
            task = self.task,
            src = self.src,
            w = f.clause_span(span, self.now)?,
        );
        let Some(w) = rolled else {
            return Ok(format!("({events}) AS d"));
        };
        Ok(format!(
            "({events} UNION ALL SELECT session_id, person AS distinct_id, release, first_ts, \
             last_ts, interactions AS n, errors, tasks FROM session_day WHERE {w}) AS d"
        ))
    }

    /// The sessions block over `span` (7 days, or the release window and
    /// its comparison, D-055): per session (by `session_id` alone), its
    /// first person, its day in the project's zone, its interactions,
    /// length, errors and tasks.
    pub(crate) fn sessions_sql(&self, f: &Filters, span: Span) -> Result<String> {
        // The day of the session's first event, in the project's zone:
        // exact from `first_ts` in any zone (D-041).
        let day = format!(
            "CAST(CAST({} AS DATE) AS VARCHAR)",
            f.local("min(first_ts)")?
        );
        Ok(format!(
            "SELECT count(*) AS sessions, \
             avg(CASE WHEN n > 1 THEN secs END) AS avg_secs, \
             avg(CASE WHEN n <= 1 THEN 1.0 ELSE 0.0 END) AS bounce_rate, \
             avg(CASE WHEN errors > 0 THEN 1.0 ELSE 0.0 END) AS error_rate, \
             sum(tasks) AS tasks, count(DISTINCT concat(uid, '|', day)) AS user_days \
             FROM (SELECT session_id, min(distinct_id) AS uid, {day} AS day, sum(n) AS n, \
                   date_part('epoch', max(last_ts)) - date_part('epoch', min(first_ts)) AS secs, \
                   sum(errors) AS errors, sum(tasks) AS tasks \
                   FROM {rows} GROUP BY session_id) s",
            rows = self.rows(f, span)?
        ))
    }

    /// The releases table over 90 days, whatever the release filter: per
    /// release and session, as the table always counted them.
    pub(crate) fn releases_sql(&self, f: &Filters) -> Result<String> {
        let any_release = Filters {
            release: None,
            ..f.clone()
        };
        Ok(format!(
            "SELECT release, CAST(min(first) AS VARCHAR) AS first_seen, count(DISTINCT uid) AS users, \
             count(*) AS sessions, avg(CASE WHEN n > 1 THEN secs END) AS avg_secs, \
             avg(CASE WHEN n <= 1 THEN 1.0 ELSE 0.0 END) AS bounce_rate, \
             avg(CASE WHEN errors > 0 THEN 1.0 ELSE 0.0 END) AS error_rate, \
             avg(CAST(tasks AS DOUBLE)) AS tasks_per_session \
             FROM (SELECT release, session_id, min(distinct_id) AS uid, min(first_ts) AS first, \
                   sum(n) AS n, \
                   date_part('epoch', max(last_ts)) - date_part('epoch', min(first_ts)) AS secs, \
                   sum(errors) AS errors, sum(tasks) AS tasks \
                   FROM {rows} WHERE release IS NOT NULL GROUP BY release, session_id) s \
             GROUP BY release ORDER BY min(first) DESC LIMIT 5",
            rows = self.rows(&any_release, Span::days(90))?
        ))
    }
}

/// A table of the first use of each feature in each session over the last
/// `days` days, for the filters `f` (#452, D-059): `session_id`,
/// `distinct_id`, `feature` and `first_ts`. A closed day comes from
/// `session_feature_day` (D-054) when it is in `rolled` (chosen as the
/// people blocks choose theirs, [`board_people::split`]), every other day
/// from the events grouped the same way; a session crossing midnight is
/// put back together (`min(first_ts)`). Events without a feature or a
/// session have no row, as in the rollup.
pub(crate) fn feature_rows(
    src: &str,
    role: &str,
    rolled: &[NaiveDate],
    now: DateTime<Utc>,
    f: &Filters,
    days: i64,
) -> Result<String> {
    let (skip, rolled) = board_people::split(rolled, now, days, f, role);
    let mut rows = format!(
        "SELECT session_id, distinct_id, feature, min(timestamp) AS first_ts \
         FROM {src} WHERE {w}{skip} AND session_id IS NOT NULL AND feature IS NOT NULL \
         GROUP BY session_id, distinct_id, feature, date",
        w = f.clause_at(days, now)?,
    );
    if let Some(w) = rolled {
        rows.push_str(&format!(
            " UNION ALL SELECT session_id, person AS distinct_id, feature, first_ts \
             FROM session_feature_day WHERE {w}"
        ));
    }
    Ok(format!(
        "(SELECT session_id, distinct_id, feature, min(first_ts) AS first_ts \
         FROM ({rows}) r GROUP BY session_id, distinct_id, feature) AS sf"
    ))
}

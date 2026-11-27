//! Task durations and resurrected users (docs/kpis.md).

use chrono::Datelike;
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;

use crate::{Filters, Result, float, int, lit, source_from, text};

/// Time to complete one feature's tasks, last 30 days.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskTime {
    /// Feature key.
    pub feature: String,
    /// Median seconds from the first entry to the success event.
    pub median_secs: f64,
    /// 90th percentile, in seconds.
    pub p90_secs: f64,
    /// Completed tasks measured.
    pub tasks: i64,
}

/// A task runs from the first use of a feature to its success event, at
/// most 30 minutes apart (longer means the task was abandoned and the
/// success belongs to a later attempt). One query for every declared
/// feature with a success event.
///
/// The success events are a scan filtered on their names, like the
/// board's success events (D-051). A feature's other events count only
/// within 30 minutes before one of the same person's successes, so only
/// those events have their feature resolved from the map: the others are
/// dropped first, on their own columns (#452, D-072). The definition and
/// the numbers are unchanged.
pub(crate) async fn task_times(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    successes: &[(String, String)],
) -> Result<Vec<TaskTime>> {
    let q = &q.with_params(&f.params()?);
    if successes.is_empty() {
        return Ok(Vec::new());
    }
    let w = f.clause(30)?;
    let since = (chrono::Utc::now() - chrono::Duration::days(30)).format("%Y-%m-%d");
    let pairs = successes
        .iter()
        .map(|(k, s)| Ok(format!("({}, {})", lit(k)?, lit(s)?)))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let events = successes
        .iter()
        .map(|(_, s)| lit(s))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let src = source_from(map, "events");
    // The events of a person within 30 minutes before one of their
    // successes, read with their own columns, before the map resolves
    // their feature.
    let near = source_from(map, "near");
    let sql = format!(
        "WITH m AS (SELECT column1 AS feature, column2 AS success FROM (VALUES {pairs})), \
         s0 AS (SELECT distinct_id, timestamp AS ts, event FROM {src} WHERE {w} AND event IN ({events})), \
         s AS (SELECT s0.distinct_id, s0.ts, m.feature FROM s0 JOIN m ON s0.event = m.success), \
         near AS (SELECT x.* FROM events x WHERE x.date >= '{since}' AND EXISTS \
                  (SELECT 1 FROM s0 WHERE s0.distinct_id = x.distinct_id AND x.timestamp <= s0.ts \
                   AND x.timestamp >= s0.ts - INTERVAL '30 minutes')), \
         ev AS (SELECT distinct_id, timestamp, event, feature FROM {near} WHERE {w}), \
         e AS (SELECT ev.distinct_id, ev.timestamp AS ts, ev.feature FROM ev JOIN m ON ev.feature = m.feature \
               WHERE ev.event <> m.success), \
         t AS (SELECT s.feature, s.distinct_id, s.ts, \
                 date_part('epoch', s.ts) - date_part('epoch', min(e.ts)) AS d \
               FROM s JOIN e ON s.distinct_id = e.distinct_id AND s.feature = e.feature \
                 AND e.ts <= s.ts AND e.ts >= s.ts - INTERVAL '30 minutes' \
               GROUP BY s.feature, s.distinct_id, s.ts) \
         SELECT feature, approx_percentile_cont(d, 0.5) AS median, approx_percentile_cont(d, 0.9) AS p90, \
                count(*) AS tasks FROM t GROUP BY feature"
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .iter()
        .filter_map(|r| {
            Some(TaskTime {
                feature: text(r, "feature")?,
                median_secs: float(r, "median"),
                p90_secs: float(r, "p90"),
                tasks: int(r, "tasks"),
            })
        })
        .collect())
}

/// Users active in the last 30 days whose previous active day was more than
/// `churn` days earlier: they had churned and came back. `rows` is a table
/// of the last 90 days with `distinct_id`, `first_ts` and `last_ts`, one row
/// per person and UTC day at most (the board's people rows, D-040, or
/// [`event_rows`]): a UTC day spans at most two days of the project's zone,
/// so a person's active days there are the days of its rows' first and last
/// events (#452, D-059).
pub(crate) async fn resurrected(
    q: &QueryEngine,
    project: &str,
    rows: &str,
    f: &Filters,
    churn: i64,
) -> Result<i64> {
    let q = &q.with_params(&f.params()?);
    let (first, last) = (f.local("first_ts")?, f.local("last_ts")?);
    let today = f.local("now()")?;
    let sql = format!(
        "WITH r AS (SELECT distinct_id, first_ts, last_ts FROM {rows}), \
         d AS (SELECT DISTINCT distinct_id, day FROM \
               (SELECT distinct_id, CAST({first} AS DATE) AS day FROM r \
                UNION ALL SELECT distinct_id, CAST({last} AS DATE) AS day FROM r) x), \
         g AS (SELECT distinct_id, day, lag(day) OVER (PARTITION BY distinct_id ORDER BY day) AS prev FROM d) \
         SELECT count(DISTINCT distinct_id) AS n FROM g \
         WHERE prev IS NOT NULL AND day >= CAST({today} AS DATE) - INTERVAL '30 days' \
           AND day - prev > {churn}"
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .first()
        .map_or(0, |r| int(r, "n")))
}

/// The events of the last 90 days as [`resurrected`]'s rows, one per event,
/// for a page that does not read the rollups.
pub(crate) fn event_rows(src: &str, f: &Filters) -> Result<String> {
    Ok(format!(
        "(SELECT distinct_id, timestamp AS first_ts, timestamp AS last_ts FROM {src} WHERE {}) AS r",
        f.clause(90)?
    ))
}

/// One weekly retention cohort: users first seen that week, and the share
/// of them active again 1, 2, … weeks later (weeks start on Monday, in the
/// project's time zone).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RetentionRow {
    /// First day of the cohort week (YYYY-MM-DD).
    pub week: String,
    /// Users first seen that week.
    pub users: i64,
    /// Share active in week 1, 2, …, up to the current week.
    pub weeks: Vec<f64>,
}

/// Weekly retention cohorts for users first seen in the last 8 weeks.
/// `rows` is the board's people rows of the last 90 days (D-040): a week
/// in which a person was active is the week of one of its rows' first or
/// last event, since a row is one UTC day, which spans two weeks at most
/// (#452, D-059).
pub(crate) async fn retention(
    q: &QueryEngine,
    project: &str,
    rows: &str,
    f: &Filters,
    ident: &str,
) -> Result<Vec<RetentionRow>> {
    let q = &q.with_params(&f.params()?);
    // Weeks start on Monday in the project's time zone (D-017).
    let first = f.local("least(min(ev.first_ts), min(p.first_seen))")?;
    let ts = f.local("act.ts")?;
    let now = f.local("now()")?;
    let sql = format!(
        "WITH ev AS (SELECT distinct_id, first_ts, last_ts FROM {rows} WHERE {ident}), \
         fs AS (SELECT ev.distinct_id, date_trunc('week', {first}) AS cw \
                FROM ev LEFT JOIN persons p ON ev.distinct_id = p.person GROUP BY ev.distinct_id), \
         act AS (SELECT distinct_id, first_ts AS ts FROM ev UNION ALL SELECT distinct_id, last_ts AS ts FROM ev), \
         a AS (SELECT DISTINCT act.distinct_id, fs.cw, \
                 CAST(round((date_part('epoch', date_trunc('week', {ts})) - date_part('epoch', fs.cw)) / 604800) AS BIGINT) AS k \
               FROM act JOIN fs ON act.distinct_id = fs.distinct_id \
               WHERE fs.cw >= date_trunc('week', {now}) - INTERVAL '49 days') \
         SELECT substr(CAST(cw AS VARCHAR), 1, 10) AS week, k, count(*) AS users FROM a GROUP BY cw, k ORDER BY cw, k"
    );
    let rows = q.sql(project, &sql).await?.rows;
    // Sorted here too: the ratios need each cohort's size (k = 0) first.
    let mut cells: Vec<(String, i64, i64)> = rows
        .iter()
        .filter_map(|r| Some((text(r, "week")?, int(r, "k"), int(r, "users"))))
        .collect();
    cells.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    let mut out: Vec<RetentionRow> = Vec::new();
    for (week, k, n) in cells {
        if out.last().is_none_or(|x| x.week != week) {
            out.push(RetentionRow {
                week,
                users: 0,
                weeks: Vec::new(),
            });
        }
        let Some(row) = out.last_mut() else { continue };
        match k {
            0 => row.users = n,
            1..=7 => {
                let i = usize::try_from(k).unwrap_or(0);
                if row.weeks.len() < i {
                    row.weeks.resize(i, 0.0);
                }
                row.weeks[i - 1] = n as f64 / row.users.max(1) as f64;
            }
            _ => {}
        }
    }
    // A cohort has as many measurable weeks as have started since it began.
    let today = f.tz.parse::<chrono_tz::Tz>().map_or_else(
        |_| chrono::Utc::now().date_naive(),
        |tz| chrono::Utc::now().with_timezone(&tz).date_naive(),
    );
    let this_week =
        today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()));
    for row in &mut out {
        let started = chrono::NaiveDate::parse_from_str(&row.week, "%Y-%m-%d").unwrap_or(this_week);
        let n = usize::try_from((this_week - started).num_days() / 7)
            .unwrap_or(0)
            .min(7);
        row.weeks.resize(n, 0.0);
    }
    Ok(out)
}

/// The week whose return the board's retention line compares (#619): a
/// month after the first visit. Fewer weeks when no two cohorts have
/// lived that long yet.
const TREND_WEEK: usize = 4;

/// Which way week-N return went from the oldest cohort to the newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Trend {
    /// The newest cohort comes back less.
    Falling,
    /// The newest cohort comes back more.
    Rising,
}

/// The board's retention line (#619): week-N return of the oldest and the
/// newest cohort for which week N is over, and which way it went.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RetentionTrend {
    /// N: the week after the first visit that is compared (1 = the week
    /// after the cohort's own week).
    pub week: usize,
    /// The oldest complete cohort's week (YYYY-MM-DD) and its week-N share.
    pub oldest: (String, f64),
    /// The newest complete cohort's week and its week-N share.
    pub newest: (String, f64),
    /// None when both shares read the same at 0.1 %.
    pub trend: Option<Trend>,
}

/// The retention line from the cohorts (#619). A cohort counts for week N
/// only when week N is over: its `weeks` then holds week N + 1 too (the
/// last entry is the week in progress, see [`RetentionRow::weeks`]), so a
/// newest cohort still living its week N is skipped, not compared on a
/// partial week. Week [`TREND_WEEK`] first, then earlier weeks; none when
/// no week has two complete cohorts with users.
#[must_use]
pub fn retention_trend(rows: &[RetentionRow]) -> Option<RetentionTrend> {
    (1..=TREND_WEEK).rev().find_map(|n| {
        let done: Vec<(&str, f64)> = rows
            .iter()
            .filter(|r| r.users > 0 && r.weeks.len() > n)
            .filter_map(|r| Some((r.week.as_str(), *r.weeks.get(n - 1)?)))
            .collect();
        if done.len() < 2 {
            return None;
        }
        // Weeks are YYYY-MM-DD: their text order is their date order.
        let oldest = done.iter().min_by(|a, b| a.0.cmp(b.0))?;
        let newest = done.iter().max_by(|a, b| a.0.cmp(b.0))?;
        // Compared as printed (0.1 %), so the tag never contradicts the
        // two numbers beside it.
        let shown = |s: f64| (s * 1000.0).round();
        let trend = match shown(newest.1).total_cmp(&shown(oldest.1)) {
            std::cmp::Ordering::Less => Some(Trend::Falling),
            std::cmp::Ordering::Greater => Some(Trend::Rising),
            std::cmp::Ordering::Equal => None,
        };
        Some(RetentionTrend {
            week: n,
            oldest: (oldest.0.to_owned(), oldest.1),
            newest: (newest.0.to_owned(), newest.1),
            trend,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cohort of `users` that has lived `weeks.len()` weeks since its own.
    fn row(week: &str, users: i64, weeks: &[f64]) -> RetentionRow {
        RetentionRow {
            week: week.to_owned(),
            users,
            weeks: weeks.to_vec(),
        }
    }

    #[test]
    fn the_line_compares_complete_cohorts_only() {
        let rows = [
            row("2026-10-05", 40, &[0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2]),
            row("2026-10-12", 30, &[0.8, 0.7, 0.6, 0.5, 0.4, 0.3]),
            row("2026-10-19", 20, &[0.8, 0.7, 0.6, 0.45, 0.4]),
            // Living its week 4 now: a partial week, not compared.
            row("2026-10-26", 10, &[0.9, 0.9, 0.9, 0.01]),
            row("2026-11-02", 0, &[0.0, 0.0, 0.0, 0.0, 0.0]),
        ];
        let t = retention_trend(&rows).unwrap();
        let (complete, partial) = (&rows[2], &rows[3]);
        assert!(t.week < complete.weeks.len());
        assert!(t.week >= partial.weeks.len(), "{t:?}");
        assert_eq!(t.newest.0, complete.week);
        assert_eq!(t.oldest.0, rows[0].week);
        assert_eq!(t.newest.1, complete.weeks[t.week - 1]);
        // Removing the partial cohort changes nothing.
        let without: Vec<_> = rows
            .iter()
            .filter(|r| r.week != partial.week)
            .cloned()
            .collect();
        assert_eq!(retention_trend(&without), Some(t));
    }

    #[test]
    fn the_tag_follows_the_comparison() {
        let at = |old: f64, new: f64| {
            let rows = [
                row("2026-10-05", 10, &[old, old]),
                row("2026-10-12", 10, &[new, new]),
            ];
            retention_trend(&rows).unwrap()
        };
        let down = at(0.6, 0.4);
        assert!(down.newest.1 < down.oldest.1);
        assert_eq!(down.trend, Some(Trend::Falling));
        let up = at(0.4, 0.6);
        assert!(up.newest.1 > up.oldest.1);
        assert_eq!(up.trend, Some(Trend::Rising));
        assert_eq!(at(0.5, 0.5).trend, None);
    }

    #[test]
    fn one_complete_cohort_makes_no_line() {
        // Week 1 is over for the first cohort only.
        let rows = [
            row("2026-11-09", 10, &[0.5, 0.2]),
            row("2026-11-16", 10, &[0.5]),
            row("2026-11-23", 10, &[]),
        ];
        assert_eq!(retention_trend(&rows), None);
        assert_eq!(retention_trend(&[]), None);
    }
}

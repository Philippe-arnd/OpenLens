//! Task durations and resurrected users (docs/kpis.md).

use chrono::Datelike;
use openlens_query::QueryEngine;

use crate::{Filters, Result, float, int, lit, text};

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
pub(crate) async fn task_times(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    successes: &[(String, String)],
) -> Result<Vec<TaskTime>> {
    if successes.is_empty() {
        return Ok(Vec::new());
    }
    let w = f.clause(30)?;
    let pairs = successes
        .iter()
        .map(|(k, s)| Ok(format!("({}, {})", lit(k)?, lit(s)?)))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let sql = format!(
        "WITH m AS (SELECT column1 AS feature, column2 AS success FROM (VALUES {pairs})), \
         ev AS (SELECT distinct_id, timestamp, event, feature FROM {src} WHERE {w}), \
         s AS (SELECT ev.distinct_id, ev.timestamp AS ts, m.feature FROM ev JOIN m ON ev.event = m.success), \
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
/// `churn` days earlier: they had churned and came back.
pub(crate) async fn resurrected(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    churn: i64,
) -> Result<i64> {
    let w = f.clause(90)?;
    let day = f.day()?;
    let today = f.local("now()")?;
    let sql = format!(
        "WITH d AS (SELECT DISTINCT distinct_id, CAST({day} AS DATE) AS day FROM {src} WHERE {w}), \
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
pub(crate) async fn retention(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    ident: &str,
) -> Result<Vec<RetentionRow>> {
    let w = f.clause(90)?;
    // Weeks start on Monday in the project's time zone (D-017).
    let first = f.local("least(min(ev.timestamp), min(p.first_seen))")?;
    let ts = f.local("ev.timestamp")?;
    let now = f.local("now()")?;
    let sql = format!(
        "WITH ev AS (SELECT distinct_id, timestamp FROM {src} WHERE {w} AND {ident}), \
         fs AS (SELECT ev.distinct_id, date_trunc('week', {first}) AS cw \
                FROM ev LEFT JOIN persons p ON ev.distinct_id = p.person GROUP BY ev.distinct_id), \
         a AS (SELECT DISTINCT ev.distinct_id, fs.cw, \
                 CAST(round((date_part('epoch', date_trunc('week', {ts})) - date_part('epoch', fs.cw)) / 604800) AS BIGINT) AS k \
               FROM ev JOIN fs ON ev.distinct_id = fs.distinct_id \
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

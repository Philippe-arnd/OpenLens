//! Task durations and resurrected users (docs/kpis.md).

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
    let sql = format!(
        "WITH d AS (SELECT DISTINCT distinct_id, CAST(date AS DATE) AS day FROM {src} WHERE {w}), \
         g AS (SELECT distinct_id, day, lag(day) OVER (PARTITION BY distinct_id ORDER BY day) AS prev FROM d) \
         SELECT count(DISTINCT distinct_id) AS n FROM g \
         WHERE prev IS NOT NULL AND day >= CAST(now() AS DATE) - INTERVAL '30 days' \
           AND day - prev > {churn}"
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .first()
        .map_or(0, |r| int(r, "n")))
}

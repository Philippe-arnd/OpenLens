//! Flag cohorts: the sessions of each flag against the rest, for the flag
//! cards and for tickets shipped behind a flag (#704). Moved out of
//! `insights.rs` as is (file length, #307).

use super::*;
use crate::ERROR_COUNT;

/// Compare the sessions of each flag seen lately (at most 5) with the rest.
pub(crate) async fn flag_cohorts(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Vec<FlagCohort>> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause(30)?;
    let seen = q
        .sql(
            project,
            &format!("SELECT DISTINCT flags FROM {src} WHERE {w} AND flags IS NOT NULL LIMIT 200"),
        )
        .await?;
    let mut keys: Vec<String> = seen
        .rows
        .iter()
        .filter_map(|r| text(r, "flags"))
        .flat_map(|s| s.split(',').map(str::to_owned).collect::<Vec<_>>())
        .filter(|k| valid_flag(k))
        .collect();
    keys.sort();
    keys.dedup();
    keys.truncate(5);
    let runs = keys.iter().map(|key| cohort_of(q, project, src, &w, key));
    futures::future::join_all(runs).await.into_iter().collect()
}

/// One flag's sessions against the rest over the last `days` days (1 to
/// 112), for a ticket shipped behind it (#704): `None` when the key is not
/// a flag key.
pub(crate) async fn flag_cohort(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    key: &str,
    days: i64,
) -> Result<Option<FlagCohort>> {
    if !valid_flag(key) {
        return Ok(None);
    }
    let q = &q.with_params(&f.params()?);
    let w = f.clause(days.clamp(1, 112))?;
    cohort_of(q, project, src, &w, key).await.map(Some)
}

/// The sessions with `key` on against the others, within `w`. `key` must
/// have passed [`valid_flag`].
pub(super) async fn cohort_of(
    q: &QueryEngine,
    project: &str,
    src: &str,
    w: &str,
    key: &str,
) -> Result<FlagCohort> {
    // `key` is restricted to [a-z0-9_-], so it is safe inside the literal.
    let sql = format!(
        "SELECT has, count(DISTINCT uid) AS users, count(*) AS sessions, \
         avg(CASE WHEN n > 1 THEN secs END) AS avg_secs, avg(CAST(tasks AS DOUBLE)) AS tps, stddev(CAST(tasks AS DOUBLE)) AS tps_sd, \
         avg(CASE WHEN errors > 0 THEN 1.0 ELSE 0.0 END) AS err \
         FROM (SELECT session_id, min(distinct_id) AS uid, \
               max(CASE WHEN strpos(concat(',', coalesce(flags, ''), ','), ',{key},') > 0 THEN 1 ELSE 0 END) AS has, \
               count(*) AS n, date_part('epoch', max(timestamp)) - date_part('epoch', min(timestamp)) AS secs, \
               sum({ERROR_COUNT}) AS errors, \
               sum(CASE WHEN feature IS NOT NULL THEN 1 ELSE 0 END) AS tasks \
               FROM {src} WHERE {w} AND session_id IS NOT NULL GROUP BY session_id) s GROUP BY has"
    );
    let res = q.sql(project, &sql).await?;
    let mut c = FlagCohort {
        flag: key.to_owned(),
        with: CohortStats::default(),
        without: CohortStats::default(),
        p_errors: None,
        p_tasks: None,
    };
    for r in res.rows {
        let stats = CohortStats {
            users: int(&r, "users"),
            sessions: int(&r, "sessions"),
            avg_secs: r.get("avg_secs").and_then(Value::as_f64).unwrap_or(0.0),
            tasks_per_session: r.get("tps").and_then(Value::as_f64).unwrap_or(0.0),
            error_rate: r.get("err").and_then(Value::as_f64).unwrap_or(0.0),
            tasks_sd: r.get("tps_sd").and_then(Value::as_f64).unwrap_or(0.0),
        };
        if int(&r, "has") == 1 {
            c.with = stats
        } else {
            c.without = stats
        }
    }
    let (w, o) = (&c.with, &c.without);
    c.p_errors = crate::stats::two_proportions(w.error_rate, w.sessions, o.error_rate, o.sessions);
    c.p_tasks = crate::stats::two_means(
        w.tasks_per_session,
        w.tasks_sd,
        w.sessions,
        o.tasks_per_session,
        o.tasks_sd,
        o.sessions,
    );
    Ok(c)
}

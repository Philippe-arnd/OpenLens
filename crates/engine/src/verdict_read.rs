//! What the release verdict reads (#522, D-067): the numbers of every
//! release of one environment over the last 90 days, in four grouped
//! queries, then each release compared with the one before it
//! ([`crate::verdict::compare`]).
//!
//! - **Sessions** (users, bounces, errors, tasks): `session_day` for the
//!   closed days rolled with the map in force, the events for the others
//!   (D-047), as the board's releases table reads them.
//! - **Features with a success event**: `session_feature_day` the same
//!   way (D-054).
//! - **New error groups**: a scan of the `$error` events only, grouped by
//!   the Errors page's fingerprint; a group is new in the release of its
//!   first occurrence in the 90 days.
//! - **Web Vitals**: a scan of the `$vital` events only, p75 per release.
//!
//! The team's own users are left out, as on the board.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;

use crate::board_people;
use crate::board_sessions::SessionRows;
use crate::errors::FINGERPRINT;
use crate::goals::Goal;
use crate::releases::Release;
use crate::verdict::{self, FeatureUse, NewError, Report, Side, VitalP75};
use crate::{Filters, Result, Span, float, int, lit, lookups, source, text};

/// Days read (the releases' window).
const DAYS: i64 = crate::releases::WINDOW_DAYS;
/// Rows read at most per query.
const MAX_ROWS: usize = 5_000;

/// The numbers of each release of `environment` over the last 90 days, by
/// release name.
async fn sides(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    map: Option<(&FeatureMap, i64)>,
    now: DateTime<Utc>,
) -> Result<HashMap<String, Side>> {
    let f = Filters {
        environment: environment.to_owned(),
        ..Filters::default()
    };
    let q = &q.with_params(&f.params()?);
    let src = source(map.map(|(m, _)| m));
    let role = map.map_or_else(|| "role".to_owned(), |(m, _)| lookups(m).0);
    // Completed tasks as the board counts them: the map's success events,
    // or any feature use without them.
    let successes: Vec<(String, String)> = map
        .map(|(m, _)| {
            m.features
                .iter()
                .filter_map(|x| Some((x.key.clone(), x.success.clone()?)))
                .collect()
        })
        .unwrap_or_default();
    let task = if successes.is_empty() {
        "feature IS NOT NULL".to_owned()
    } else {
        let list = successes
            .iter()
            .map(|(_, e)| lit(e))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        format!("event IN ({list})")
    };
    let version = map.map(|(_, v)| v);
    let (rolled_sessions, rolled_features) = futures::try_join!(
        board_people::rolled_days(q, project, "session_day", version, now),
        board_people::rolled_days(q, project, "session_feature_day", version, now),
    )?;
    let sessions = SessionRows {
        src: &src,
        role: role.clone(),
        task: &task,
        rolled: rolled_sessions,
        now,
    };
    let kpis_sql = format!(
        "SELECT release, count(DISTINCT uid) AS users, count(*) AS sessions, \
         sum(CASE WHEN n <= 1 THEN 1 ELSE 0 END) AS bounced, \
         sum(CASE WHEN errors > 0 THEN 1 ELSE 0 END) AS bad, \
         sum(CASE WHEN tasks > 0 THEN 1 ELSE 0 END) AS with_task, sum(tasks) AS tasks, \
         avg(CASE WHEN n > 1 THEN secs END) AS avg_secs \
         FROM (SELECT release, session_id, min(distinct_id) AS uid, sum(n) AS n, \
               date_part('epoch', max(last_ts)) - date_part('epoch', min(first_ts)) AS secs, \
               sum(errors) AS errors, sum(tasks) AS tasks \
               FROM {rows} WHERE release IS NOT NULL GROUP BY release, session_id) s \
         GROUP BY release ORDER BY sessions DESC LIMIT {MAX_ROWS}",
        rows = sessions.rows(&f, Span::days(DAYS))?
    );
    let w = f.clause_at(DAYS, now)?;
    let errors_sql = format!(
        "WITH e AS (SELECT distinct_id, release, timestamp, error_message AS msg, error_source AS source \
           FROM {src} WHERE {w} AND event = '$error'), \
         g AS (SELECT distinct_id, release, timestamp, msg, {FINGERPRINT} AS fp FROM e), \
         fr AS (SELECT fp, first_value(release ORDER BY timestamp) AS rel FROM g GROUP BY fp) \
         SELECT g.release AS release, g.fp AS fp, min(g.msg) AS sample, \
         count(DISTINCT g.distinct_id) AS users, count(*) AS n \
         FROM g JOIN fr ON g.fp = fr.fp AND g.release = fr.rel \
         GROUP BY g.release, g.fp ORDER BY users DESC, n DESC LIMIT {MAX_ROWS}"
    );
    let vitals_sql = format!(
        "SELECT release, vital_name AS name, approx_percentile_cont(vital_value, 0.75) AS p75, \
         count(*) AS samples FROM {src} WHERE {w} AND event = '$vital' AND release IS NOT NULL \
         AND vital_name IN ('LCP', 'INP', 'CLS') AND vital_value IS NOT NULL \
         GROUP BY release, vital_name LIMIT {MAX_ROWS}"
    );
    // Features with a success event: sessions that used each, and those
    // that reached it, per release.
    let features_sql = if successes.is_empty() {
        None
    } else {
        let (skip, rolled) = board_people::split(&rolled_features, now, DAYS, &f, &role);
        let mut rows = format!(
            "SELECT session_id, release, feature, sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS ok \
             FROM {src} WHERE {w}{skip} AND session_id IS NOT NULL AND feature IS NOT NULL \
             GROUP BY session_id, release, feature, date"
        );
        if let Some(rw) = rolled {
            rows.push_str(&format!(
                " UNION ALL SELECT session_id, release, feature, successes AS ok \
                 FROM session_feature_day WHERE {rw}"
            ));
        }
        Some(format!(
            "SELECT release, feature, count(*) AS sessions, \
             sum(CASE WHEN ok > 0 THEN 1 ELSE 0 END) AS succeeded \
             FROM (SELECT release, session_id, feature, sum(ok) AS ok FROM ({rows}) r \
                   WHERE release IS NOT NULL GROUP BY release, session_id, feature) s \
             GROUP BY release, feature ORDER BY sessions DESC LIMIT {MAX_ROWS}"
        ))
    };
    let features_fut = async {
        match &features_sql {
            Some(s) => Some(q.sql(project, s).await),
            None => None,
        }
    };
    let (kpis, errors, vitals, features) = futures::join!(
        q.sql(project, &kpis_sql),
        q.sql(project, &errors_sql),
        q.sql(project, &vitals_sql),
        features_fut,
    );
    let mut out: HashMap<String, Side> = HashMap::new();
    for r in &kpis?.rows {
        let release = text(r, "release").unwrap_or_default();
        out.insert(
            release.clone(),
            Side {
                release,
                users: int(r, "users"),
                sessions: int(r, "sessions"),
                bounced: int(r, "bounced"),
                error_sessions: int(r, "bad"),
                task_sessions: int(r, "with_task"),
                tasks: int(r, "tasks"),
                avg_secs: float(r, "avg_secs"),
                ..Side::default()
            },
        );
    }
    for r in &errors?.rows {
        if let Some(s) = text(r, "release").and_then(|x| out.get_mut(&x)) {
            s.new_errors.push(NewError {
                fingerprint: text(r, "fp").unwrap_or_default(),
                sample: text(r, "sample").unwrap_or_else(|| "(no message)".into()),
                users: int(r, "users"),
                count: int(r, "n"),
            });
        }
    }
    for r in &vitals?.rows {
        if let Some(s) = text(r, "release").and_then(|x| out.get_mut(&x)) {
            s.vitals.push(VitalP75 {
                name: text(r, "name").unwrap_or_default(),
                p75: float(r, "p75"),
                samples: int(r, "samples"),
            });
        }
    }
    if let Some(features) = features {
        for r in &features?.rows {
            let feature = text(r, "feature").unwrap_or_default();
            if !successes.iter().any(|(k, _)| *k == feature) {
                continue;
            }
            if let Some(s) = text(r, "release").and_then(|x| out.get_mut(&x)) {
                s.features.push(FeatureUse {
                    feature,
                    sessions: int(r, "sessions"),
                    succeeded: int(r, "succeeded"),
                });
            }
        }
    }
    Ok(out)
}

/// Which releases to judge and with what.
#[derive(Clone, Copy)]
pub struct Scope<'a> {
    /// The environment judged.
    pub environment: &'a str,
    /// The zone `releases.list` wrote its times in.
    pub tz: &'a str,
    /// The project's feature map and its version.
    pub map: Option<(&'a FeatureMap, i64)>,
    /// The project's goals (they count in production only).
    pub goals: &'a [Goal],
}

/// Judge every release of the scope's environment with traffic in
/// `releases` (as `releases.list` returns them) against the one before
/// it, at `now`.
///
/// # Errors
/// A failed query.
pub async fn judge(
    q: &QueryEngine,
    project: &str,
    scope: Scope<'_>,
    releases: &[Release],
    now: DateTime<Utc>,
) -> Result<Vec<Report>> {
    let Scope {
        environment,
        tz,
        map,
        goals,
    } = scope;
    let mut ours: Vec<(DateTime<Utc>, &Release)> = releases
        .iter()
        .filter(|r| r.environment == environment && r.users > 0)
        .filter_map(|r| Some((crate::board_window::parse_local(&r.first_seen, tz)?, r)))
        .collect();
    if ours.is_empty() {
        return Ok(Vec::new());
    }
    ours.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.release.cmp(&b.1.release)));
    let sides = sides(q, project, environment, map, now).await?;
    let goals = if environment == crate::releases::PRODUCTION {
        goals
    } else {
        &[]
    };
    let empty = |name: &str| Side {
        release: name.to_owned(),
        ..Side::default()
    };
    let mut out = Vec::with_capacity(ours.len());
    for (i, (first, r)) in ours.iter().enumerate() {
        let current = sides
            .get(&r.release)
            .cloned()
            .unwrap_or_else(|| empty(&r.release));
        let previous = i.checked_sub(1).and_then(|j| ours.get(j)).map(|(_, p)| {
            sides
                .get(&p.release)
                .cloned()
                .unwrap_or_else(|| empty(&p.release))
        });
        let age = (now - *first).num_hours().max(0);
        out.push(verdict::compare(
            environment,
            &current,
            previous.as_ref(),
            age,
            goals,
        ));
    }
    Ok(out)
}

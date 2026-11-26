//! Where the feature map's numbers read from (#483, D-054): its users,
//! uses and successes per feature, the `requires` pairs, the roles of a
//! feature and the relations between features come from the daily
//! rollups for the closed days rolled up with the map in force, and from
//! the events for the other days, as the board's blocks do (D-040, D-047,
//! with the same per-day choice).
//!
//! - People rows (`person_day`, D-038): one per person, role, feature and
//!   day, with events and successes, over 60 days split at 30.
//! - Session rows (`session_feature_day`, D-054): one per session and
//!   feature, with the first use and the successes, over 30 days; the
//!   relations are added up in SQL from them, with no cap on sessions.
//!
//! The split, merge and step order suggestions, drift, coverage and the
//! pages, errors, sessions and flags of a feature still read the events.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;

use crate::relations::{Counts, MAX_PER_SESSION, MIN_TOGETHER, Pair, Relation};
use crate::{Filters, Result, board_people, int, lit, lookups, source, text};

/// The closed days the map's numbers can read from the rollups.
#[derive(Debug, Clone)]
pub(crate) struct MapDays {
    /// Days whose `person_day` was rolled with the map in force.
    person: Vec<NaiveDate>,
    /// Days whose `session_feature_day` was rolled with the map in force.
    sessions: Vec<NaiveDate>,
    /// The clock: windows and days are cut at this instant.
    now: DateTime<Utc>,
}

impl MapDays {
    /// The rolled-up days of `project` for the map version `version`; none
    /// (every day from the events) without a version.
    pub(crate) async fn load(q: &QueryEngine, project: &str, version: Option<i64>) -> Result<Self> {
        let now = Utc::now();
        let Some(v) = version else {
            return Ok(Self {
                person: Vec::new(),
                sessions: Vec::new(),
                now,
            });
        };
        let (person, sessions) = futures::try_join!(
            board_people::rolled_days(q, project, "person_day", Some(v), now),
            board_people::rolled_days(q, project, "session_feature_day", Some(v), now),
        )?;
        Ok(Self {
            person,
            sessions,
            now,
        })
    }

    /// The days of the 60 the map's numbers read that count nobody: no
    /// events left, and no rollups made with the map in force (#489, D-070).
    pub(crate) async fn unavailable(&self, q: &QueryEngine, project: &str) -> Result<Vec<String>> {
        let from = (self.now - Duration::days(59)).date_naive();
        crate::unavailable::days(q, project, from, &[&self.person, &self.sessions], self.now).await
    }

    /// A table of people rows over the last 60 days: `distinct_id`, `role`
    /// (canonical), `feature`, `events`, `successes` and `cur` (the row is
    /// in the last 30 days). The day the 30 days start in is read from its
    /// events, so every row is on one side.
    pub(crate) fn people(&self, m: &FeatureMap, f: &Filters) -> Result<String> {
        let (src, role, task) = (source(Some(m)), lookups(m).0, task(m)?);
        let edge = (self.now - Duration::days(30)).date_naive();
        let rolled: Vec<NaiveDate> = self.person.iter().copied().filter(|d| *d != edge).collect();
        let (skip, rolled) = board_people::split(&rolled, self.now, 60, f, &role);
        let cur = "timestamp >= now() - INTERVAL '30 days'";
        let events = format!(
            "SELECT distinct_id, role, feature, count(*) AS events, \
             sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS successes, {cur} AS cur \
             FROM {src} WHERE {w}{skip} GROUP BY distinct_id, role, feature, {cur}",
            w = f.clause_at(60, self.now)?,
        );
        let Some(w) = rolled else {
            return Ok(format!("({events}) AS people"));
        };
        Ok(format!(
            "({events} UNION ALL SELECT person AS distinct_id, {role} AS role, feature, events, \
             successes, date > '{edge}' AS cur FROM person_day WHERE {w}) AS people",
            edge = edge.format("%Y-%m-%d"),
        ))
    }

    /// The numbers [`crate::maphealth::assess`] takes per feature: uses and
    /// users (`by_feature`), users who succeeded (`success`), and the
    /// (user, feature) pairs of the features in `pair_keys` (SQL literals).
    pub(crate) async fn usage(
        &self,
        q: &QueryEngine,
        project: &str,
        m: &FeatureMap,
        f: &Filters,
        pair_keys: &[String],
        u: &mut crate::maphealth::Usage,
    ) -> Result<()> {
        let people = self.people(m, f)?;
        let by_feature = format!(
            "SELECT feature, cur, sum(events) AS n, count(DISTINCT distinct_id) AS u \
             FROM {people} GROUP BY feature, cur"
        );
        let with_success = keys(m, |x| x.success.is_some())?;
        let success = format!(
            "SELECT feature, cur, count(DISTINCT CASE WHEN successes > 0 THEN distinct_id END) AS u \
             FROM {people} WHERE feature IN ({}) GROUP BY feature, cur",
            with_success.join(", ")
        );
        let pairs = format!(
            "SELECT DISTINCT distinct_id, feature FROM {people} WHERE cur AND feature IN ({}) LIMIT 200000",
            pair_keys.join(", ")
        );
        // A query left out (no feature to ask about) has no rows.
        let run = |sql: Option<String>| async move {
            match sql {
                Some(s) => q.sql(project, &s).await.map(|r| r.rows),
                None => Ok(Vec::new()),
            }
        };
        let (a, b, c) = futures::join!(
            run(Some(by_feature)),
            run((!with_success.is_empty()).then_some(success)),
            run((!pair_keys.is_empty()).then_some(pairs)),
        );
        let cur = |r: &serde_json::Map<String, serde_json::Value>| {
            r.get("cur")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        for r in &a? {
            u.by_feature
                .push((text(r, "feature"), cur(r), int(r, "n"), int(r, "u")));
        }
        for r in &b? {
            if let Some(k) = text(r, "feature") {
                u.success.push((k, cur(r), int(r, "u")));
            }
        }
        for r in &c? {
            if let (Some(d), Some(k)) = (text(r, "distinct_id"), text(r, "feature")) {
                u.pairs.push((d, k));
            }
        }
        Ok(())
    }

    /// Users and users who succeeded (when `success`: it declares a
    /// success event) per canonical role of the feature `key`, last 30
    /// days, most users first.
    pub(crate) fn roles_sql(
        &self,
        m: &FeatureMap,
        f: &Filters,
        key: &str,
        success: bool,
    ) -> Result<String> {
        let key = lit(key)?;
        let s = if success {
            "count(DISTINCT CASE WHEN successes > 0 THEN distinct_id END)"
        } else {
            "0"
        };
        Ok(format!(
            "SELECT coalesce(role, '(anonymous)') AS role, count(DISTINCT distinct_id) AS u, {s} AS s \
             FROM {people} WHERE cur AND feature = {key} GROUP BY 1 ORDER BY u DESC LIMIT 20",
            people = self.people(m, f)?
        ))
    }

    /// The relations' counts over the last 30 days: per session, its first
    /// [`MAX_PER_SESSION`] declared features, added up in SQL.
    pub(crate) async fn relations(
        &self,
        q: &QueryEngine,
        project: &str,
        m: &FeatureMap,
        f: &Filters,
    ) -> Result<Counts> {
        let declared = keys(m, |_| true)?;
        if declared.is_empty() {
            return Ok(Counts::default());
        }
        let (src, role, task) = (source(Some(m)), lookups(m).0, task(m)?);
        let (skip, rolled) = board_people::split(&self.sessions, self.now, 30, f, &role);
        let mut rows = format!(
            "SELECT session_id, feature, min(timestamp) AS first_ts, \
             sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS successes \
             FROM {src} WHERE {w}{skip} AND session_id IS NOT NULL AND feature IS NOT NULL \
             GROUP BY session_id, feature",
            w = f.clause_at(30, self.now)?,
        );
        if let Some(w) = rolled {
            rows.push_str(&format!(
                " UNION ALL SELECT session_id, feature, first_ts, successes \
                 FROM session_feature_day WHERE {w}"
            ));
        }
        let with = format!(
            "WITH r AS ({rows}), \
             s AS (SELECT session_id, feature, min(first_ts) AS t, \
                   max(CASE WHEN successes > 0 THEN 1 ELSE 0 END) AS ok \
                   FROM r WHERE feature IN ({list}) GROUP BY session_id, feature), \
             k AS (SELECT session_id, feature, t, ok FROM \
                   (SELECT session_id, feature, t, ok, \
                    row_number() OVER (PARTITION BY session_id ORDER BY t, feature) AS rk FROM s) x \
                   WHERE rk <= {MAX_PER_SESSION}) ",
            list = declared.join(", ")
        );
        let total = format!("{with}SELECT count(DISTINCT session_id) AS n FROM r");
        let per =
            format!("{with}SELECT feature, count(*) AS n, sum(ok) AS ok FROM k GROUP BY feature");
        let pairs = format!(
            "{with}SELECT a.feature AS x, b.feature AS y, count(*) AS n, \
             sum(CASE WHEN a.t <= b.t THEN 1 ELSE 0 END) AS xf, \
             sum(CASE WHEN b.t <= a.t THEN 1 ELSE 0 END) AS yf, \
             sum(CASE WHEN a.t <= b.t THEN b.ok ELSE 0 END) AS y_after, \
             sum(CASE WHEN b.t <= a.t THEN a.ok ELSE 0 END) AS x_after, \
             sum(a.ok) AS x_ok, sum(b.ok) AS y_ok \
             FROM k a JOIN k b ON a.session_id = b.session_id AND a.feature < b.feature \
             GROUP BY a.feature, b.feature HAVING count(*) >= {MIN_TOGETHER} \
             ORDER BY n DESC LIMIT 5000"
        );
        let (t, p, x) = futures::join!(
            q.sql(project, &total),
            q.sql(project, &per),
            q.sql(project, &pairs)
        );
        let mut c = Counts {
            sessions: t?.rows.first().map_or(0, |r| int(r, "n")),
            ..Counts::default()
        };
        for r in &p?.rows {
            if let Some(k) = text(r, "feature") {
                c.features.insert(k, (int(r, "n"), int(r, "ok")));
            }
        }
        for r in &x?.rows {
            if let (Some(a), Some(b)) = (text(r, "x"), text(r, "y")) {
                let pair = Pair {
                    both: int(r, "n"),
                    x_first: int(r, "xf"),
                    y_first: int(r, "yf"),
                    y_ok_after: int(r, "y_after"),
                    x_ok_after: int(r, "x_after"),
                    x_ok: int(r, "x_ok"),
                    y_ok: int(r, "y_ok"),
                };
                c.pairs.insert((a, b), pair);
            }
        }
        Ok(c)
    }
}

/// Whether an event (feature resolved) is a task, as the rollups count
/// them (D-038): the map's success events, or any event with a feature
/// when it declares none.
fn task(m: &FeatureMap) -> Result<String> {
    let list = keys_of(m.features.iter().filter_map(|x| x.success.as_deref()))?;
    Ok(if list.is_empty() {
        "feature IS NOT NULL".to_owned()
    } else {
        format!("event IN ({})", list.join(", "))
    })
}

/// The keys of the map's features that pass `keep`, as SQL literals.
fn keys(
    m: &FeatureMap,
    keep: impl Fn(&openlens_featuremap::Feature) -> bool,
) -> Result<Vec<String>> {
    keys_of(
        m.features
            .iter()
            .filter(|x| keep(x))
            .map(|x| x.key.as_str()),
    )
}

/// `values`, deduplicated, as SQL literals.
fn keys_of<'a>(values: impl Iterator<Item = &'a str>) -> Result<Vec<String>> {
    values
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(lit)
        .collect()
}

/// The relations between the map's features (#283, #483), alone: from the
/// session rollup for the closed days rolled up with the map version
/// `version` (none: every day from the events), and the events for the
/// rest. Most shared sessions first.
pub async fn feature_relations(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    version: Option<i64>,
    f: &Filters,
) -> Result<Vec<Relation>> {
    let q = &q.with_params(&f.params()?);
    let days = MapDays::load(q, project, version).await?;
    let c = days.relations(q, project, m, f).await?;
    Ok(crate::relations::from_counts(m, &c))
}

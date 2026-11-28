//! Computing one closed day's rollups (#322) from its events. Where they
//! are written, and when they go away, is `openlens_storage::rollup`.

use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;

use crate::{QueryEngine, QueryError, offload};

/// `person_q` of one UTC day, from the `events` view (people already
/// merged with their anonymous visits).
fn person_sql(day: &str) -> String {
    format!(
        "SELECT {bucket} AS bucket, distinct_id AS person, environment, release, role, \
         count(*) AS events, \
         sum({ERROR_COUNT}) AS errors, \
         min(timestamp) AS first_ts, max(timestamp) AS last_ts \
         FROM events WHERE date = '{day}' \
         GROUP BY 1, 2, 3, 4, 5",
        bucket = bucket()
    )
}

/// How `person_day` resolves features and tasks (#451): the feature map in
/// force when the day is rolled up, written out by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayFeatures {
    /// SQL expression over the `events` view giving an event's feature
    /// (the map's lookup, or plain `feature` without a map). Trusted SQL
    /// built by the engine, never user text.
    pub feature: String,
    /// The map's success events; empty means "every event with a feature
    /// is a task", as on the board.
    pub successes: Vec<String>,
    /// The version of the map, `None` without one.
    pub map_version: Option<i64>,
}

impl Default for DayFeatures {
    /// No feature map: the feature the event carried.
    fn default() -> Self {
        Self {
            feature: "feature".into(),
            successes: Vec::new(),
            map_version: None,
        }
    }
}

/// `person_day` of one UTC day (#451): per person, environment, release,
/// role and feature, from the `events` view (people already merged).
fn person_day_sql(day: &str, f: &DayFeatures) -> Result<String, QueryError> {
    let (task, version) = (task_sql(f)?, version_sql(f));
    Ok(format!(
        "SELECT person, environment, release, role, feature, count(*) AS events, \
         count(DISTINCT session_id) AS sessions, \
         sum({ERROR_COUNT}) AS errors, \
         sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS successes, \
         min(timestamp) AS first_ts, max(timestamp) AS last_ts, \
         CAST({version} AS BIGINT) AS map_version \
         FROM (SELECT distinct_id AS person, environment, release, role, \
               {feature} AS feature, event, session_id, timestamp \
               FROM events WHERE date = '{day}') e \
         GROUP BY 1, 2, 3, 4, 5",
        feature = f.feature
    ))
}

/// Whether an event (with its `feature` already resolved) is a task, as
/// the board counts them: the map's success events, or any event with a
/// feature when the map declares none.
fn task_sql(f: &DayFeatures) -> Result<String, QueryError> {
    Ok(success_sql(f)?.unwrap_or_else(|| "feature IS NOT NULL".to_owned()))
}

/// Whether an event is one of the map's success events; none when the map
/// declares none.
fn success_sql(f: &DayFeatures) -> Result<Option<String>, QueryError> {
    if f.successes.is_empty() {
        return Ok(None);
    }
    let list = f
        .successes
        .iter()
        .map(|e| openlens_sqlgen::lit(e))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| QueryError::Rejected("not a valid success event".into()))?;
    Ok(Some(format!("event IN ({})", list.join(", "))))
}

/// The map's version as a SQL literal (`NULL` without a map).
fn version_sql(f: &DayFeatures) -> String {
    f.map_version
        .map_or_else(|| "NULL".to_owned(), |v| v.to_string())
}

/// Events a person does on purpose. A session with one of them at most is
/// a bounce; `$scroll`, `$vital`, `$identify` or `$error` alone are not
/// activity. The board's sessions use the same rule (`openlens-engine`
/// checks that its copy is this one).
pub const INTERACTION: &str =
    "(event IN ('$pageview', '$click', '$submit', '$feature', '$survey') OR event NOT LIKE '$%')";

/// The SQL of [`OCCURRENCES`], written once for both constants.
macro_rules! occurrences {
    () => {
        "CASE WHEN strpos(properties, '\"$repeat\":true') > 0 \
         THEN coalesce(CAST(regexp_match(properties, '\"\\$repeats\":([0-9]{1,7})[,}]')[1] AS BIGINT), 0) ELSE 1 END"
    };
}

/// The occurrences one `$error` row stands for (#703, D-097): 1, or for
/// the row the SDK sends with the repeats of an error it already sent in
/// the session (`"$repeat":true`), its `$repeats` (at most 7 digits; a row
/// without a readable count stands for none). Reads `properties` only on
/// those rows.
pub const OCCURRENCES: &str = occurrences!();

/// The errors an event counts for: [`OCCURRENCES`] of an `$error`, else 0.
/// Sum it wherever errors are counted, rather than counting `$error` rows.
pub const ERROR_COUNT: &str = concat!(
    "CASE WHEN event = '$error' THEN ",
    occurrences!(),
    " ELSE 0 END"
);

/// What one distribution-only event (`$vital`, `$scroll`, `$field`)
/// stands for (#702, D-097): `1 / $sample` when the SDK sent it from a
/// sample of sessions, else 1. Bounded to 1 – 100 000 so a hostile value
/// cannot blow a count up. Sum it wherever such events, or the sessions
/// they show, are counted as an absolute number; shares and percentiles
/// need no weight. Reads `properties` only on rows that carry `$sample`.
pub const SAMPLE_WEIGHT: &str = "CASE WHEN strpos(properties, '\"$sample\":') > 0 \
     THEN least(greatest(coalesce(1.0 / nullif(CAST(regexp_match(properties, '\"\\$sample\":(0\\.[0-9]{1,20})[,}]')[1] AS DOUBLE), 0.0), 1.0), 1.0), 100000.0) \
     ELSE 1.0 END";

/// `session_day` of one UTC day (#493): per session, person, environment,
/// release and role, from the `events` view (people already merged).
fn session_day_sql(day: &str, f: &DayFeatures) -> Result<String, QueryError> {
    let (task, version) = (task_sql(f)?, version_sql(f));
    Ok(format!(
        "SELECT session_id, person, environment, release, role, \
         min(timestamp) AS first_ts, max(timestamp) AS last_ts, \
         sum(CASE WHEN {INTERACTION} THEN 1 ELSE 0 END) AS interactions, \
         sum({ERROR_COUNT}) AS errors, \
         sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS tasks, \
         CAST({version} AS BIGINT) AS map_version \
         FROM (SELECT session_id, distinct_id AS person, environment, release, role, \
               {feature} AS feature, event, timestamp \
               FROM events WHERE date = '{day}' AND session_id IS NOT NULL) e \
         GROUP BY 1, 2, 3, 4, 5",
        feature = f.feature
    ))
}

/// `session_feature_day` of one UTC day (#483): per session, person,
/// environment, release, role and feature, from the `events` view (people
/// already merged), events with a feature and a session only.
fn session_feature_day_sql(day: &str, f: &DayFeatures) -> Result<String, QueryError> {
    let (task, version) = (task_sql(f)?, version_sql(f));
    Ok(format!(
        "SELECT session_id, person, environment, release, role, feature, \
         min(timestamp) AS first_ts, max(timestamp) AS last_ts, count(*) AS events, \
         sum(CASE WHEN {task} THEN 1 ELSE 0 END) AS successes, \
         CAST({version} AS BIGINT) AS map_version \
         FROM (SELECT session_id, distinct_id AS person, environment, release, role, \
               {feature} AS feature, event, timestamp \
               FROM events WHERE date = '{day}' AND session_id IS NOT NULL) e \
         WHERE feature IS NOT NULL \
         GROUP BY 1, 2, 3, 4, 5, 6",
        feature = f.feature
    ))
}

/// `success_day` of one UTC day (#452): per person, environment, release,
/// role and success event of the map, from the `events` view (people
/// already merged). Without success events it has no row but the day's
/// marker ([`day_marker`]).
fn success_day_sql(day: &str, f: &DayFeatures) -> Result<String, QueryError> {
    let (success, version) = (success_sql(f)?, version_sql(f));
    Ok(format!(
        "SELECT distinct_id AS person, environment, release, role, event, \
         count(*) AS events, min(timestamp) AS first_ts, \
         CAST({version} AS BIGINT) AS map_version \
         FROM events WHERE date = '{day}' AND {success} \
         GROUP BY 1, 2, 3, 4, 5",
        success = success.as_deref().unwrap_or("false")
    ))
}

/// The row that says a day's `success_day` was rolled up, with the map
/// `version` (D-062): every other column is null, so no reader counts it.
fn day_marker(schema: &SchemaRef, version: Option<i64>) -> Result<RecordBatch, QueryError> {
    let cols = schema
        .fields()
        .iter()
        .map(|f| {
            if f.name() == "map_version" {
                std::sync::Arc::new(arrow::array::Int64Array::from(vec![version]))
                    as arrow::array::ArrayRef
            } else {
                arrow::array::new_null_array(f.data_type(), 1)
            }
        })
        .collect();
    RecordBatch::try_new(schema.clone(), cols).map_err(|e| QueryError::Storage(e.to_string()))
}

/// `metric_q` of one UTC day.
fn metric_sql(day: &str) -> String {
    format!(
        "SELECT {bucket} AS bucket, environment, release, role, event, feature, \
         count(*) AS events, count(DISTINCT distinct_id) AS persons \
         FROM events WHERE date = '{day}' \
         GROUP BY 1, 2, 3, 4, 5, 6",
        bucket = bucket()
    )
}

/// The dimensions of the page rollups, in their schema order.
const DIMS: &str = "path, environment, release, role";

/// `"$name":<number>` of the properties, as a double.
fn num(name: &str) -> String {
    format!(r#"CAST(regexp_match(properties, '"\${name}":([0-9.]+)')[1] AS DOUBLE)"#)
}

const RAGE: &str = r#"CASE WHEN properties LIKE '%"$rage":true%' THEN 1 ELSE 0 END"#;

/// `click_q` of one UTC day (#324): the click map's cells.
fn click_sql(day: &str) -> String {
    format!(
        "SELECT {DIMS}, CAST(least(floor(x * 50), 49) AS BIGINT) AS cx, CAST(floor(y / 20) AS BIGINT) AS cy, \
         count(*) AS clicks, sum(rage) AS rages, sum(vw) AS vw_sum, count(vw) AS vw_n \
         FROM (SELECT {DIMS}, {x} AS x, {y} AS y, {vw} AS vw, {RAGE} AS rage \
               FROM events WHERE date = '{day}' AND event = '$click' AND path IS NOT NULL) c \
         WHERE x IS NOT NULL AND y IS NOT NULL AND x <= 1 AND y < 30000 \
         GROUP BY 1, 2, 3, 4, 5, 6",
        x = num("x"),
        y = num("y"),
        vw = num("vw"),
    )
}

/// `label_q` of one UTC day (#324): the most clicked labels per page.
fn label_sql(day: &str) -> String {
    format!(
        r#"SELECT {DIMS}, label, clicks, rages FROM (
             SELECT {DIMS}, label, count(*) AS clicks, sum(rage) AS rages,
                    row_number() OVER (PARTITION BY path ORDER BY count(*) DESC, label) AS rk
             FROM (SELECT {DIMS},
                          coalesce(feature, regexp_match(properties, '"\$text":"([^"]{{1,60}})"')[1],
                                   regexp_match(properties, '"\$tag":"([a-z0-9-]+)"')[1], '?') AS label,
                          {RAGE} AS rage
                   FROM events WHERE date = '{day}' AND event = '$click' AND path IS NOT NULL) c
             GROUP BY 1, 2, 3, 4, 5) t
           WHERE rk <= {top}"#,
        top = openlens_storage::rollup::TOP_LABELS
    )
}

/// `scroll_q` of one UTC day (#324): sessions by deepest scroll, 10 % steps,
/// scaled up when the SDK sent `$scroll` from a sample of sessions (#702).
fn scroll_sql(day: &str) -> String {
    format!(
        "SELECT {DIMS}, CAST(least(floor(d / 10), 10) AS BIGINT) AS depth, \
         CAST(round(sum(w)) AS BIGINT) AS sessions FROM ( \
           SELECT {DIMS}, session_id, max({depth}) AS d, max({SAMPLE_WEIGHT}) AS w FROM events \
           WHERE date = '{day}' AND event = '$scroll' AND path IS NOT NULL GROUP BY 1, 2, 3, 4, 5) s \
         WHERE d IS NOT NULL GROUP BY 1, 2, 3, 4, 5",
        depth = num("depth")
    )
}

fn bucket() -> String {
    format!(
        "date_bin(INTERVAL '{} minutes', timestamp, TIMESTAMP '1970-01-01T00:00:00Z')",
        openlens_storage::rollup::BUCKET_MINUTES
    )
}

/// `YYYY-MM-DD`, the only form a day may take in the SQL above.
fn valid_day(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

impl QueryEngine {
    /// The rollup tables named in `wanted` (all of them when it is
    /// `None`) of `project` for the UTC `day` (`YYYY-MM-DD`), as
    /// `(name, batch)` pairs in the schemas of `openlens_storage::rollup`.
    /// `features` resolves the features of `person_day` and
    /// `session_feature_day`, the tasks of those and `session_day`, and the
    /// success events of `success_day`.
    pub async fn rollup_day(
        &self,
        project: &str,
        day: &str,
        wanted: Option<&[&str]>,
        features: &DayFeatures,
    ) -> Result<Vec<(&'static str, RecordBatch)>, QueryError> {
        if !valid_day(day) {
            return Err(QueryError::Rejected(format!("not a day: {day}")));
        }
        let this = self.clone();
        let (project, day) = (project.to_owned(), day.to_owned());
        let wanted: Option<Vec<String>> =
            wanted.map(|w| w.iter().map(|n| (*n).to_owned()).collect());
        let features = features.clone();
        offload(async move {
            let timeout = this.limits.timeout;
            let work = this.rollup_here(&project, &day, wanted.as_deref(), &features);
            match tokio::time::timeout(timeout, work).await {
                Ok(r) => r,
                Err(_) => Err(this.timed_out(timeout)),
            }
        })
        .await
    }

    async fn rollup_here(
        &self,
        project: &str,
        day: &str,
        wanted: Option<&[String]>,
        features: &DayFeatures,
    ) -> Result<Vec<(&'static str, RecordBatch)>, QueryError> {
        let ctx = self.context(project).await?;
        let mut out = Vec::new();
        for (name, schema) in openlens_storage::rollup::tables() {
            if wanted.is_some_and(|w| !w.iter().any(|n| n == name)) {
                continue;
            }
            let sql = match name {
                "person_q" => person_sql(day),
                "person_day" => person_day_sql(day, features)?,
                "session_day" => session_day_sql(day, features)?,
                "session_feature_day" => session_feature_day_sql(day, features)?,
                "success_day" => success_day_sql(day, features)?,
                "metric_q" => metric_sql(day),
                "click_q" => click_sql(day),
                "label_q" => label_sql(day),
                _ => scroll_sql(day),
            };
            let batches = ctx
                .sql(&sql)
                .await
                .map_err(|e| QueryError::Storage(e.to_string()))?
                .collect()
                .await
                .map_err(|e| QueryError::Storage(e.to_string()))?;
            let mut batch = conform(&schema, &batches)?;
            if name == "success_day" {
                let marker = day_marker(&schema, features.map_version)?;
                batch = arrow::compute::concat_batches(&schema, &[batch, marker])
                    .map_err(|e| QueryError::Storage(e.to_string()))?;
            }
            out.push((name, batch));
        }
        Ok(out)
    }
}

/// One batch in exactly `schema` (same column order; types cast where
/// DataFusion picked another width or zone spelling).
fn conform(schema: &SchemaRef, batches: &[RecordBatch]) -> Result<RecordBatch, QueryError> {
    let err = |e: arrow::error::ArrowError| QueryError::Storage(e.to_string());
    let mut parts = Vec::with_capacity(batches.len());
    for b in batches {
        let cols = schema
            .fields()
            .iter()
            .zip(b.columns())
            .map(|(f, c)| arrow::compute::cast(c, f.data_type()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        parts.push(RecordBatch::try_new(schema.clone(), cols).map_err(err)?);
    }
    arrow::compute::concat_batches(schema, &parts).map_err(err)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_plain_days_reach_the_sql() {
        assert!(super::valid_day("2026-03-01"));
        for bad in ["2026-3-01", "2026-03-01'", "x026-03-01", "2026/03/01", ""] {
            assert!(!super::valid_day(bad), "{bad}");
        }
    }
}

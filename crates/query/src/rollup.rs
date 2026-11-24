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
         sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors, \
         min(timestamp) AS first_ts, max(timestamp) AS last_ts \
         FROM events WHERE date = '{day}' \
         GROUP BY 1, 2, 3, 4, 5",
        bucket = bucket()
    )
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

/// `scroll_q` of one UTC day (#324): sessions by deepest scroll, 10 % steps.
fn scroll_sql(day: &str) -> String {
    format!(
        "SELECT {DIMS}, CAST(least(floor(d / 10), 10) AS BIGINT) AS depth, count(*) AS sessions FROM ( \
           SELECT {DIMS}, session_id, max({depth}) AS d FROM events \
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
    /// The rollup tables of `project` for the UTC `day` (`YYYY-MM-DD`), as
    /// `(name, batch)` pairs in the schemas of `openlens_storage::rollup`.
    pub async fn rollup_day(
        &self,
        project: &str,
        day: &str,
    ) -> Result<Vec<(&'static str, RecordBatch)>, QueryError> {
        if !valid_day(day) {
            return Err(QueryError::Rejected(format!("not a day: {day}")));
        }
        let this = self.clone();
        let (project, day) = (project.to_owned(), day.to_owned());
        offload(async move {
            let timeout = this.limits.timeout;
            match tokio::time::timeout(timeout, this.rollup_here(&project, &day)).await {
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
    ) -> Result<Vec<(&'static str, RecordBatch)>, QueryError> {
        let ctx = self.context(project).await?;
        let mut out = Vec::new();
        for (name, schema) in openlens_storage::rollup::tables() {
            let sql = match name {
                "person_q" => person_sql(day),
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
            out.push((name, conform(&schema, &batches)?));
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

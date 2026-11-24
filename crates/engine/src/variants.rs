//! Variant readouts of experiments and flags. Moved out of lib.rs as is (#307).

use super::*;

/// One arm of a flag's experiment (#252).
#[derive(Debug, Clone, Serialize)]
pub struct VariantRow {
    /// Variant key.
    pub variant: String,
    /// Users who got it.
    pub users: i64,
    /// Share of them reaching a success event (any feature use without
    /// success events).
    pub value_rate: f64,
    /// Share of their sessions with an error.
    pub error_rate: f64,
    /// Users who reached value.
    pub reached: i64,
    /// Their sessions.
    pub sessions: i64,
    /// The arm the others are compared with (#279): `control` when there
    /// is one, else the flag's first variant.
    pub baseline: bool,
    /// Two-proportion p-value of the value rate against the baseline
    /// (None for the baseline or when either side is under 30).
    pub value_p: Option<f64>,
    /// Same for the error rate (per session).
    pub error_p: Option<f64>,
    /// The threshold the p-values are held to: 0.05 shared by the arms
    /// compared with the baseline (Bonferroni).
    pub alpha: f64,
}

impl VariantRow {
    /// The value rate differs from the baseline's, below [`Self::alpha`].
    #[must_use]
    pub fn value_significant(&self) -> bool {
        self.value_p.is_some_and(|p| p < self.alpha)
    }
    /// The error rate differs from the baseline's, below [`Self::alpha`].
    #[must_use]
    pub fn error_significant(&self) -> bool {
        self.error_p.is_some_and(|p| p < self.alpha)
    }
}

/// Compare every arm with the baseline (`control`, else the first of
/// `order`, else the first row): p-values, Bonferroni-corrected alpha.
pub fn compare_variants(rows: &mut [VariantRow], order: &[String]) {
    let base = rows
        .iter()
        .position(|r| r.variant == "control")
        .or_else(|| {
            order
                .iter()
                .find_map(|k| rows.iter().position(|r| &r.variant == k))
        })
        .or(if rows.is_empty() { None } else { Some(0) });
    let Some(b) = base else { return };
    let others = rows.len().saturating_sub(1).max(1) as f64;
    let alpha = stats::ALPHA / others;
    let (bv, bu, be, bs) = (
        rows[b].value_rate,
        rows[b].users,
        rows[b].error_rate,
        rows[b].sessions,
    );
    for (i, r) in rows.iter_mut().enumerate() {
        r.alpha = alpha;
        r.baseline = i == b;
        if i != b {
            r.value_p = stats::two_proportions(r.value_rate, r.users, bv, bu);
            r.error_p = stats::two_proportions(r.error_rate, r.sessions, be, bs);
        }
    }
}

/// The arms of a flag with variants, last 30 days, from `$variants`.
pub async fn variant_readout(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    flag: &str,
    order: &[String],
) -> Result<Vec<VariantRow>> {
    let q = &q.with_params(&f.params()?);
    if flag.is_empty()
        || !flag
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(EngineError::InvalidFilter("flag key"));
    }
    let src = source(map);
    let w = f.clause(30)?;
    let successes: Vec<String> = map
        .map(|m| {
            m.features
                .iter()
                .filter_map(|x| x.success.clone())
                .collect()
        })
        .unwrap_or_default();
    let ok = if successes.is_empty() {
        "e.feature IS NOT NULL".to_owned()
    } else {
        format!(
            "e.event IN ({})",
            successes
                .iter()
                .map(|s| lit(s))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        )
    };
    let sql = format!(
        "WITH v AS (SELECT distinct_id, min(variant) AS variant FROM \
           (SELECT distinct_id, regexp_match(properties, '\"\\$variants\":\"(?:[^\"]*,)?{flag}=([a-z0-9_-]+)')[1] AS variant FROM {src} WHERE {w}) \
           WHERE variant IS NOT NULL GROUP BY distinct_id), \
         e AS (SELECT distinct_id, session_id, event, feature FROM {src} WHERE {w}) \
         SELECT v.variant, count(DISTINCT v.distinct_id) AS users, \
                count(DISTINCT CASE WHEN {ok} THEN e.distinct_id END) AS reached, \
                count(DISTINCT e.session_id) AS sessions, \
                count(DISTINCT CASE WHEN e.event = '$error' THEN e.session_id END) AS err \
         FROM v JOIN e ON v.distinct_id = e.distinct_id GROUP BY v.variant ORDER BY v.variant"
    );
    let mut rows: Vec<VariantRow> = q
        .sql(project, &sql)
        .await?
        .rows
        .iter()
        .filter_map(|r| {
            let users = int(r, "users");
            let sessions = int(r, "sessions");
            Some(VariantRow {
                variant: text(r, "variant")?,
                users,
                value_rate: int(r, "reached") as f64 / users.max(1) as f64,
                error_rate: int(r, "err") as f64 / sessions.max(1) as f64,
                reached: int(r, "reached"),
                sessions,
                baseline: false,
                value_p: None,
                error_p: None,
                alpha: stats::ALPHA,
            })
        })
        .collect();
    compare_variants(&mut rows, order);
    Ok(rows)
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    fn row(variant: &str, users: i64, reached: i64, sessions: i64, err: i64) -> VariantRow {
        VariantRow {
            variant: variant.into(),
            users,
            value_rate: reached as f64 / users as f64,
            error_rate: err as f64 / sessions as f64,
            reached,
            sessions,
            baseline: false,
            value_p: None,
            error_p: None,
            alpha: stats::ALPHA,
        }
    }

    #[test]
    fn arms_are_compared_with_control_under_bonferroni() {
        let mut rows = vec![
            row("long", 400, 120, 800, 40),
            row("control", 400, 80, 800, 40),
            row("short", 20, 15, 30, 1),
        ];
        compare_variants(
            &mut rows,
            &["long".into(), "control".into(), "short".into()],
        );
        assert!(rows[1].baseline && !rows[0].baseline);
        assert!((rows[0].alpha - 0.025).abs() < 1e-12, "two arms share 0.05");
        assert!(rows[0].value_significant(), "30% vs 20% on 400 each");
        assert!(!rows[0].error_significant(), "same error rate");
        assert_eq!(rows[2].value_p, None, "under 30 users: too small");
        assert_eq!(rows[1].value_p, None);
        // No control: the flag's first variant.
        let mut rows = vec![row("a", 100, 10, 100, 1), row("b", 100, 10, 100, 1)];
        compare_variants(&mut rows, &["b".into(), "a".into()]);
        assert!(rows[1].baseline);
        assert!((rows[0].alpha - 0.05).abs() < 1e-12);
    }
}

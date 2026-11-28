//! What users say (#718, D-116): `$feedback` events, counted per rating
//! and per feature, and their latest texts for the people allowed to read
//! them (admins and the automation token). The text is personal data:
//! viewers get the counts only, the files drop it after 90 days
//! (`openlens_storage::trim_feedback`), and neither Ask nor the digest
//! reads it.

use openlens_event::feedback::{ABOUT, RATING, TEXT, TEXT_DAYS};
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, int, text};

/// Longest period read, in days: older feedback has no text left.
pub const MAX_DAYS: i64 = TEXT_DAYS;
/// Texts listed by default, newest first (a feature's page shows these).
pub const LATEST: usize = 20;
/// Most texts one call returns.
pub const MAX_LATEST: usize = 200;

/// One feedback, as an admin reads it.
#[derive(Debug, Clone, Serialize)]
pub struct FeedbackItem {
    /// When it was sent (UTC, `YYYY-MM-DDTHH:MM:SS`).
    pub at: String,
    /// The feature it is about, when it named one.
    pub feature: Option<String>,
    /// Its rating, 1 to 5, when it gave one.
    pub rating: Option<i64>,
    /// What the user wrote; empty for a rating alone.
    pub text: String,
    /// Who sent it.
    pub distinct_id: String,
}

/// Feedback over a period.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeedbackReport {
    /// Days read.
    pub days: i64,
    /// Environment read.
    pub environment: String,
    /// The feature asked for; none: every feedback.
    pub feature: Option<String>,
    /// Feedbacks in the period.
    pub total: i64,
    /// `(rating, feedbacks)` for each rating from 1 to 5.
    pub by_rating: Vec<(i64, i64)>,
    /// Feedbacks without a rating.
    pub unrated: i64,
    /// Mean rating of those with one.
    pub mean_rating: Option<f64>,
    /// `(feature key, feedbacks)`, the most first; feedbacks naming no
    /// feature are left out.
    pub by_feature: Vec<(String, i64)>,
    /// Whether `latest` carries the texts: only for admins and the
    /// automation token.
    pub texts: bool,
    /// The latest feedbacks, newest first (empty when `texts` is false).
    pub latest: Vec<FeedbackItem>,
}

/// The `$feedback` rows of `f`'s environment over `days`, with their
/// feature key and rating extracted, people of the team left out unless
/// `f` keeps them.
fn base(f: &Filters, days: i64) -> Result<String> {
    let w = f.clause(days)?;
    Ok(format!(
        "SELECT regexp_match(properties, '\"\\{ABOUT}\":\"([A-Za-z0-9_.-]+)\"')[1] AS about, \
                TRY_CAST(regexp_match(properties, '\"\\{RATING}\":([1-5])')[1] AS BIGINT) AS rating, \
                timestamp, distinct_id, properties \
         FROM events WHERE {w} AND event = '{}'",
        openlens_event::names::FEEDBACK
    ))
}

/// Feedback of the last `days` (1 to [`MAX_DAYS`]), for one feature or
/// all; the latest `latest` texts when `texts` (the caller may read them).
pub async fn feedback(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    feature: Option<&str>,
    days: i64,
    texts: bool,
    latest: usize,
) -> Result<FeedbackReport> {
    let mut params = f.params()?;
    let only = match feature {
        Some(k) => {
            params
                .set("feature", k)
                .map_err(|_| crate::EngineError::InvalidFilter("feature"))?;
            " WHERE about = $feature"
        }
        None => "",
    };
    let q = &q.with_params(&params);
    let base = base(f, days)?;
    let counts =
        format!("SELECT about, rating, count(*) AS n FROM ({base}) b{only} GROUP BY about, rating");
    let rows = q.sql(project, &counts).await?.rows;
    let mut out = FeedbackReport {
        days,
        environment: f.environment.clone(),
        feature: feature.map(str::to_owned),
        by_rating: (1..=5).map(|r| (r, 0)).collect(),
        texts,
        ..FeedbackReport::default()
    };
    let mut features: std::collections::BTreeMap<String, i64> = Default::default();
    for r in &rows {
        let n = int(r, "n");
        out.total += n;
        match r.get("rating").and_then(serde_json::Value::as_i64) {
            Some(v) => {
                if let Some(slot) = out.by_rating.iter_mut().find(|x| x.0 == v) {
                    slot.1 += n;
                }
            }
            None => out.unrated += n,
        }
        if let Some(k) = text(r, "about") {
            *features.entry(k).or_default() += n;
        }
    }
    let rated: i64 = out.by_rating.iter().map(|x| x.1).sum();
    if rated > 0 {
        let sum: i64 = out.by_rating.iter().map(|(r, n)| r * n).sum();
        out.mean_rating = Some(sum as f64 / rated as f64);
    }
    out.by_feature = features.into_iter().collect();
    out.by_feature
        .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if texts && out.total > 0 {
        let sql = format!(
            "SELECT CAST(timestamp AS VARCHAR) AS ts, about, rating, distinct_id, properties \
             FROM ({base}) b{only} ORDER BY timestamp DESC LIMIT {}",
            latest.clamp(1, MAX_LATEST)
        );
        out.latest = q
            .sql(project, &sql)
            .await?
            .rows
            .iter()
            .map(|r| FeedbackItem {
                at: text(r, "ts")
                    .map(|t| t.chars().take(19).collect())
                    .unwrap_or_default(),
                feature: text(r, "about"),
                rating: r.get("rating").and_then(serde_json::Value::as_i64),
                text: text(r, "properties")
                    .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
                    .and_then(|v| v.get(TEXT).and_then(|t| t.as_str()).map(str::to_owned))
                    .unwrap_or_default(),
                distinct_id: text(r, "distinct_id").unwrap_or_default(),
            })
            .collect();
    }
    Ok(out)
}

/// Feedbacks of the last 7 days per feature key, the most first: the
/// "N feedback this week" of the board's insight cards.
pub async fn feedback_week(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
) -> Result<Vec<(String, i64)>> {
    let q = &q.with_params(&f.params()?);
    let sql = format!(
        "SELECT about, count(*) AS n FROM ({}) b WHERE about IS NOT NULL GROUP BY about ORDER BY n DESC LIMIT 500",
        base(f, 7)?
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .iter()
        .filter_map(|r| Some((text(r, "about")?, int(r, "n"))))
        .collect())
}

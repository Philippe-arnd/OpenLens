//! Where users come from, judged by what they do next (#226): per channel,
//! source or campaign (first or last touch), the users, the share reaching
//! the value moment (a success event), coming back on another day, and
//! churned. Last touch ignores direct visits (a bookmark does not undo
//! the campaign that brought the user).
//!
//! Anonymous visitors who were never linked to a user are not users
//! (#708, D-101): they are counted as `visitors`, next to the users whose
//! signup (#707) falls in the window, `signed_up`. Every count is made in
//! SQL, so the engine's row cap never cuts a row.

use chrono::Utc;
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, NOT_SERVER, Result, any_person, before, int, lit, source, text};

/// One row of the acquisition table.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct AcquisitionRow {
    /// Channel, source or campaign.
    pub value: String,
    /// Identified users whose touch it was.
    pub users: i64,
    /// Anonymous visitors whose touch it was and who were never linked to
    /// a user (#708).
    pub visitors: i64,
    /// Of the users, those whose signup falls in the window (#707).
    pub signed_up: i64,
    /// `signed_up / (visitors + signed_up)`: a lower bound, since a
    /// signup on another device leaves no link (D-101).
    pub signup_rate: f64,
    /// Of the users, reached a success event.
    pub reached: i64,
    /// Of the users, active on two days or more.
    pub returned: i64,
    /// Users old enough to judge churn (first seen before the churn window).
    pub judged: i64,
    /// Of those, inactive for longer than the churn window now.
    pub churned: i64,
}

impl AcquisitionRow {
    /// Share reaching the value moment.
    #[must_use]
    pub fn value_rate(&self) -> f64 {
        self.reached as f64 / self.users.max(1) as f64
    }
}

/// What the rows are grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    /// The first landing with a channel.
    First,
    /// The latest landing that was not direct.
    Last,
}

/// Per-touch rows (last 90 days).
pub async fn acquisition(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    by: &str,
    touch: Touch,
) -> Result<Vec<AcquisitionRow>> {
    let q = &q.with_params(&f.params()?);
    let src = source(map);
    let now = Utc::now();
    let w = f.clause_at(90, now)?;
    let start = before(now, 90 * 86_400);
    // The landing's facts are typed columns (#482, D-039).
    let key = match by {
        "source" => "acq_utm_source",
        "campaign" => "acq_utm_campaign",
        "referrer" => "acq_ref_host",
        _ => "acq_channel",
    };
    // First touch: the first landing with a channel. Last touch: the
    // latest landing that was not direct, else a direct one.
    let order = match touch {
        Touch::First => "timestamp",
        Touch::Last => "CASE WHEN acq_channel = 'direct' THEN 1 ELSE 0 END, timestamp DESC",
    };
    let successes: Vec<String> = map
        .map(|m| {
            m.features
                .iter()
                .filter_map(|x| x.success.clone())
                .collect()
        })
        .unwrap_or_default();
    let ok = if successes.is_empty() {
        "feature IS NOT NULL".to_owned()
    } else {
        format!(
            "event IN ({})",
            successes
                .iter()
                .map(|s| lit(s))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        )
    };
    let churn = map.map_or(14, |m| i64::from(m.churn_days()));
    // A project that never identified anyone counts every visitor as a
    // user (docs/kpis.md).
    let known = if any_person(q, project).await? {
        "s.person IS NOT NULL"
    } else {
        "true"
    };
    let sql = format!(
        "WITH t AS (SELECT distinct_id, first_value({key} ORDER BY {order}) AS v FROM {src} \
               WHERE {w} AND event = '$pageview' AND acq_channel IS NOT NULL GROUP BY distinct_id), \
         u AS (SELECT distinct_id, count(DISTINCT date) AS days, max(CASE WHEN {ok} THEN 1 ELSE 0 END) AS ok, \
               min(timestamp) AS first_ts, max(timestamp) AS last_ts \
               FROM {src} WHERE {w} AND {NOT_SERVER} GROUP BY distinct_id), \
         s AS (SELECT person, COALESCE(signed_up, first_seen) AS signup FROM persons) \
         SELECT coalesce(t.v, '(none)') AS g, \
         sum(CASE WHEN {known} THEN 1 ELSE 0 END) AS users, \
         sum(CASE WHEN {known} THEN 0 ELSE 1 END) AS visitors, \
         sum(CASE WHEN {known} AND s.signup >= {start} THEN 1 ELSE 0 END) AS signed_up, \
         sum(CASE WHEN {known} THEN u.ok ELSE 0 END) AS reached, \
         sum(CASE WHEN {known} AND u.days >= 2 THEN 1 ELSE 0 END) AS returned, \
         sum(CASE WHEN {known} AND u.first_ts < now() - INTERVAL '{churn} days' THEN 1 ELSE 0 END) AS judged, \
         sum(CASE WHEN {known} AND u.first_ts < now() - INTERVAL '{churn} days' \
             AND u.last_ts < now() - INTERVAL '{churn} days' THEN 1 ELSE 0 END) AS churned \
         FROM u JOIN t ON t.distinct_id = u.distinct_id LEFT JOIN s ON s.person = u.distinct_id \
         GROUP BY 1"
    );
    let mut out: Vec<AcquisitionRow> = q
        .sql(project, &sql)
        .await?
        .rows
        .iter()
        .map(|r| {
            let (visitors, signed_up) = (int(r, "visitors"), int(r, "signed_up"));
            AcquisitionRow {
                value: text(r, "g").unwrap_or_else(|| "(none)".into()),
                users: int(r, "users"),
                visitors,
                signed_up,
                signup_rate: if visitors + signed_up > 0 {
                    signed_up as f64 / (visitors + signed_up) as f64
                } else {
                    0.0
                },
                reached: int(r, "reached"),
                returned: int(r, "returned"),
                judged: int(r, "judged"),
                churned: int(r, "churned"),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.value_rate()
            .total_cmp(&a.value_rate())
            .then(b.users.cmp(&a.users))
            .then(a.value.cmp(&b.value))
    });
    Ok(out)
}

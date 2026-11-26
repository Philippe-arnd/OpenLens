//! Where users come from, judged by what they do next (#226): per channel,
//! source or campaign (first or last touch), the users, the share reaching
//! the value moment (a success event), coming back on another day, and
//! churned. Last touch ignores direct visits (a bookmark does not undo
//! the campaign that brought the user).

use std::collections::HashMap;

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, int, lit, source, text};

/// One row of the acquisition table.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct AcquisitionRow {
    /// Channel, source or campaign.
    pub value: String,
    /// Users whose touch it was.
    pub users: i64,
    /// Of them, reached a success event.
    pub reached: i64,
    /// Of them, active on two days or more.
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

/// Per-user facts, then per-touch rows (last 90 days).
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
    let w = f.clause(90)?;
    // The landing's facts are typed columns (#482, D-039).
    let key = match by {
        "source" => "acq_utm_source",
        "campaign" => "acq_utm_campaign",
        "referrer" => "acq_ref_host",
        _ => "acq_channel",
    };
    let landings = format!(
        "SELECT distinct_id, acq_channel AS ch, {key} AS v FROM {src} WHERE {w} AND event = '$pageview' \
         AND acq_channel IS NOT NULL ORDER BY timestamp LIMIT 300000"
    );
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
    let users = format!(
        "SELECT distinct_id, count(DISTINCT date) AS days, max(CASE WHEN {ok} THEN 1 ELSE 0 END) AS ok, \
         CASE WHEN min(timestamp) < now() - INTERVAL '{churn} days' THEN 1 ELSE 0 END AS judged, \
         CASE WHEN max(timestamp) < now() - INTERVAL '{churn} days' THEN 1 ELSE 0 END AS gone \
         FROM {src} WHERE {w} GROUP BY distinct_id"
    );
    let (l, u) = futures::join!(q.sql(project, &landings), q.sql(project, &users));
    let mut touch_of: HashMap<String, String> = HashMap::new();
    for r in &l?.rows {
        let (Some(id), Some(ch)) = (text(r, "distinct_id"), text(r, "ch")) else {
            continue;
        };
        let v = text(r, "v").unwrap_or_else(|| "(none)".into());
        match touch {
            Touch::First => {
                touch_of.entry(id).or_insert(v);
            }
            Touch::Last => {
                if ch != "direct" || !touch_of.contains_key(&id) {
                    touch_of.insert(id, v);
                }
            }
        }
    }
    let mut rows: HashMap<String, AcquisitionRow> = HashMap::new();
    for r in &u?.rows {
        let Some(id) = text(r, "distinct_id") else {
            continue;
        };
        let Some(v) = touch_of.get(&id) else { continue };
        let row = rows.entry(v.clone()).or_insert_with(|| AcquisitionRow {
            value: v.clone(),
            ..AcquisitionRow::default()
        });
        row.users += 1;
        row.reached += int(r, "ok");
        row.returned += i64::from(int(r, "days") >= 2);
        row.judged += int(r, "judged");
        row.churned += int(r, "judged") * int(r, "gone");
    }
    let mut out: Vec<AcquisitionRow> = rows.into_values().collect();
    out.sort_by(|a, b| {
        b.value_rate()
            .total_cmp(&a.value_rate())
            .then(b.users.cmp(&a.users))
            .then(a.value.cmp(&b.value))
    });
    Ok(out)
}

//! Revenue on success events (#231). When a feature's success event
//! carries `revenue` (or `value`) and optionally `currency`, the board
//! shows revenue per feature, role and release, and per active user.
//! Other events' amounts are ignored. Without a rate table in the feature
//! map, each currency stays on its own (never summed).

use std::collections::BTreeMap;

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, any_person, identified, int, lit, source, text};

/// Revenue figures in one currency.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Money {
    /// ISO code (`EUR`), or `?` when events gave none and no display
    /// currency is set.
    pub currency: String,
    /// Total.
    pub total: f64,
    /// Success events with an amount.
    pub orders: i64,
    /// Users who paid.
    pub payers: i64,
    /// Total per active user of the window.
    pub per_active_user: f64,
}

/// Revenue of the last 30 days.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Revenue {
    /// Totals, one per currency (one only when a display currency is set).
    pub totals: Vec<Money>,
    /// (feature, currency, total), largest first.
    pub by_feature: Vec<(String, String, f64)>,
    /// (role, currency, total).
    pub by_role: Vec<(String, String, f64)>,
    /// (release, currency, total).
    pub by_release: Vec<(String, String, f64)>,
    /// Whether amounts were converted into the display currency.
    pub converted: bool,
}

/// An event's amount: its `revenue` property, else its `value` (#231).
/// The lifecycle's paid stage reads the same (#708).
pub(crate) const AMOUNT: &str = "coalesce(CAST(regexp_match(properties, '\"revenue\":(-?[0-9]+(\\.[0-9]+)?)')[1] AS DOUBLE), \
     CAST(regexp_match(properties, '\"value\":(-?[0-9]+(\\.[0-9]+)?)')[1] AS DOUBLE))";

/// An event's currency: its `currency` property, upper-cased (#231).
pub(crate) const CURRENCY: &str =
    "upper(regexp_match(properties, '\"currency\":\"([A-Za-z]{3})\"')[1])";

/// An amount in the currency it is shown in: the display currency when
/// the map sets one and it converts (same currency, or a rate), else its
/// own (`?` when the event gave none and no display currency is set).
pub(crate) fn shown(
    settings: Option<&openlens_featuremap::RevenueSettings>,
    cur: Option<String>,
    amt: f64,
) -> (String, f64) {
    match settings {
        Some(s) => {
            let c = cur.unwrap_or_else(|| s.currency.clone());
            if c == s.currency {
                (c, amt)
            } else if let Some(rate) = s.rates.get(&c) {
                (s.currency.clone(), amt * rate)
            } else {
                (c, amt)
            }
        }
        None => (cur.unwrap_or_else(|| "?".into()), amt),
    }
}

/// Revenue of a project, or None when no success event carries an amount.
pub async fn revenue(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
) -> Result<Option<Revenue>> {
    let q = &q.with_params(&f.params()?);
    let Some(m) = map else { return Ok(None) };
    let successes: Vec<(String, String)> = m
        .features
        .iter()
        .filter_map(|x| x.success.clone().map(|s| (x.key.clone(), s)))
        .collect();
    if successes.is_empty() {
        return Ok(None);
    }
    let src = source(map);
    let w = f.clause(30)?;
    let ident = identified(any_person(q, project).await?);
    let list = successes
        .iter()
        .map(|(_, s)| lit(s))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let amount = AMOUNT;
    let cur = CURRENCY;
    let sql = format!(
        "SELECT event, role, release, distinct_id, {cur} AS cur, {amount} AS amt FROM {src} \
         WHERE {w} AND event IN ({list}) AND {amount} IS NOT NULL LIMIT 500000"
    );
    let active_sql =
        format!("SELECT count(DISTINCT distinct_id) AS n FROM {src} WHERE {w} AND {ident}");
    let (rows, active) = futures::join!(q.sql(project, &sql), q.sql(project, &active_sql));
    let rows = rows?.rows;
    if rows.is_empty() {
        return Ok(None);
    }
    let active = active?.rows.first().map_or(0, |r| int(r, "n")).max(1);
    let settings = m.revenue.as_ref();
    let feature_of = |event: &str| -> String {
        successes
            .iter()
            .find(|(_, s)| s == event)
            .map_or_else(|| event.to_owned(), |(k, _)| k.clone())
    };
    let mut totals: BTreeMap<String, (f64, i64, std::collections::HashSet<String>)> =
        BTreeMap::new();
    let mut by_f: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut by_r: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut by_rel: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut converted = false;
    for r in &rows {
        let Some(amt) = r.get("amt").and_then(serde_json::Value::as_f64) else {
            continue;
        };
        if !amt.is_finite() {
            continue;
        }
        let raw_cur = text(r, "cur");
        let (c, v) = shown(settings, raw_cur.clone(), amt);
        if raw_cur.is_some_and(|rc| rc != c) {
            converted = true;
        }
        let t = totals.entry(c.clone()).or_default();
        t.0 += v;
        t.1 += 1;
        if let Some(id) = text(r, "distinct_id") {
            t.2.insert(id);
        }
        *by_f
            .entry((feature_of(&text(r, "event").unwrap_or_default()), c.clone()))
            .or_default() += v;
        *by_r
            .entry((
                text(r, "role").unwrap_or_else(|| "(none)".into()),
                c.clone(),
            ))
            .or_default() += v;
        *by_rel
            .entry((text(r, "release").unwrap_or_else(|| "(none)".into()), c))
            .or_default() += v;
    }
    let flat = |m: BTreeMap<(String, String), f64>| {
        let mut v: Vec<(String, String, f64)> =
            m.into_iter().map(|((a, b), x)| (a, b, x)).collect();
        v.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
        v.truncate(20);
        v
    };
    let mut t: Vec<Money> = totals
        .into_iter()
        .map(|(currency, (total, orders, payers))| Money {
            currency,
            total,
            orders,
            payers: payers.len() as i64,
            per_active_user: total / active as f64,
        })
        .collect();
    t.sort_by(|a, b| b.total.total_cmp(&a.total));
    Ok(Some(Revenue {
        totals: t,
        by_feature: flat(by_f),
        by_role: flat(by_r),
        by_release: flat(by_rel),
        converted,
    }))
}

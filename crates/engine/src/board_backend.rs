//! The board's server side (#623): what the route stats (#593, #594) say
//! over the board's window, against the same length just before.
//!
//! - **Key actions** (the status strip): each feature with `server:`
//!   routes in the map, its success rate (the share of its requests not
//!   answered 5xx) now and before, and whether it degraded by the
//!   verdict's own 5xx rule ([`crate::routes::checks`]).
//! - **5xx share** (the Quality tile): requests answered 5xx over every
//!   request of the window.
//!
//! One grouped query of `$request_stats` beside the other board blocks,
//! cached with the board. Route stats carry no user: only the environment
//! (and a release filter) applies.

use std::collections::HashMap;

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::routes::{self, RouteStat};
use crate::verdict::Outcome;
use crate::{Filters, Result, Span, interval, text};

/// One key action over the window.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct KeyAction {
    /// The feature's key.
    pub feature: String,
    /// Its name in the map.
    pub name: String,
    /// Requests to its routes in the window.
    pub requests: i64,
    /// Of those, answered 5xx.
    pub errors: i64,
    /// Requests the same length before.
    pub previous_requests: i64,
    /// Of those, answered 5xx.
    pub previous_errors: i64,
    /// Its 5xx share rose by the verdict's rule (enough requests on both
    /// sides, a significant rise of at least 2 points).
    pub degraded: bool,
}

impl KeyAction {
    /// Share of its requests not answered 5xx; none without a request.
    #[must_use]
    pub fn success_rate(&self) -> Option<f64> {
        (self.requests > 0).then(|| 1.0 - self.errors as f64 / self.requests as f64)
    }

    /// The same, the length before.
    #[must_use]
    pub fn previous_success_rate(&self) -> Option<f64> {
        (self.previous_requests > 0)
            .then(|| 1.0 - self.previous_errors as f64 / self.previous_requests as f64)
    }
}

/// The server side of the board's window.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ServerLine {
    /// Requests in the window, every route.
    pub requests: i64,
    /// Of those, answered 5xx.
    pub errors: i64,
    /// Requests the same length before.
    pub previous_requests: i64,
    /// Of those, answered 5xx.
    pub previous_errors: i64,
    /// Features with key actions in the map.
    pub declared: usize,
    /// Those with a request in the window or before: the degraded first,
    /// then the lowest success rate.
    pub key_actions: Vec<KeyAction>,
}

impl ServerLine {
    /// Share of the window's requests answered 5xx; none without a
    /// request (no server stats: nothing to say, never a zero).
    #[must_use]
    pub fn error_rate(&self) -> Option<f64> {
        (self.requests > 0).then(|| self.errors as f64 / self.requests as f64)
    }

    /// The key action to show: the worst, when one had a request in the
    /// window.
    #[must_use]
    pub fn worst(&self) -> Option<&KeyAction> {
        self.key_actions.iter().find(|k| k.requests > 0)
    }
}

/// The line from the routes of the window and of the length before.
#[must_use]
pub fn line(current: &[RouteStat], previous: &[RouteStat], map: Option<&FeatureMap>) -> ServerLine {
    let sum = |l: &[RouteStat]| {
        l.iter().fold((0i64, 0i64), |(r, e), s| {
            (r.saturating_add(s.requests), e.saturating_add(s.errors))
        })
    };
    let ((requests, errors), (previous_requests, previous_errors)) = (sum(current), sum(previous));
    let keys = routes::key_actions(map);
    let names: HashMap<&str, &str> = map
        .map(|m| m.features.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|f| (f.key.as_str(), f.name.as_str()))
        .collect();
    // Each feature's routes merged, on each side.
    let mut merged: HashMap<&str, (RouteStat, RouteStat)> = HashMap::new();
    for (side, list) in [(0, current), (1, previous)] {
        for s in list {
            let Some(feature) = keys.get(&s.name()) else {
                continue;
            };
            let e = merged.entry(feature.as_str()).or_insert_with(|| {
                let empty = RouteStat {
                    method: String::new(),
                    route: feature.clone(),
                    buckets: vec![0; routes::BUCKETS],
                    ..RouteStat::default()
                };
                (empty.clone(), empty)
            });
            if side == 0 { &mut e.0 } else { &mut e.1 }.merge(s);
        }
    }
    let mut key_actions: Vec<KeyAction> = merged
        .into_iter()
        .map(|(feature, (c, p))| KeyAction {
            feature: feature.to_owned(),
            name: names.get(feature).copied().unwrap_or(feature).to_owned(),
            requests: c.requests,
            errors: c.errors,
            previous_requests: p.requests,
            previous_errors: p.errors,
            degraded: routes::checks(std::slice::from_ref(&c), std::slice::from_ref(&p))
                .iter()
                .any(|x| x.key.starts_with("5xx:") && x.outcome == Outcome::Worse),
        })
        .collect();
    key_actions.sort_by(|a, b| {
        b.degraded
            .cmp(&a.degraded)
            .then(
                a.success_rate()
                    .unwrap_or(1.0)
                    .total_cmp(&b.success_rate().unwrap_or(1.0)),
            )
            .then(a.feature.cmp(&b.feature))
    });
    let mut declared: Vec<&String> = keys.values().collect();
    declared.sort();
    declared.dedup();
    ServerLine {
        requests,
        errors,
        previous_requests,
        previous_errors,
        declared: declared.len(),
        key_actions,
    }
}

/// The routes of `f`'s environment (and release) over `current` and over
/// `previous` (just before it, the same length), on `q` with `f`'s
/// parameters bound: one grouped query, [`routes::MAX_ROUTES`] routes a
/// side at most.
///
/// # Errors
/// A failed query.
pub(crate) async fn read(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    (current, previous): (Span, Span),
    map: Option<&FeatureMap>,
) -> Result<ServerLine> {
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::seconds(previous.start)).format("%Y-%m-%d");
    let mut w = format!(
        "date >= '{since}' AND timestamp >= now() - INTERVAL '{}' AND environment = $env",
        interval(previous.start)
    );
    if current.end > 0 {
        w.push_str(&format!(
            " AND timestamp < now() - INTERVAL '{}'",
            interval(current.end)
        ));
    }
    if f.release.is_some() {
        w.push_str(" AND release = $release");
    }
    let part = format!(
        "CASE WHEN timestamp >= now() - INTERVAL '{}' THEN 'now' ELSE 'before' END",
        interval(current.start)
    );
    let keys = routes::key_actions(map);
    let r = q
        .sql(
            project,
            &routes::stats_sql(&w, Some(&part), routes::MAX_ROUTES * 2),
        )
        .await?;
    let (mut now_l, mut before) = (Vec::new(), Vec::new());
    for x in &r.rows {
        let Some(s) = routes::row_stat(x, &keys) else {
            continue;
        };
        if text(x, "part").as_deref() == Some("now") {
            now_l.push(s);
        } else {
            before.push(s);
        }
    }
    Ok(line(&now_l, &before, map))
}

/// The spans of the board's server line: the window and the one before,
/// or the last 7 days and the 7 before for the fixed windows (the Quality
/// tile's own).
pub(crate) fn spans(window: Option<(Span, Span)>) -> (Span, Span) {
    window.unwrap_or((
        Span::days(7),
        Span {
            start: 14 * 86_400,
            end: 7 * 86_400,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(route: &str, requests: i64, errors: i64) -> RouteStat {
        let mut buckets = vec![0; routes::BUCKETS];
        if let Some(b) = buckets.get_mut(3) {
            *b = requests;
        }
        RouteStat {
            method: "POST".into(),
            route: route.into(),
            requests,
            errors,
            max_ms: 40,
            buckets,
            ..RouteStat::default()
        }
    }

    fn map() -> FeatureMap {
        openlens_featuremap::parse(
            "features:\n  - key: sign-in\n    name: Sign-in\n    server: ['POST /api/auth/sign-in']\n  - key: export\n    name: Export\n    server: ['POST /api/export']\n",
        )
        .map
        .unwrap()
    }

    #[test]
    fn a_key_action_failing_more_than_before_is_degraded_and_comes_first() {
        let m = map();
        let now = [
            stat("/api/auth/sign-in", 400, 120),
            stat("/api/export", 300, 0),
            stat("/api/other", 1_000, 10),
        ];
        let before = [
            stat("/api/auth/sign-in", 400, 1),
            stat("/api/export", 300, 0),
            stat("/api/other", 1_000, 10),
        ];
        let l = line(&now, &before, Some(&m));
        assert_eq!(l.declared, 2);
        let worst = l.worst().unwrap();
        assert_eq!(worst.name, "Sign-in");
        assert!(worst.degraded);
        assert!(worst.success_rate() < worst.previous_success_rate());
        // Export did not move: listed, not degraded.
        assert!(
            l.key_actions
                .iter()
                .any(|k| k.feature == "export" && !k.degraded)
        );
        // The 5xx share counts every route.
        let rate = l.error_rate().unwrap();
        assert!(rate > 10.0 / 1_000.0 && rate < 120.0 / 400.0, "{rate}");
    }

    #[test]
    fn the_same_share_is_fine_and_no_stats_say_nothing() {
        let m = map();
        let same = [stat("/api/auth/sign-in", 400, 2)];
        let l = line(&same, &same, Some(&m));
        assert!(l.key_actions.iter().all(|k| !k.degraded));
        // No route stats: no rate, no key action to show, never zeros.
        let none = line(&[], &[], Some(&m));
        assert!(none.error_rate().is_none() && none.worst().is_none());
        assert_eq!(none.declared, 2, "still declared");
        // No map: nothing declared.
        assert_eq!(line(&same, &same, None).declared, 0);
    }
}

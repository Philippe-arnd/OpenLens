//! Relations between features, inferred from sessions (#283): how much
//! more often two features are used together than chance (lift), which
//! one comes first, and whether using the first raises the success of the
//! second. They honour the board filters, so a role or a release gives
//! the relations of that role or release.

use std::collections::{BTreeMap, HashMap, HashSet};

use openlens_featuremap::FeatureMap;
use serde::Serialize;

use crate::mapsessions::Sessions;

/// Fewest sessions using both features before a relation is shown.
pub const MIN_TOGETHER: i64 = 20;
/// Relations kept (the most sessions together first).
const MAX_RELATIONS: usize = 50;
/// Features of one session considered (the first used).
const MAX_PER_SESSION: usize = 20;

/// Two features seen in the same sessions.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Relation {
    /// The feature used first in most of their shared sessions.
    pub first: String,
    /// The other one.
    pub then: String,
    /// Sessions that used `first`.
    pub sessions_first: i64,
    /// Sessions that used `then`.
    pub sessions_then: i64,
    /// Sessions that used both.
    pub together: i64,
    /// How many times more often they are used together than if they were
    /// unrelated (1 = no relation).
    pub lift: f64,
    /// Of the shared sessions, the share where `first` came first.
    pub first_share: f64,
    /// Success rate of `then` in sessions where `first` came before it
    /// (None when `then` declares no success event).
    pub success_after: Option<f64>,
    /// Success rate of `then` in sessions that used it without `first`.
    pub success_without: Option<f64>,
    /// Using `first` raises the success of `then`, and the difference is
    /// significant (two-proportion test, 30 sessions on each side at least).
    pub leads_to_value: bool,
}

/// Infer the relations between the map's features from `sessions`.
#[must_use]
pub fn infer(m: &FeatureMap, sessions: &Sessions) -> Vec<Relation> {
    let success: HashMap<&str, &str> = m
        .features
        .iter()
        .filter_map(|f| Some((f.key.as_str(), f.success.as_deref()?)))
        .collect();
    let declared: HashSet<&str> = m.features.iter().map(|f| f.key.as_str()).collect();
    let total = sessions.len() as f64;
    let mut alone: HashMap<&str, i64> = HashMap::new();
    // (x, y) in key order → (sessions with both, of them `x` first).
    let mut pairs: BTreeMap<(&str, &str), (i64, i64)> = BTreeMap::new();
    for s in sessions.values() {
        let mut used: Vec<(&str, f64)> = s
            .features
            .iter()
            .filter(|(k, _)| declared.contains(k.as_str()))
            .map(|(k, v)| (k.as_str(), v.first))
            .collect();
        used.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(b.0)));
        used.truncate(MAX_PER_SESSION);
        for (k, _) in &used {
            *alone.entry(k).or_default() += 1;
        }
        // `used` is in time order: `a` comes before `b`.
        for (i, (a, _)) in used.iter().enumerate() {
            for (b, _) in &used[i + 1..] {
                let (key, a_is_x) = if a < b {
                    ((*a, *b), true)
                } else {
                    ((*b, *a), false)
                };
                let e = pairs.entry(key).or_default();
                e.0 += 1;
                e.1 += i64::from(a_is_x);
            }
        }
    }
    let mut out: Vec<Relation> = pairs
        .into_iter()
        .filter(|(_, (both, _))| *both >= MIN_TOGETHER)
        .map(|((x, y), (both, x_first))| {
            let (nx, ny) = (
                alone.get(x).copied().unwrap_or(0),
                alone.get(y).copied().unwrap_or(0),
            );
            // Orient: `first` is the one that comes first in most shared sessions.
            let (first, then, nf, nt, first_n) = if x_first * 2 >= both {
                (x, y, nx, ny, x_first)
            } else {
                (y, x, ny, nx, both - x_first)
            };
            Relation {
                first: first.to_owned(),
                then: then.to_owned(),
                sessions_first: nf,
                sessions_then: nt,
                together: both,
                lift: if nx > 0 && ny > 0 {
                    both as f64 * total / (nx as f64 * ny as f64)
                } else {
                    0.0
                },
                first_share: first_n as f64 / both as f64,
                ..Relation::default()
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.together
            .cmp(&a.together)
            .then(a.first.cmp(&b.first))
            .then(a.then.cmp(&b.then))
    });
    out.truncate(MAX_RELATIONS);
    for r in &mut out {
        if let Some(ev) = success.get(r.then.as_str()) {
            value(r, sessions, ev);
        }
    }
    out
}

/// Leads-to-value of `r`: the success of `then` (event `ev`) in sessions
/// where `first` came before it, against sessions without `first`.
fn value(r: &mut Relation, sessions: &Sessions, ev: &str) {
    let (mut after, mut after_ok, mut without, mut without_ok) = (0i64, 0i64, 0i64, 0i64);
    for s in sessions.values() {
        let Some(t) = s.features.get(&r.then) else {
            continue;
        };
        let ok = i64::from(s.successes.contains(ev));
        match s.features.get(&r.first) {
            Some(f) if f.first <= t.first => {
                after += 1;
                after_ok += ok;
            }
            Some(_) => {}
            None => {
                without += 1;
                without_ok += ok;
            }
        }
    }
    let rate = |ok: i64, n: i64| (n > 0).then(|| ok as f64 / n as f64);
    r.success_after = rate(after_ok, after);
    r.success_without = rate(without_ok, without);
    if let (Some(a), Some(w)) = (r.success_after, r.success_without) {
        r.leads_to_value = a > w
            && crate::stats::two_proportions(a, after, w, without)
                .is_some_and(|p| p < crate::stats::ALPHA);
    }
}

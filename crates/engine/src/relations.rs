//! Relations between features, inferred from sessions (#283): how much
//! more often two features are used together than chance (lift), which
//! one comes first, and whether using the first raises the success of the
//! second. They honour the board filters, so a role or a release gives
//! the relations of that role or release.
//!
//! They are computed from [`Counts`]: sessions per feature and per pair of
//! features, which SQL adds up over the session rollup and the events
//! (`mapdays`, #483), or which [`counts`] takes from loaded sessions.

use std::collections::{BTreeMap, HashMap, HashSet};

use openlens_featuremap::FeatureMap;
use serde::Serialize;

use crate::mapsessions::Sessions;

/// Fewest sessions using both features before a relation is shown.
pub const MIN_TOGETHER: i64 = 20;
/// Relations kept (the most sessions together first).
const MAX_RELATIONS: usize = 50;
/// Features of one session considered (the first used).
pub const MAX_PER_SESSION: usize = 20;

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

/// The sessions of two features `x` < `y` (key order) used together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pair {
    /// Sessions that used both.
    pub both: i64,
    /// Of them, sessions where `x` was first used before `y`, or at the
    /// same time.
    pub x_first: i64,
    /// Of them, sessions where `y` was first used before `x`, or at the
    /// same time.
    pub y_first: i64,
    /// Of `x_first`, sessions where `y` succeeded.
    pub y_ok_after: i64,
    /// Of `y_first`, sessions where `x` succeeded.
    pub x_ok_after: i64,
    /// Of `both`, sessions where `x` succeeded.
    pub x_ok: i64,
    /// Of `both`, sessions where `y` succeeded.
    pub y_ok: i64,
}

/// What the relations are inferred from: of each session, its first
/// [`MAX_PER_SESSION`] declared features, when each was first used and
/// whether it succeeded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counts {
    /// Sessions that used a feature.
    pub sessions: i64,
    /// Declared feature → (sessions that used it, of them sessions where
    /// it succeeded).
    pub features: BTreeMap<String, (i64, i64)>,
    /// `(x, y)` in key order → their shared sessions.
    pub pairs: BTreeMap<(String, String), Pair>,
}

/// The [`Counts`] of loaded `sessions`: a feature succeeded in a session
/// when its success event fired there.
#[must_use]
pub fn counts(m: &FeatureMap, sessions: &Sessions) -> Counts {
    let success: HashMap<&str, &str> = m
        .features
        .iter()
        .filter_map(|f| Some((f.key.as_str(), f.success.as_deref()?)))
        .collect();
    let declared: HashSet<&str> = m.features.iter().map(|f| f.key.as_str()).collect();
    let mut c = Counts {
        sessions: sessions.len() as i64,
        ..Counts::default()
    };
    for s in sessions.values() {
        let mut used: Vec<(&str, f64, i64)> = s
            .features
            .iter()
            .filter(|(k, _)| declared.contains(k.as_str()))
            .map(|(k, v)| {
                let ok = success
                    .get(k.as_str())
                    .is_some_and(|e| s.successes.contains(*e));
                (k.as_str(), v.first, i64::from(ok))
            })
            .collect();
        used.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(b.0)));
        used.truncate(MAX_PER_SESSION);
        for (k, _, ok) in &used {
            let e = c.features.entry((*k).to_owned()).or_default();
            e.0 += 1;
            e.1 += ok;
        }
        for (i, a) in used.iter().enumerate() {
            for b in &used[i + 1..] {
                let (x, y) = if a.0 < b.0 { (a, b) } else { (b, a) };
                let p = c
                    .pairs
                    .entry((x.0.to_owned(), y.0.to_owned()))
                    .or_default();
                let (xf, yf) = (i64::from(x.1 <= y.1), i64::from(y.1 <= x.1));
                p.both += 1;
                p.x_first += xf;
                p.y_first += yf;
                p.y_ok_after += xf * y.2;
                p.x_ok_after += yf * x.2;
                p.x_ok += x.2;
                p.y_ok += y.2;
            }
        }
    }
    c
}

/// Infer the relations between the map's features from `sessions`.
#[must_use]
pub fn infer(m: &FeatureMap, sessions: &Sessions) -> Vec<Relation> {
    from_counts(m, &counts(m, sessions))
}

/// Infer the relations between the map's features from `c`.
#[must_use]
pub fn from_counts(m: &FeatureMap, c: &Counts) -> Vec<Relation> {
    let success: HashSet<&str> = m
        .features
        .iter()
        .filter(|f| f.success.is_some())
        .map(|f| f.key.as_str())
        .collect();
    let of = |k: &str| c.features.get(k).copied().unwrap_or((0, 0));
    let total = c.sessions as f64;
    let mut out: Vec<Relation> = c
        .pairs
        .iter()
        .filter(|(_, p)| p.both >= MIN_TOGETHER)
        .map(|((x, y), p)| {
            let ((nx, x_ok), (ny, y_ok)) = (of(x.as_str()), of(y.as_str()));
            // Orient: `first` is the one that comes first in most shared
            // sessions. Then `then`'s success after `first`, and without it.
            let (first, then, nf, nt, first_n, after, after_ok, without, without_ok) =
                if p.x_first * 2 >= p.both {
                    let w = (ny - p.both, y_ok - p.y_ok);
                    (x, y, nx, ny, p.x_first, p.x_first, p.y_ok_after, w.0, w.1)
                } else {
                    let w = (nx - p.both, x_ok - p.x_ok);
                    let first_n = p.both - p.x_first;
                    (y, x, ny, nx, first_n, p.y_first, p.x_ok_after, w.0, w.1)
                };
            let mut r = Relation {
                first: first.clone(),
                then: then.clone(),
                sessions_first: nf,
                sessions_then: nt,
                together: p.both,
                lift: if nx > 0 && ny > 0 {
                    p.both as f64 * total / (nx as f64 * ny as f64)
                } else {
                    0.0
                },
                first_share: first_n as f64 / p.both as f64,
                ..Relation::default()
            };
            if success.contains(then.as_str()) {
                value(&mut r, (after, after_ok), (without, without_ok));
            }
            r
        })
        .collect();
    out.sort_by(|a, b| {
        b.together
            .cmp(&a.together)
            .then(a.first.cmp(&b.first))
            .then(a.then.cmp(&b.then))
    });
    out.truncate(MAX_RELATIONS);
    out
}

/// Leads-to-value of `r`: the success of `then` in the sessions where
/// `first` came before it (`after`: sessions, successes), against the
/// sessions that used it without `first` (`without`).
fn value(r: &mut Relation, after: (i64, i64), without: (i64, i64)) {
    let rate = |(n, ok): (i64, i64)| (n > 0).then(|| ok as f64 / n as f64);
    r.success_after = rate(after);
    r.success_without = rate(without);
    if let (Some(a), Some(w)) = (r.success_after, r.success_without) {
        r.leads_to_value = a > w
            && crate::stats::two_proportions(a, after.0, w, without.0)
                .is_some_and(|p| p < crate::stats::ALPHA);
    }
}

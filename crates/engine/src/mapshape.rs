//! Is each feature cut the right size, and are its steps in the right
//! order (#283)? Three map health findings from what sessions did:
//!
//! - `too_big`: the feature's pages and events fall into two or more
//!   groups that sessions do not mix (pages used together in fewer than
//!   10% of the smaller one's sessions), each group used by at least 25%
//!   of the feature's sessions and 20 users. Pages used by more than 80%
//!   of its sessions (a shared entry page) are left out of the grouping.
//!   Suggests one feature per group.
//! - `too_small`: two features used together in 90% or more of the
//!   sessions that use either, each with 20 users. Suggests a merge.
//! - `step_order`: two consecutive declared `steps` happen in the other
//!   order in most of the 20 or more sessions that do both. Suggests the
//!   observed order.
//!
//! These are suggestions: nothing changes the map until someone pushes it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use openlens_featuremap::{Feature, FeatureMap};

use crate::maphealth::{Finding, key_for};
use crate::mapsessions::Sessions;
use crate::relations::Relation;

/// Fewest users (and sessions) before a shape is judged.
const MIN_USERS: usize = 20;
/// A group must hold this share of the feature's sessions.
const MIN_GROUP_SHARE: f64 = 0.25;
/// Units used by more than this share of the sessions are shared entry points.
const SHARED_SHARE: f64 = 0.8;
/// Two units belong together when this share of the smaller one's
/// sessions use both.
const LINK_SHARE: f64 = 0.1;
/// Co-use (both / either) from which two features are one.
const MERGE_SHARE: f64 = 0.9;
/// Units of one feature grouped at most (the most used).
const MAX_UNITS: usize = 20;

/// The split, merge and step order findings of `m` on `sessions`.
#[must_use]
pub fn findings(m: &FeatureMap, sessions: &Sessions, relations: &[Relation]) -> Vec<Finding> {
    let mut out = Vec::new();
    for f in &m.features {
        if let Some(x) = too_big(f, sessions) {
            out.push(x);
        }
        out.extend(step_order(f, sessions));
    }
    out.extend(too_small(m, sessions, relations));
    out
}

fn users_of(sessions: &Sessions, key: &str) -> usize {
    sessions
        .values()
        .filter(|s| s.features.contains_key(key))
        .map(|s| s.user.as_str())
        .collect::<HashSet<_>>()
        .len()
}

fn too_big(f: &Feature, sessions: &Sessions) -> Option<Finding> {
    // Unit → sessions (indexes) that used it with this feature.
    let mut by_unit: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    let mut users: Vec<&str> = Vec::new();
    for s in sessions.values() {
        let Some(v) = s.features.get(&f.key) else {
            continue;
        };
        let i = users.len();
        users.push(s.user.as_str());
        for u in v.units.keys() {
            by_unit.entry(u.as_str()).or_default().insert(i);
        }
    }
    let n = users.len();
    if n < MIN_USERS || users.iter().collect::<HashSet<_>>().len() < MIN_USERS {
        return None;
    }
    let mut units: Vec<(&str, BTreeSet<usize>)> = by_unit
        .into_iter()
        .filter(|(_, s)| (s.len() as f64) <= SHARED_SHARE * n as f64)
        .collect();
    units.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    units.truncate(MAX_UNITS);
    // Union-find over the units.
    let mut parent: Vec<usize> = (0..units.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..units.len() {
        for j in i + 1..units.len() {
            let both = units[i].1.intersection(&units[j].1).count();
            let small = units[i].1.len().min(units[j].1.len());
            if small > 0 && both as f64 >= LINK_SHARE * small as f64 {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: BTreeMap<usize, (Vec<&str>, BTreeSet<usize>)> = BTreeMap::new();
    for (i, (u, s)) in units.iter().enumerate() {
        let g = groups.entry(root(&mut parent, i)).or_default();
        g.0.push(u);
        g.1.extend(s.iter().copied());
    }
    let mut big: Vec<(Vec<&str>, BTreeSet<usize>)> = groups
        .into_values()
        .filter(|(_, s)| {
            s.len() as f64 >= MIN_GROUP_SHARE * n as f64
                && s.iter().map(|i| users[*i]).collect::<HashSet<_>>().len() >= MIN_USERS
        })
        .collect();
    if big.len() < 2 {
        return None;
    }
    big.sort_by_key(|a| std::cmp::Reverse(a.1.len()));
    let mut fix = format!(
        "# Users of `{}` follow {} separate paths: split it (keep `{}` on the largest, renamed_from keeps its history)\n",
        f.key,
        big.len(),
        f.key
    );
    let mut evidence = Vec::new();
    let mut taken = HashSet::new();
    for (i, (us, s)) in big.iter().enumerate() {
        let routes: Vec<&str> = us.iter().copied().filter(|u| u.starts_with('/')).collect();
        let steps: Vec<&str> = us.iter().copied().filter(|u| !u.starts_with('/')).collect();
        let mut key = if i == 0 {
            f.key.clone()
        } else {
            format!(
                "{}-{}",
                f.key,
                key_for(us.first().copied().unwrap_or("part"))
            )
        };
        while !taken.insert(key.clone()) {
            key.push_str("-2");
        }
        evidence.push(format!(
            "{} of {n} sessions use {}",
            s.len(),
            us.iter().take(3).copied().collect::<Vec<_>>().join(", ")
        ));
        fix.push_str(&format!(
            "- key: {key}\n  name: {}\n",
            key.replace('-', " ")
        ));
        if !routes.is_empty() {
            fix.push_str(&format!("  entry:\n    routes: {routes:?}\n"));
        }
        if !steps.is_empty() {
            fix.push_str(&format!("  steps: {steps:?}\n"));
        }
    }
    Some(Finding {
        kind: "too_big",
        level: "warning",
        feature: Some(f.key.clone()),
        message: format!(
            "`{}` looks like {} features: its users split into separate paths that sessions do not mix",
            f.key,
            big.len()
        ),
        evidence: format!("{} (30 days)", evidence.join("; ")),
        fix: Some(fix),
        impact: n as i64,
    })
}

fn step_order(f: &Feature, sessions: &Sessions) -> Vec<Finding> {
    let mut out = Vec::new();
    for w in f.steps.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let (mut in_order, mut reversed) = (0usize, 0usize);
        for s in sessions.values() {
            let Some(v) = s.features.get(&f.key) else {
                continue;
            };
            if let (Some(ta), Some(tb)) = (v.units.get(a), v.units.get(b)) {
                if tb < ta {
                    reversed += 1;
                } else {
                    in_order += 1;
                }
            }
        }
        if in_order + reversed < MIN_USERS || reversed <= in_order {
            continue;
        }
        let mut steps = f.steps.clone();
        if let (Some(i), Some(j)) = (
            steps.iter().position(|x| x == a),
            steps.iter().position(|x| x == b),
        ) {
            steps.swap(i, j);
        }
        out.push(Finding {
            kind: "step_order",
            level: "warning",
            feature: Some(f.key.clone()),
            message: format!(
                "in `{}`, `{b}` usually happens before `{a}`, not after",
                f.key
            ),
            evidence: format!(
                "{reversed} of {} sessions that did both did `{b}` first (30 days)",
                in_order + reversed
            ),
            fix: Some(format!(
                "# The observed order (or check the app if `{a}` should come first):\n- key: {}\n  steps: {steps:?}",
                f.key
            )),
            impact: reversed as i64,
        });
    }
    out
}

fn too_small(m: &FeatureMap, sessions: &Sessions, relations: &[Relation]) -> Vec<Finding> {
    let defs: HashMap<&str, &Feature> = m.features.iter().map(|f| (f.key.as_str(), f)).collect();
    let mut out = Vec::new();
    for r in relations {
        let either = r.sessions_first + r.sessions_then - r.together;
        if either <= 0 || (r.together as f64) < MERGE_SHARE * either as f64 {
            continue;
        }
        let (Some(a), Some(b)) = (defs.get(r.first.as_str()), defs.get(r.then.as_str())) else {
            continue;
        };
        if users_of(sessions, &a.key) < MIN_USERS || users_of(sessions, &b.key) < MIN_USERS {
            continue;
        }
        let routes: Vec<&str> = a
            .entry
            .routes
            .iter()
            .chain(&b.entry.routes)
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut renamed: Vec<&str> = a.renamed_from.iter().map(String::as_str).collect();
        renamed.push(b.key.as_str());
        out.push(Finding {
            kind: "too_small",
            level: "warning",
            feature: Some(b.key.clone()),
            message: format!(
                "`{}` and `{}` are almost always used together: they are probably one feature",
                a.key, b.key
            ),
            evidence: format!(
                "{} of the {either} sessions that use either use both (30 days)",
                r.together
            ),
            fix: Some(format!(
                "# Merge `{}` into `{}` and delete the `{}` entry:\n- key: {}\n  entry:\n    routes: {routes:?}\n  renamed_from: {renamed:?}",
                b.key, a.key, b.key, a.key
            )),
            impact: r.together,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::mapsessions::{Session, Visit};

    fn visit(t: f64, units: &[(&str, f64)]) -> Visit {
        Visit {
            first: t,
            units: units.iter().map(|(u, t)| ((*u).to_owned(), *t)).collect(),
        }
    }

    fn map() -> FeatureMap {
        openlens_featuremap::parse(
            "features:
  - key: billing
    name: Billing
    entry: {routes: ['/billing/*']}
  - key: cart
    name: Cart
    steps: [cart_opened, item_added]
    entry: {routes: ['/cart']}
  - key: checkout
    name: Checkout
    entry: {routes: ['/checkout']}
",
        )
        .map
        .unwrap()
    }

    /// 60 sessions: billing splits into invoices vs cards behind a shared
    /// landing page; cart and checkout always go together; cart's steps
    /// happen in the other order.
    fn sessions() -> Sessions {
        let mut out = Sessions::new();
        for i in 0..60 {
            let mut s = Session {
                user: format!("u{i}"),
                ..Session::default()
            };
            let side = if i % 2 == 0 {
                "/billing/invoices"
            } else {
                "/billing/cards"
            };
            s.features.insert(
                "billing".into(),
                visit(1.0, &[("/billing", 1.0), (side, 2.0)]),
            );
            s.features.insert(
                "cart".into(),
                visit(3.0, &[("item_added", 3.0), ("cart_opened", 4.0)]),
            );
            s.features.insert("checkout".into(), visit(5.0, &[]));
            out.insert(format!("s{i}"), s);
        }
        out
    }

    #[test]
    fn planted_shapes_are_found_with_fixes() {
        let m = map();
        let s = sessions();
        let rel = crate::relations::infer(&m, &s);
        let f = findings(&m, &s, &rel);
        let kinds: Vec<(&str, Option<&str>)> =
            f.iter().map(|x| (x.kind, x.feature.as_deref())).collect();
        assert!(kinds.contains(&("too_big", Some("billing"))), "{kinds:?}");
        assert!(kinds.contains(&("step_order", Some("cart"))), "{kinds:?}");
        assert!(
            kinds.contains(&("too_small", Some("checkout"))),
            "{kinds:?}"
        );
        let big = f.iter().find(|x| x.kind == "too_big").unwrap();
        let fix = big.fix.as_deref().unwrap();
        assert!(
            fix.contains("/billing/invoices") && fix.contains("/billing/cards"),
            "{fix}"
        );
        // The shared landing page is not a reason to keep them together.
        assert!(!fix.contains("\"/billing\""), "{fix}");
        let order = f.iter().find(|x| x.kind == "step_order").unwrap();
        assert!(
            order
                .fix
                .as_deref()
                .unwrap()
                .contains("[\"item_added\", \"cart_opened\"]")
        );
    }

    #[test]
    fn mixed_use_and_small_samples_suggest_nothing() {
        let m = map();
        let mut s = sessions();
        // Everyone does both billing pages, in the declared step order.
        for v in s.values_mut() {
            v.features.insert(
                "billing".into(),
                visit(1.0, &[("/billing/invoices", 1.0), ("/billing/cards", 2.0)]),
            );
            v.features.insert(
                "cart".into(),
                visit(3.0, &[("cart_opened", 3.0), ("item_added", 4.0)]),
            );
        }
        let rel = crate::relations::infer(&m, &s);
        let f = findings(&m, &s, &rel);
        assert!(
            !f.iter()
                .any(|x| x.kind == "too_big" || x.kind == "step_order"),
            "{f:?}"
        );
        // Under 20 users nothing is judged.
        let few: Sessions = sessions().into_iter().take(10).collect();
        let rel = crate::relations::infer(&m, &few);
        assert!(findings(&m, &few, &rel).is_empty());
    }
}

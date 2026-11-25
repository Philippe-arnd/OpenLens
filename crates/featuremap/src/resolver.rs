//! The map's feature and role rules, in Rust (#446).
//!
//! [`FeatureMap::feature_sql`] and [`FeatureMap::role_sql`] stay the
//! reference: a `CASE` with one `regexp_like` per route and one `lower()`
//! per row. DataFusion evaluated it on every event of every board, 83 to
//! 91 % of a board's time at 4 M events. [`Resolver`] gives the same answer
//! per row with every route tried in one pass (a `RegexSet` built from the
//! same patterns, with the same regex engine as `regexp_like`), and a
//! lookup for the rest. When it cannot promise the SQL's answer (a role
//! pattern with a `LIKE` escape, a route the regex engine refuses), it is
//! not built and the `CASE` is used.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::{FeatureMap, route_regex};

/// A role rule, as `role_sql` writes it.
#[derive(Debug)]
enum Pattern {
    /// `lower(role) = '<value>'`.
    Exact(String),
    /// `lower(role) LIKE '<value>'`, `*` as the only wildcard.
    Glob(Vec<String>),
}

/// The feature and role of a row, as the map's SQL would compute them.
#[derive(Debug)]
pub struct Resolver {
    renamed: HashMap<String, String>,
    success: HashMap<String, String>,
    routes: regex::RegexSet,
    route_keys: Vec<String>,
    /// `None`: the map has no roles, the raw value is kept.
    roles: Option<Vec<(Pattern, String)>>,
}

impl Resolver {
    /// The resolver of `map`, or `None` when the SQL must be used.
    pub fn new(map: &FeatureMap) -> Option<Self> {
        let mut renamed = HashMap::new();
        let mut success = HashMap::new();
        let mut patterns = Vec::new();
        let mut route_keys = Vec::new();
        for f in &map.features {
            for old in &f.renamed_from {
                // `CASE` order: the first feature listing a name wins.
                renamed.entry(old.clone()).or_insert_with(|| f.key.clone());
            }
            if let Some(ev) = &f.success {
                success.entry(ev.clone()).or_insert_with(|| f.key.clone());
            }
            for r in &f.entry.routes {
                patterns.push(route_regex(r));
                route_keys.push(f.key.clone());
            }
        }
        let routes = regex::RegexSet::new(&patterns).ok()?;
        let roles = if map.roles.is_empty() {
            None
        } else {
            let mut v = Vec::new();
            for r in &map.roles {
                for m in &r.matches {
                    let pat = m.trim().to_lowercase();
                    if pat.contains('\\') {
                        return None;
                    }
                    let p = if pat.contains('*') {
                        Pattern::Glob(pat.split('*').map(str::to_owned).collect())
                    } else {
                        Pattern::Exact(pat)
                    };
                    v.push((p, r.key.clone()));
                }
            }
            Some(v)
        };
        Some(Self {
            renamed,
            success,
            routes,
            route_keys,
            roles,
        })
    }

    /// The feature of an event (`feature_sql`): its renamed feature, its
    /// own `feature`, the feature it is the success event of, then the
    /// first feature whose route matches its path.
    pub fn feature<'a>(
        &'a self,
        event: Option<&str>,
        path: Option<&str>,
        feature: Option<&'a str>,
    ) -> Option<&'a str> {
        if let Some(f) = feature {
            return Some(self.renamed.get(f).map_or(f, String::as_str));
        }
        if let Some(k) = event.and_then(|e| self.success.get(e)) {
            return Some(k);
        }
        let i = self.routes.matches(path?).iter().next()?;
        self.route_keys.get(i).map(String::as_str)
    }

    /// The canonical role of a raw value (`role_sql`): the first rule that
    /// matches its lowercase form, else `unmatched:<raw>`; the raw value
    /// itself when the map has no roles.
    pub fn role<'a>(&'a self, raw: Option<&'a str>) -> Option<Cow<'a, str>> {
        let raw = raw?;
        let Some(rules) = &self.roles else {
            return Some(Cow::Borrowed(raw));
        };
        let lower = raw.to_lowercase();
        for (p, key) in rules {
            let hit = match p {
                Pattern::Exact(v) => *v == lower,
                Pattern::Glob(parts) => glob(parts, &lower),
            };
            if hit {
                return Some(Cow::Borrowed(key));
            }
        }
        Some(Cow::Owned(format!("unmatched:{raw}")))
    }
}

/// `text` against a pattern split on `*`: literal parts, in order, the
/// first anchored at the start and the last at the end.
fn glob(parts: &[String], text: &str) -> bool {
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else {
        return true;
    };
    if parts.len() == 1 {
        return text == first;
    }
    if !text.starts_with(first.as_str())
        || !text.ends_with(last.as_str())
        || text.len() < first.len() + last.len()
    {
        return false;
    }
    let Some(mut rest) = text.get(first.len()..text.len() - last.len()) else {
        return false;
    };
    for mid in parts.get(1..parts.len() - 1).unwrap_or_default() {
        match rest.find(mid.as_str()) {
            Some(at) => rest = rest.get(at + mid.len()..).unwrap_or_default(),
            None => return false,
        }
    }
    true
}

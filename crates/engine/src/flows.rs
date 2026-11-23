//! Feature flow (#224): how sessions move between the features of the
//! feature map, as a Sankey. Each session becomes its ordered features
//! (repeats collapsed), cut at [`DEPTH`] steps, and ends in *success*
//! (it reached a success event) or *exit*. Per step, the widest
//! [`WIDTH`] nodes are kept and the rest fold into *Other*.

use std::collections::{BTreeMap, HashMap};

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, lit, source, text};

/// Most steps shown.
pub const DEPTH: usize = 5;
/// Most nodes per step before folding into *Other*.
pub const WIDTH: usize = 8;
/// Node for a session that reached a success event.
pub const SUCCESS: &str = "✓ success";
/// Node for a session that ended without one.
pub const EXIT: &str = "exit";
/// Node for the folded rest of a step.
pub const OTHER: &str = "Other";

/// One link between two steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowLink {
    /// Step of `from` (0-based); `to` is at `step + 1`.
    pub step: usize,
    /// Source node.
    pub from: String,
    /// Target node.
    pub to: String,
    /// Sessions that took it.
    pub sessions: i64,
}

/// A flow: nodes per step and the links between them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Flow {
    /// Nodes of each step with their sessions, widest first.
    pub steps: Vec<Vec<(String, i64)>>,
    /// Links, widest first within a step.
    pub links: Vec<FlowLink>,
    /// Sessions in the flow.
    pub sessions: i64,
    /// Of which reached a success event.
    pub succeeded: i64,
}

/// Which sessions and where they start.
#[derive(Debug, Clone, Default)]
pub struct FlowQuery {
    /// Start at the first use of this feature (otherwise the session's
    /// first feature).
    pub from: Option<String>,
    /// Only sessions that reached a success event.
    pub to_success: bool,
}

/// Fold sessions (each its ordered features, and whether it succeeded)
/// into a [`Flow`]. Pure, so it is tested without data.
#[must_use]
pub fn fold(sessions: &[(Vec<String>, bool)], q: &FlowQuery) -> Flow {
    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut succeeded = 0;
    for (features, ok) in sessions {
        if q.to_success && !ok {
            continue;
        }
        let mut path: Vec<String> = Vec::new();
        for f in features {
            if path.last() != Some(f) {
                path.push(f.clone());
            }
        }
        if let Some(from) = &q.from {
            match path.iter().position(|f| f == from) {
                Some(i) => {
                    path.drain(..i);
                }
                None => continue,
            }
        }
        if path.is_empty() {
            continue;
        }
        path.truncate(DEPTH);
        path.push(if *ok { SUCCESS } else { EXIT }.to_owned());
        if *ok {
            succeeded += 1;
        }
        paths.push(path);
    }
    // Keep the widest nodes per step; others become *Other* (the end nodes
    // always stay).
    let steps = paths.iter().map(Vec::len).max().unwrap_or(0);
    let mut keep: Vec<Vec<String>> = Vec::new();
    for s in 0..steps {
        let mut count: HashMap<&str, i64> = HashMap::new();
        for p in &paths {
            if let Some(n) = p.get(s) {
                *count.entry(n.as_str()).or_default() += 1;
            }
        }
        let mut v: Vec<(&str, i64)> = count.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        keep.push(
            v.iter()
                .enumerate()
                .filter(|(i, (n, _))| *i < WIDTH || *n == SUCCESS || *n == EXIT)
                .map(|(_, (n, _))| (*n).to_owned())
                .collect(),
        );
    }
    for p in &mut paths {
        for (s, n) in p.iter_mut().enumerate() {
            if !keep[s].contains(n) {
                *n = OTHER.to_owned();
            }
        }
    }
    let mut nodes: Vec<BTreeMap<String, i64>> = vec![BTreeMap::new(); steps];
    let mut links: BTreeMap<(usize, String, String), i64> = BTreeMap::new();
    for p in &paths {
        for (s, n) in p.iter().enumerate() {
            *nodes[s].entry(n.clone()).or_default() += 1;
            if let Some(next) = p.get(s + 1) {
                *links.entry((s, n.clone(), next.clone())).or_default() += 1;
            }
        }
    }
    let mut out_links: Vec<FlowLink> = links
        .into_iter()
        .map(|((step, from, to), sessions)| FlowLink {
            step,
            from,
            to,
            sessions,
        })
        .collect();
    out_links.sort_by(|a, b| a.step.cmp(&b.step).then(b.sessions.cmp(&a.sessions)));
    Flow {
        steps: nodes
            .into_iter()
            .map(|m| {
                let mut v: Vec<(String, i64)> = m.into_iter().collect();
                v.sort_by(|a, b| {
                    let end = |n: &str| n == SUCCESS || n == EXIT;
                    end(&a.0)
                        .cmp(&end(&b.0))
                        .then(b.1.cmp(&a.1))
                        .then(a.0.cmp(&b.0))
                });
                v
            })
            .collect(),
        links: out_links,
        sessions: paths.len() as i64,
        succeeded,
    }
}

/// The flow of the last 30 days (sessions with at least one feature; at
/// most 300 000 events read).
pub async fn flows(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    fq: &FlowQuery,
) -> Result<Flow> {
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
        "FALSE".to_owned()
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
    let sql = format!(
        "SELECT session_id, feature, CASE WHEN {ok} THEN 1 ELSE 0 END AS ok FROM {src} \
         WHERE {w} AND session_id IS NOT NULL AND (feature IS NOT NULL OR {ok}) \
         ORDER BY session_id, timestamp LIMIT 300000"
    );
    let rows = q.sql(project, &sql).await?.rows;
    let mut sessions: Vec<(Vec<String>, bool)> = Vec::new();
    let mut current: Option<String> = None;
    for r in &rows {
        let sid = text(r, "session_id");
        if sid != current {
            sessions.push((Vec::new(), false));
            current = sid;
        }
        let Some(last) = sessions.last_mut() else {
            continue;
        };
        if r.get("ok").and_then(serde_json::Value::as_i64) == Some(1) {
            last.1 = true;
        }
        if let Some(feat) = text(r, "feature") {
            last.0.push(feat);
        }
    }
    Ok(fold(&sessions, fq))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn s(f: &[&str], ok: bool) -> (Vec<String>, bool) {
        (f.iter().map(|x| (*x).to_owned()).collect(), ok)
    }

    #[test]
    fn folds_repeats_ends_and_starts() {
        let data = vec![
            s(&["search", "search", "cart", "pay"], true),
            s(&["search", "cart"], false),
            s(&["home", "search"], false),
            s(&[], true),
        ];
        let f = fold(&data, &FlowQuery::default());
        assert_eq!((f.sessions, f.succeeded), (3, 1));
        assert_eq!(f.steps[0], vec![("search".into(), 2), ("home".into(), 1)]);
        assert!(f.links.contains(&FlowLink {
            step: 2,
            from: "pay".into(),
            to: SUCCESS.into(),
            sessions: 1
        }));
        assert!(f.links.contains(&FlowLink {
            step: 1,
            from: "cart".into(),
            to: EXIT.into(),
            sessions: 1
        }));
        let after = fold(
            &data,
            &FlowQuery {
                from: Some("cart".into()),
                to_success: false,
            },
        );
        assert_eq!(after.sessions, 2);
        assert_eq!(after.steps[0], vec![("cart".into(), 2)]);
        let won = fold(
            &data,
            &FlowQuery {
                from: None,
                to_success: true,
            },
        );
        assert_eq!((won.sessions, won.succeeded), (1, 1));
    }

    #[test]
    fn folds_narrow_nodes_into_other_and_cuts_depth() {
        let mut data = Vec::new();
        for i in 0..12 {
            let k = format!("f{i}");
            for _ in 0..(12 - i) {
                data.push((
                    vec![
                        k.clone(),
                        "a".into(),
                        "b".into(),
                        "c".into(),
                        "d".into(),
                        "e".into(),
                        "g".into(),
                    ],
                    false,
                ));
            }
        }
        let f = fold(&data, &FlowQuery::default());
        assert_eq!(f.steps[0].len(), WIDTH + 1, "8 kept + Other");
        assert!(f.steps[0].iter().any(|(n, _)| n == OTHER));
        assert_eq!(f.steps.len(), DEPTH + 1, "5 steps + the end");
    }
}

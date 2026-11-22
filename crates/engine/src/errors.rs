//! Error groups: `$error` events grouped by a fingerprint of their
//! normalised message (numbers, ids and quoted values masked) and source
//! file, with the users, roles, releases and places they hit.

use serde::Serialize;

use crate::{Filters, Result, int, text};
use openlens_query::QueryEngine;

/// One group of errors.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorGroup {
    /// Fingerprint (normalised message @ source file).
    pub fingerprint: String,
    /// One real message of the group.
    pub sample: String,
    /// Occurrences.
    pub count: i64,
    /// Distinct users hit.
    pub users: i64,
    /// Distinct sessions hit.
    pub sessions: i64,
    /// First and last time seen.
    pub first_seen: String,
    /// Last time seen.
    pub last_seen: String,
    /// Most affected roles, releases and places (feature or page), with users.
    pub roles: Vec<(String, i64)>,
    /// Releases.
    pub releases: Vec<(String, i64)>,
    /// Places.
    pub places: Vec<(String, i64)>,
    /// The session of the latest occurrence (links to its replay).
    pub last_session: Option<String>,
    /// The stack of the latest occurrence, as sent (minified).
    pub stack: Option<String>,
    /// The release of the latest occurrence (to pick its source maps).
    pub last_release: Option<String>,
}

/// The top 20 error groups of the last 30 days, most users first.
pub async fn error_groups(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Vec<ErrorGroup>> {
    let w = f.clause(30)?;
    let base = format!(
        "WITH e AS (SELECT distinct_id, session_id, timestamp, role, release, coalesce(feature, path) AS place, \
           regexp_match(properties, '\"\\$message\":\"((?:[^\"\\\\]|\\\\.)*)\"')[1] AS msg, \
           regexp_match(properties, '\"\\$source\":\"([^\"]*)\"')[1] AS source, \
           regexp_match(properties, '\"\\$stack\":\"((?:[^\"\\\\]|\\\\.)*)\"')[1] AS stack \
           FROM {src} WHERE {w} AND event = '$error'), \
         g AS (SELECT *, concat( \
             regexp_replace(regexp_replace(coalesce(msg, '(no message)'), '0x[0-9a-fA-F]+|[0-9a-fA-F-]{{12,}}|[0-9]+', 'N', 'g'), \
                            '''[^'']*''|\\\\\"[^\\\\]*\\\\\"', '…', 'g'), \
             ' @ ', coalesce(regexp_replace(regexp_replace(source, '[?#].*$', ''), ':[0-9]+:[0-9]+$', ''), '?')) AS fp FROM e)"
    );
    let groups_sql = format!(
        "{base} SELECT fp, min(msg) AS sample, count(*) AS n, count(DISTINCT distinct_id) AS users, \
         count(DISTINCT session_id) AS sessions, CAST(min(timestamp) AS VARCHAR) AS first_seen, \
         CAST(max(timestamp) AS VARCHAR) AS last_seen, \
         last_value(session_id ORDER BY timestamp) AS last_session, \
         last_value(stack ORDER BY timestamp) AS stack, last_value(release ORDER BY timestamp) AS last_release FROM g GROUP BY fp ORDER BY users DESC, n DESC LIMIT 20"
    );
    let dims_sql = format!(
        "{base} SELECT fp, 'role' AS dim, coalesce(role, 'not identified') AS v, count(DISTINCT distinct_id) AS users FROM g GROUP BY fp, role \
         UNION ALL SELECT fp, 'release' AS dim, coalesce(release, 'untagged') AS v, count(DISTINCT distinct_id) AS users FROM g GROUP BY fp, release \
         UNION ALL SELECT fp, 'place' AS dim, coalesce(place, '?') AS v, count(DISTINCT distinct_id) AS users FROM g GROUP BY fp, place"
    );
    let (groups, dims) = futures::join!(q.sql(project, &groups_sql), q.sql(project, &dims_sql));
    let dims = dims?.rows;
    let top = |fp: &str, dim: &str| {
        let mut v: Vec<(String, i64)> = dims
            .iter()
            .filter(|r| {
                text(r, "fp").as_deref() == Some(fp) && text(r, "dim").as_deref() == Some(dim)
            })
            .map(|r| (text(r, "v").unwrap_or_default(), int(r, "users")))
            .collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v.truncate(3);
        v
    };
    Ok(groups?
        .rows
        .iter()
        .map(|r| {
            let fp = text(r, "fp").unwrap_or_default();
            ErrorGroup {
                roles: top(&fp, "role"),
                releases: top(&fp, "release"),
                places: top(&fp, "place"),
                sample: text(r, "sample").unwrap_or_else(|| "(no message)".into()),
                count: int(r, "n"),
                users: int(r, "users"),
                sessions: int(r, "sessions"),
                first_seen: text(r, "first_seen").unwrap_or_default(),
                last_seen: text(r, "last_seen").unwrap_or_default(),
                last_session: text(r, "last_session"),
                // The stack is a JSON string body: undo its escapes.
                stack: text(r, "stack")
                    .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok()),
                last_release: text(r, "last_release"),
                fingerprint: fp,
            }
        })
        .collect())
}

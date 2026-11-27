//! Error groups: `$error` events grouped by a fingerprint of their
//! normalised message (numbers, ids and quoted values masked) and source
//! file, with the users, roles, releases and places they hit, where they
//! were raised (browser or server, #624) and, for a server group, the
//! browser sessions whose requests raised it.

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
    /// User of the latest occurrence (links to their profile).
    pub last_user: Option<String>,
    /// The stack of the latest occurrence, as sent (minified).
    pub stack: Option<String>,
    /// The release of the latest occurrence (to pick its source maps).
    pub last_release: Option<String>,
    /// Raised on a server (`$kind: server`, sent by a server SDK, #593):
    /// the group's origin, browser otherwise.
    pub server: bool,
    /// A server group's browser sessions (#624), latest first, at most
    /// [`LINKED_MAX`]: the session the browser SDK sent with the request
    /// (`x-openlens-session`), or, without one, the session of the failed
    /// request (`$request_failed`) with the same trace id. `replay` is set
    /// by [`mark_replays`].
    pub linked: Vec<LinkedSession>,
}

/// A browser session linked to a server error group (#624).
#[derive(Debug, Clone, Serialize)]
pub struct LinkedSession {
    /// The browser's session id.
    pub session: String,
    /// The user of that session, when known (never `$server`).
    pub user: Option<String>,
    /// The latest linked occurrence (UTC).
    pub last_seen: String,
    /// A recording of that session exists.
    pub replay: bool,
}

/// Most browser sessions kept per server group.
pub const LINKED_MAX: usize = 5;

/// Which error groups to read (#624): every group, or those raised in the
/// browser or on a server only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Every group.
    #[default]
    All,
    /// Groups raised in the browser.
    Browser,
    /// Groups raised on a server.
    Server,
}

impl Origin {
    /// From a request value: `browser` or `server`, anything else is every
    /// group (the value is hostile: compared, never stored).
    #[must_use]
    pub fn parse(v: Option<&str>) -> Self {
        match v.map(str::trim) {
            Some("browser") => Self::Browser,
            Some("server") => Self::Server,
            _ => Self::All,
        }
    }

    /// The request value, empty for every group.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Browser => "browser",
            Self::Server => "server",
        }
    }
}

/// 1 for an error raised on a server (`$kind: server`, #593), else 0.
/// Reads the column `properties`.
pub(crate) const SERVER_ERROR: &str =
    "CASE WHEN strpos(properties, '\"$kind\":\"server\"') > 0 THEN 1 ELSE 0 END";

/// The W3C trace id an event carries (`trace_id`, #601), or NULL. Reads
/// the column `properties`.
pub(crate) const TRACE_ID: &str = "regexp_match(properties, '\"trace_id\":\"([0-9a-f]{32})\"')[1]";

/// The pseudo user of server events tied to no user (#593): never a user.
pub(crate) const SERVER_ID: &str = openlens_event::names::SERVER_ID;

/// An error's group (#522 shares it): its message with numbers, ids and
/// quoted values masked, `@`, and its source file without query, line and
/// column. Reads the columns `msg` and `source`.
pub(crate) const FINGERPRINT: &str = "concat( \
     regexp_replace(regexp_replace(coalesce(msg, '(no message)'), '0x[0-9a-fA-F]+|[0-9a-fA-F-]{12,}|[0-9]+', 'N', 'g'), \
                    '''[^'']*''|\\\\\"[^\\\\]*\\\\\"', '…', 'g'), \
     ' @ ', coalesce(regexp_replace(regexp_replace(source, '[?#].*$', ''), ':[0-9]+:[0-9]+$', ''), '?'))";

/// The browser sessions of server errors (#624), per group `fp`: the
/// session each was sent with, else the session of the failed request
/// (`$request_failed`, read from `src` where `w` holds) with its trace id.
/// `base` defines `g` with the columns `fp`, `session_id`, `uid`,
/// `timestamp`, `trace` and `srv`. Rows `fp`, `sid`, `uid` and `seen`,
/// the latest 1000 pairs at most.
pub(crate) fn linked_sql(base: &str, src: &str, w: &str) -> String {
    format!(
        "{base}, s AS (SELECT fp, session_id, uid, timestamp, trace FROM g WHERE srv = 1), \
         b AS (SELECT session_id, nullif(distinct_id, '{SERVER_ID}') AS uid, timestamp, {TRACE_ID} AS trace \
               FROM {src} WHERE {w} AND event = '$request_failed' AND session_id IS NOT NULL), \
         l AS (SELECT fp, session_id, uid, timestamp FROM s WHERE session_id IS NOT NULL \
               UNION ALL SELECT s.fp, b.session_id, b.uid, b.timestamp FROM s JOIN b ON s.trace = b.trace \
               WHERE s.session_id IS NULL AND b.trace IS NOT NULL) \
         SELECT fp, session_id AS sid, max(uid) AS uid, CAST(max(timestamp) AS VARCHAR) AS seen \
         FROM l WHERE session_id <> '' GROUP BY fp, session_id ORDER BY seen DESC LIMIT 1000"
    )
}

/// The top 20 error groups of the last 30 days, most users first.
pub async fn error_groups(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Vec<ErrorGroup>> {
    error_groups_of(q, project, src, f, Origin::All).await
}

/// [`error_groups`] raised in the browser or on a server only (#624).
/// Users are people: the `$server` pseudo user is none (#593 D).
pub async fn error_groups_of(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    origin: Origin,
) -> Result<Vec<ErrorGroup>> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause(30)?;
    let base = format!(
        "WITH e AS (SELECT nullif(distinct_id, '{SERVER_ID}') AS uid, session_id, timestamp, role, release, \
           coalesce(feature, path) AS place, error_message AS msg, error_source AS source, error_stack AS stack, \
           {SERVER_ERROR} AS srv, {TRACE_ID} AS trace \
           FROM {src} WHERE {w} AND event = '$error'), \
         g AS (SELECT *, {FINGERPRINT} AS fp FROM e)"
    );
    let having = match origin {
        Origin::All => "",
        Origin::Browser => " HAVING max(srv) = 0",
        Origin::Server => " HAVING max(srv) = 1",
    };
    let groups_sql = format!(
        "{base} SELECT fp, min(msg) AS sample, count(*) AS n, count(DISTINCT uid) AS users, \
         count(DISTINCT session_id) AS sessions, CAST(min(timestamp) AS VARCHAR) AS first_seen, \
         CAST(max(timestamp) AS VARCHAR) AS last_seen, \
         last_value(session_id ORDER BY timestamp) AS last_session, \
         last_value(uid ORDER BY timestamp) AS last_user, max(srv) AS server, \
         last_value(stack ORDER BY timestamp) AS stack, last_value(release ORDER BY timestamp) AS last_release \
         FROM g GROUP BY fp{having} ORDER BY users DESC, sessions DESC, n DESC LIMIT 20"
    );
    let dims_sql = format!(
        "{base} SELECT fp, 'role' AS dim, coalesce(role, 'not identified') AS v, count(DISTINCT uid) AS users FROM g GROUP BY fp, role \
         UNION ALL SELECT fp, 'release' AS dim, coalesce(release, 'untagged') AS v, count(DISTINCT uid) AS users FROM g GROUP BY fp, release \
         UNION ALL SELECT fp, 'place' AS dim, coalesce(place, '?') AS v, count(DISTINCT uid) AS users FROM g GROUP BY fp, place"
    );
    // A server error's browser sessions: the one it was sent with, else
    // the session of the failed request with its trace id. Bounded: the
    // latest pairs only, then LINKED_MAX per group.
    let linked_sql = linked_sql(&base, src, &w);
    let (groups, dims, linked) = futures::join!(
        q.sql(project, &groups_sql),
        q.sql(project, &dims_sql),
        q.sql(project, &linked_sql)
    );
    let dims = dims?.rows;
    let linked = linked?.rows;
    let sessions_of = |fp: &str| -> Vec<LinkedSession> {
        linked
            .iter()
            .filter(|r| text(r, "fp").as_deref() == Some(fp))
            .filter_map(|r| {
                Some(LinkedSession {
                    session: text(r, "sid")?,
                    user: text(r, "uid"),
                    last_seen: text(r, "seen").unwrap_or_default(),
                    replay: false,
                })
            })
            .take(LINKED_MAX)
            .collect()
    };
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
                last_user: text(r, "last_user"),
                // The stack is a JSON string body: undo its escapes.
                stack: text(r, "stack")
                    .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok()),
                last_release: text(r, "last_release"),
                server: int(r, "server") > 0,
                linked: sessions_of(&fp),
                fingerprint: fp,
            }
        })
        .collect())
}

/// Mark the linked sessions that have a recording (#624): a file check
/// each, at most 20 × [`LINKED_MAX`]. Blocking: run it off the async
/// threads.
pub fn mark_replays(root: &std::path::Path, project: &str, groups: &mut [ErrorGroup]) {
    for s in groups.iter_mut().flat_map(|g| g.linked.iter_mut()) {
        s.replay = openlens_replay::exists(root, project, &s.session);
    }
}

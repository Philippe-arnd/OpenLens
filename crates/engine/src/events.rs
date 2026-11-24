//! The event list and its families. Moved out of lib.rs as is (#307).

use super::*;

/// Event families, read at a glance on the events page (#217): the key,
/// its label, and the event names in it. `custom` is every event the app
/// names itself (no `$`), `other` the remaining `$` events.
pub const FAMILIES: &[(&str, &str, &[&str])] = &[
    ("page", "Page views", &["$pageview", "$scroll"]),
    ("click", "Clicks", &["$click", "$rage"]),
    ("form", "Forms", &["$submit"]),
    ("feature", "Features", &["$feature"]),
    ("identify", "Logins", &["$identify"]),
    ("error", "Errors", &["$error", "$request_failed"]),
    ("vital", "Web vitals", &["$vital"]),
    ("survey", "Surveys", &["$survey"]),
    ("custom", "App events", &[]),
    ("other", "Other", &[]),
];

/// The family of an event name (a key of [`FAMILIES`]).
#[must_use]
pub fn family(event: &str) -> &'static str {
    if !event.starts_with('$') {
        return "custom";
    }
    FAMILIES
        .iter()
        .find(|(_, _, names)| names.contains(&event))
        .map_or("other", |(k, _, _)| k)
}

/// How the events page lists events: filters, sort and position.
#[derive(Debug, Clone, Default)]
pub struct EventQuery {
    /// One event name.
    pub event: Option<String>,
    /// A family key of [`FAMILIES`].
    pub family: Option<String>,
    /// One user (`distinct_id`).
    pub person: Option<String>,
    /// Text the page path contains.
    pub path: Option<String>,
    /// Days back (1 to 90; default 30).
    pub days: Option<i64>,
    /// `time` (newest first, default), `event` or `person`.
    pub sort: Option<String>,
    /// Newest-first cursor: only events strictly older than this timestamp.
    pub before: Option<String>,
    /// Rows to skip (event and person sorts; at most 2000).
    pub skip: usize,
    /// Rows to return (at most 200).
    pub limit: usize,
}

/// Events matching an [`EventQuery`].
pub async fn event_list(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
    eq: &EventQuery,
) -> Result<Vec<EventRow>> {
    let q = &q.with_params(&f.params()?);
    let src = source(map);
    let mut w = f.clause(eq.days.unwrap_or(30).clamp(1, 90))?;
    if let Some(e) = &eq.event {
        w.push_str(&format!(" AND event = {}", lit(e)?));
    }
    if let Some(p) = &eq.person {
        w.push_str(&format!(" AND distinct_id = {}", lit(p)?));
    }
    if let Some(p) = &eq.path {
        w.push_str(&format!(" AND strpos(path, {}) > 0", lit(p)?));
    }
    if let Some(fam) = eq.family.as_deref() {
        let known = |names: &[&str]| -> Result<String> {
            Ok(names
                .iter()
                .map(|n| lit(n))
                .collect::<Result<Vec<_>>>()?
                .join(", "))
        };
        match fam {
            "custom" => w.push_str(" AND event NOT LIKE '$%'"),
            "other" => {
                let all: Vec<&str> = FAMILIES
                    .iter()
                    .flat_map(|(_, _, n)| n.iter().copied())
                    .collect();
                w.push_str(&format!(
                    " AND event LIKE '$%' AND event NOT IN ({})",
                    known(&all)?
                ));
            }
            k => match FAMILIES.iter().find(|(key, _, _)| *key == k) {
                Some((_, _, names)) => w.push_str(&format!(" AND event IN ({})", known(names)?)),
                None => return Err(EngineError::InvalidFilter("unknown event family")),
            },
        }
    }
    let order = match eq.sort.as_deref() {
        Some("event") => "event, timestamp DESC",
        Some("person") => "distinct_id, timestamp DESC",
        _ => {
            if let Some(b) = &eq.before {
                w.push_str(&format!(" AND timestamp < CAST({} AS TIMESTAMP)", lit(b)?));
            }
            "timestamp DESC"
        }
    };
    let skip = if eq
        .sort
        .as_deref()
        .is_some_and(|s| s == "event" || s == "person")
    {
        eq.skip.min(2000)
    } else {
        0
    };
    let sql = format!(
        "SELECT CAST(timestamp AS VARCHAR) AS ts, event, distinct_id, role, path, feature, release, session_id \
         FROM {src} WHERE {w} ORDER BY {order} LIMIT {} OFFSET {skip}",
        eq.limit.clamp(1, 200)
    );
    let r = q.sql(project, &sql).await?;
    Ok(r.rows
        .iter()
        .map(|r| EventRow {
            timestamp: text(r, "ts").unwrap_or_default(),
            event: text(r, "event").unwrap_or_default(),
            distinct_id: text(r, "distinct_id").unwrap_or_default(),
            role: text(r, "role"),
            path: text(r, "path"),
            feature: text(r, "feature"),
            release: text(r, "release"),
            session_id: text(r, "session_id"),
        })
        .collect())
}

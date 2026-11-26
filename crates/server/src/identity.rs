//! Identity kept at ingestion (#450, D-035): every stored `$identify` is
//! recorded in the metadata store (alias and person, with a dense id), and
//! the query engine reads those tables instead of scanning the events.
//!
//! On upgrade, each project's tables are filled once from its files, in
//! the background ([`backfill`]); until then the engine keeps deriving them
//! from the files, so results never depend on whether it ran.

use std::sync::Arc;

use openlens_event::Event;
use openlens_meta::{Identified, Meta};

use crate::AppState;

/// The key the SDK puts the visitor's anonymous id under.
const ANON_KEY: &str = "\"$anon_distinct_id\":\"";
/// Longest anonymous id linked, in characters (as the query layer reads it).
const MAX_ANON_CHARS: usize = 200;

/// The anonymous id of an `$identify`'s properties, read exactly as the
/// query layer did (`"$anon_distinct_id":"([^"]{1,200})"`, first match).
fn anon_of(properties: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(i) = properties.get(from..).and_then(|s| s.find(ANON_KEY)) {
        let start = from + i + ANON_KEY.len();
        let rest = properties.get(start..).unwrap_or_default();
        if let Some(end) = rest.find('"') {
            let value = rest.get(..end).unwrap_or_default();
            if (1..=MAX_ANON_CHARS).contains(&value.chars().count()) {
                return Some(value);
            }
        }
        // The key starts with an ASCII quote: one byte on is a boundary.
        from += i + 1;
    }
    None
}

/// What an `$identify` event records: `(person, anon, first_seen, internal)`.
fn identified(e: &Event) -> Identified {
    (
        e.distinct_id.clone(),
        anon_of(&e.properties)
            .filter(|a| *a != e.distinct_id)
            .map(str::to_owned),
        e.timestamp.timestamp_micros(),
        e.properties.contains("\"$internal\":true"),
    )
}

/// Record the `$identify` events of `events` (blocking). Called before the
/// events are written, by capture and import alike. A change is seen by
/// the next query, not 30 s later.
pub(crate) fn record(state: &AppState, project: &str, events: &[Event]) -> Result<(), String> {
    let rows: Vec<Identified> = events
        .iter()
        .filter(|e| e.event == openlens_event::names::IDENTIFY && e.project == project)
        .map(identified)
        .collect();
    let changed = state
        .meta
        .record_identities(project, &rows)
        .map_err(|e| e.to_string())?;
    if changed {
        state.query.identity_changed(project);
    }
    Ok(())
}

/// Where the query engine reads identity from: the metadata store, once a
/// project's backfill ran.
pub fn source(meta: Arc<Meta>) -> openlens_query::IdentitySource {
    openlens_query::IdentitySource(Arc::new(move |project: &str| {
        let Some(t) = meta.identities(project).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        Ok(Some(openlens_query::IdentityRows {
            aliases: t.aliases,
            persons: t
                .persons
                .into_iter()
                .map(|p| openlens_query::PersonRow {
                    person: p.person,
                    first_seen: p.first_seen,
                    internal: p.internal,
                })
                .collect(),
        }))
    }))
}

/// Fill `project`'s identity tables from its files, once (upgrade from a
/// release that did not keep them). Does nothing when it already ran.
pub async fn backfill(state: &AppState, project: &str) -> Result<(), String> {
    let (meta, p) = (state.meta.clone(), project.to_owned());
    let ready = tokio::task::spawn_blocking(move || meta.identities(&p).map(|t| t.is_some()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if ready {
        // A copy derived from the files before the tables were ready is
        // not kept for its 30 s: the next query reads the tables.
        state.query.identity_reload(project);
        return Ok(());
    }
    let rows = state
        .query
        .identity_from_files(project)
        .await
        .map_err(|e| e.to_string())?;
    let (meta, p) = (state.meta.clone(), project.to_owned());
    tokio::task::spawn_blocking(move || {
        let persons: Vec<(String, i64, bool)> = rows
            .persons
            .into_iter()
            .map(|r| (r.person, r.first_seen, r.internal))
            .collect();
        meta.backfill_identities(&p, &persons, &rows.aliases)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    state.query.identity_reload(project);
    tracing::info!(project, "identity: tables filled from the stored events");
    Ok(())
}

/// Start [`backfill`] for `project` in the background, at most once per
/// ten minutes (a failure is retried then, not on every batch).
pub(crate) fn ensure(state: &AppState, project: &str) {
    let key = project.to_owned();
    if state.caches.identity_checked.get(&key).is_some() {
        return;
    }
    state.caches.identity_checked.insert(key.clone(), ());
    let st = state.clone();
    tokio::spawn(async move {
        if let Err(err) = backfill(&st, &key).await {
            tracing::warn!(project = %key, %err, "identity: backfill failed; retried later");
        }
    });
}

/// The backfill of every project, one after the other, at startup.
pub(crate) async fn backfill_all(state: AppState) {
    let meta = state.meta.clone();
    let projects = match tokio::task::spawn_blocking(move || meta.projects()).await {
        Ok(Ok(p)) => p,
        _ => return,
    };
    for p in projects {
        state.caches.identity_checked.insert(p.id.clone(), ());
        if let Err(err) = backfill(&state, &p.id).await {
            tracing::warn!(project = %p.id, %err, "identity: backfill failed; retried later");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::anon_of;

    /// The same matches as the query layer's regular expression.
    #[test]
    fn anonymous_ids_are_read_as_the_files_were() {
        assert_eq!(anon_of(r#"{"$anon_distinct_id":"a-1"}"#), Some("a-1"));
        assert_eq!(anon_of(r#"{"$anon_distinct_id":""}"#), None);
        let long = "x".repeat(201);
        assert_eq!(
            anon_of(&format!(r#"{{"$anon_distinct_id":"{long}"}}"#)),
            None
        );
        let max = "é".repeat(200);
        assert_eq!(
            anon_of(&format!(r#"{{"$anon_distinct_id":"{max}"}}"#)),
            Some(max.as_str())
        );
        // A first value that does not match does not hide a later one.
        assert_eq!(
            anon_of(r#"{"$anon_distinct_id":"","k":{"$anon_distinct_id":"b"}}"#),
            Some("b")
        );
        assert_eq!(anon_of(r#"{"$anon_distinct_id":"open"#), None);
    }
}

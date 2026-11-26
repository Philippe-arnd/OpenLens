//! Automatic releases at ingestion (#495, D-044): an event with no release
//! but a `$build` fingerprint gets the name that fingerprint has in the
//! project and environment ([`openlens_meta::releases`]); an explicit
//! release always wins and becomes the fingerprint's name for later events.
//! Names are cached, so a known fingerprint costs no metadata read.

use chrono::{DateTime, Utc};
use openlens_event::Event;
use serde::Deserialize;

use super::BOT_ENVIRONMENT;
use crate::AppState;

/// Longest fingerprint taken; the browser SDK sends 8 hex characters.
const MAX_BUILD: usize = 64;

#[derive(Deserialize)]
struct BuildOnly {
    #[serde(rename = "$build")]
    build: Option<serde_json::Value>,
}

/// The `$build` fingerprint in an event's properties, when it is a short
/// string of letters, digits, `-` and `_`.
fn build_of(properties: &str) -> Option<String> {
    // Most events have none: skip the parse.
    if !properties.contains("\"$build\"") {
        return None;
    }
    let b = serde_json::from_str::<BuildOnly>(properties).ok()?.build?;
    let b = b.as_str()?;
    let ok = !b.is_empty()
        && b.len() <= MAX_BUILD
        && b.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    ok.then(|| b.to_owned())
}

/// Give each event of `project` with a fingerprint and no release its
/// automatic name, and record explicit releases as the names of their
/// fingerprints. A metadata failure is logged and leaves the event as sent.
pub(crate) fn assign(
    state: &AppState,
    project: &str,
    timezone: &str,
    events: &mut [Event],
    now: DateTime<Utc>,
) {
    let tz = timezone.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC);
    let day = now.with_timezone(&tz).format("%Y-%m-%d").to_string();
    let caches = &state.caches;
    for ev in events.iter_mut() {
        if ev.environment == BOT_ENVIRONMENT {
            continue;
        }
        let Some(build) = build_of(&ev.properties) else {
            continue;
        };
        let key = format!("{project}|{}|{build}", ev.environment);
        let cached = caches.release_builds.get(&key);
        if let (Some(c), None) = (&cached, &ev.release) {
            ev.release = Some(c.clone());
            continue;
        }
        if cached.is_some() && cached == ev.release {
            continue;
        }
        match resolve(
            state,
            project,
            &ev.environment,
            &build,
            ev.release.as_deref(),
            &day,
            now,
        ) {
            Ok(Some(name)) => {
                caches.release_builds.insert(key, name.clone());
                if ev.release.is_none() {
                    ev.release = Some(name);
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(project, %err, "releases: build not recorded"),
        }
    }
}

/// The name of a fingerprint not in the cache: read first (a reader
/// connection), and write only for a new fingerprint or a new explicit name.
fn resolve(
    state: &AppState,
    project: &str,
    environment: &str,
    build: &str,
    explicit: Option<&str>,
    day: &str,
    now: DateTime<Utc>,
) -> Result<Option<String>, openlens_meta::MetaError> {
    let known = state.meta.build_release(project, environment, build)?;
    match (&known, explicit) {
        (Some(k), None) => return Ok(Some(k.clone())),
        (Some(k), Some(e)) if k == e => return Ok(known),
        _ => {}
    }
    // A day already full for this project refuses new fingerprints without
    // taking the writer.
    let full = format!("{project}|{day}");
    if known.is_none() && state.caches.release_days_full.get(&full).is_some() {
        return Ok(None);
    }
    let name =
        state
            .meta
            .record_build(project, environment, build, explicit, day, now.timestamp())?;
    if name.is_none() {
        state.caches.release_days_full.insert(full, ());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::build_of;

    #[test]
    fn only_a_short_plain_fingerprint_is_taken() {
        assert_eq!(
            build_of(r#"{"$build":"3f9a1c0b"}"#).as_deref(),
            Some("3f9a1c0b")
        );
        for bad in [
            r#"{"$build":""}"#,
            r#"{"$build":12}"#,
            r#"{"$build":"a b"}"#,
            r#"{"$build":"../x"}"#,
            r#"{"build":"x"}"#,
            r#"{"note":"$build"}"#,
        ] {
            assert_eq!(build_of(bad), None, "{bad}");
        }
        assert_eq!(
            build_of(&format!(r#"{{"$build":"{}"}}"#, "a".repeat(65))),
            None
        );
    }
}

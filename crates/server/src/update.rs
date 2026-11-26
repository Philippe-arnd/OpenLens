//! The daily check for a newer release (#198): the footer notice, louder
//! when the release fixes a published advisory (#416).

use std::time::Duration;

/// `vX.Y.Z` as numbers; pre-releases and other tags are ignored.
fn stable_version(tag: &str) -> Option<(u64, u64, u64)> {
    let mut it = tag.strip_prefix('v').unwrap_or(tag).split('.');
    let v = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    it.next().is_none().then_some(v)
}

/// The newest stable release, when it is newer than `current` (#198).
fn newer_release(current: &str) -> Option<openlens_ui::Update> {
    let body: serde_json::Value =
        ureq::get("https://api.github.com/repos/Philippe-arnd/OpenLens/releases/latest")
            .header("user-agent", "openlens-update-check")
            .header("accept", "application/vnd.github+json")
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
    release_update(&body, current)
}

/// Read a `/releases/latest` answer: the tag when newer than `current`,
/// and whether its notes carry a `Security` heading (#416: the release
/// fixes a published advisory, so the footer says so louder).
fn release_update(body: &serde_json::Value, current: &str) -> Option<openlens_ui::Update> {
    let tag = body.get("tag_name")?.as_str()?.to_owned();
    if stable_version(&tag)? <= stable_version(current)? {
        return None;
    }
    let security = body
        .get("body")
        .and_then(|b| b.as_str())
        .is_some_and(|notes| {
            notes.lines().any(|l| {
                let l = l.trim();
                l.starts_with('#')
                    && l.trim_start_matches('#')
                        .trim()
                        .eq_ignore_ascii_case("security")
            })
        });
    Some(openlens_ui::Update { tag, security })
}

/// Check for a newer release once a day (first check a minute after start).
pub(crate) async fn update_check() {
    if std::env::var("OPENLENS_UPDATE_CHECK").is_ok_and(|v| v == "off") {
        return;
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let found = tokio::task::spawn_blocking(|| newer_release(env!("CARGO_PKG_VERSION")))
            .await
            .ok()
            .flatten();
        if let Some(u) = &found {
            if u.security {
                tracing::warn!(release = %u.tag, "a newer OpenLens release fixes security advisories");
            } else {
                tracing::info!(release = %u.tag, "a newer OpenLens release is available");
            }
        }
        openlens_ui::set_update_available(found);
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn only_newer_stable_releases_count() {
        use super::stable_version as v;
        assert!(v("v0.3.0") > v("0.2.0"));
        assert!(v("v0.10.0") > v("v0.9.9"));
        assert_eq!(v("v0.2.0-261122-755dcee"), None);
        assert_eq!(v("nightly"), None);
    }

    #[test]
    fn a_security_heading_makes_a_security_update() {
        use super::release_update as r;
        let plain = serde_json::json!({"tag_name": "v0.5.0", "body": "## Added\n- things"});
        let sec =
            serde_json::json!({"tag_name": "v0.5.0", "body": "## Added\n\n### Security\n- GHSA-x"});
        assert!(!r(&plain, "0.4.0").unwrap().security);
        assert!(r(&sec, "0.4.0").unwrap().security);
        assert_eq!(r(&sec, "0.4.0").unwrap().tag, "v0.5.0");
        assert!(r(&sec, "0.5.0").is_none(), "same version");
        assert!(
            r(&serde_json::json!({"body": "x"}), "0.4.0").is_none(),
            "no tag"
        );
        let no_body = serde_json::json!({"tag_name": "v0.5.0"});
        assert!(!r(&no_body, "0.4.0").unwrap().security);
        let mention = serde_json::json!({"tag_name": "v0.5.0", "body": "Security is great"});
        assert!(
            !r(&mention, "0.4.0").unwrap().security,
            "only a heading counts"
        );
    }
}

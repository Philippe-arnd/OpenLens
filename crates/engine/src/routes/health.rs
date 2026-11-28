//! Health checks are not route stats (#726, D-110). The probes of a
//! container healthcheck, a load balancer or OpenLens's own uptime check
//! hit the app every few seconds whatever its traffic: kept as route stats
//! they would be most of a small app's stored events and the top of its
//! routes. So the `$request_stats` of a fixed list of health route
//! templates ([`HEALTH_ROUTES`]), plus the project's own uptime health
//! path, are dropped at ingestion, from every SDK and from OTLP alike.
//!
//! Only route stats: a server error (`$error`) on a health route is kept,
//! a failing health check matters.

use openlens_event::{Event, names};

/// The health route templates never kept as route stats, compared without
/// case and without a trailing slash. `/health/*` is any route under
/// `/health/`.
pub const HEALTH_ROUTES: [&str; 11] = [
    "/health",
    "/healthz",
    "/health/*",
    "/api/health",
    "/api/healthz",
    "/livez",
    "/readyz",
    "/ready",
    "/live",
    "/ping",
    "/status",
];

/// A route or path, lowercased, without its query and trailing slash;
/// `None` for the root (a real page) or nothing.
fn canonical(route: &str) -> Option<String> {
    let path = route.trim().split(['?', '#']).next().unwrap_or_default();
    let path = path.trim_end_matches('/');
    (!path.is_empty()).then(|| path.to_ascii_lowercase())
}

/// Is `route` a health check: one of [`HEALTH_ROUTES`], or `health_path`,
/// the project's uptime health path (ignored when it is `/`, the default,
/// which is the app's home page)?
#[must_use]
pub fn is_health_check(route: &str, health_path: Option<&str>) -> bool {
    let Some(r) = canonical(route) else {
        return false;
    };
    let listed = HEALTH_ROUTES.iter().any(|h| match h.strip_suffix("/*") {
        Some(prefix) => r
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/')),
        None => r == *h,
    });
    listed || health_path.and_then(canonical).is_some_and(|p| p == r)
}

/// The `http.route` of a `$request_stats` event, when it has one.
fn route_of(e: &Event) -> Option<String> {
    let props: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&e.properties).ok()?;
    props.get("http.route")?.as_str().map(str::to_owned)
}

/// Does the batch hold route stats at all? Lets the caller skip reading
/// the project's health path for every other batch.
#[must_use]
pub fn has_route_stats(events: &[Event]) -> bool {
    events.iter().any(|e| e.event == names::REQUEST_STATS)
}

/// Drop the `$request_stats` of health checks from `events`; returns how
/// many went. Every other event, `$error` included, is kept.
pub fn drop_health_checks(events: &mut Vec<Event>, health_path: Option<&str>) -> usize {
    let before = events.len();
    events.retain(|e| {
        e.event != names::REQUEST_STATS
            || !route_of(e).is_some_and(|r| is_health_check(&r, health_path))
    });
    before - events.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_templates_match_without_case_or_trailing_slash_and_real_routes_do_not() {
        for r in [
            "/health",
            "/HEALTH/",
            "/api/health",
            "/healthz",
            "/health/live",
            "/health/:check",
            "/readyz/",
            "/Ping",
        ] {
            assert!(is_health_check(r, None), "{r}");
        }
        for r in [
            "/api/items",
            "/healthy",
            "/health-report",
            "/api/status/:id",
            "/",
            "",
            "/api/healthcheck",
        ] {
            assert!(!is_health_check(r, None), "{r}");
        }
    }

    #[test]
    fn the_projects_health_path_counts_but_never_the_home_page() {
        assert!(is_health_check("/internal/up", Some("/internal/up/")));
        assert!(is_health_check("/internal/up", Some("/internal/up?full=1")));
        assert!(
            !is_health_check("/", Some("/")),
            "the default path is the home page"
        );
        assert!(!is_health_check("/internal/upgrade", Some("/internal/up")));
    }
}

//! Planned maintenance windows (#563, D-075): what a window covers, and
//! the two questions the rest of OpenLens asks of them, "is this
//! component under maintenance at `t`?" and "how many seconds of
//! `[from, to)` were planned maintenance for it?". Alerts (no
//! announcement during a window), the Uptime page (availability without
//! that time), the SLO budget (#560) and the status page (#561) read
//! them here.
//!
//! A component is `production` or the name of an environment (#410). A
//! window holds the incidents of the availability checks of the
//! components it covers ([`HELD_CHECKS`]); regressions, new errors and
//! certificates are announced as usual.

use openlens_meta::{Check, Meta};

use crate::environments::split_incident_rule;
pub use openlens_meta::maintenance::Window;

/// Production's component name.
pub const PRODUCTION: &str = "production";

/// The incident checks a window holds: the app down, its traffic stopped,
/// a spike of failing requests.
pub const HELD_CHECKS: [&str; 3] = ["down", "traffic", "failing"];

/// The component whose window holds an incident rule, if any:
/// `production` for production's checks, the environment for
/// `env:<env>:down`. `None` for the rules a window never holds.
#[must_use]
pub fn held_component(rule: &str) -> Option<&str> {
    match split_incident_rule(rule) {
        Some((env, check)) => HELD_CHECKS.contains(&check).then_some(env),
        None => HELD_CHECKS.contains(&rule).then_some(PRODUCTION),
    }
}

/// Does the window cover this component?
#[must_use]
pub fn covers(w: &Window, component: &str) -> bool {
    w.components.iter().any(|c| c == component)
}

/// `planned` (not started at `now`), `running` or `ended`.
#[must_use]
pub fn state_at(w: &Window, now: i64) -> &'static str {
    if now < w.starts_at {
        "planned"
    } else if now < w.ends_at {
        "running"
    } else {
        "ended"
    }
}

/// The window of `windows` that covers `component` at `t` (start
/// included, end excluded).
#[must_use]
pub fn running_at<'a>(windows: &'a [Window], component: &str, t: i64) -> Option<&'a Window> {
    windows
        .iter()
        .find(|w| covers(w, component) && w.starts_at <= t && t < w.ends_at)
}

/// Seconds of `[from, to)` inside a window of `windows` covering
/// `component`; overlapping windows count once.
#[must_use]
pub fn overlap_seconds(windows: &[Window], component: &str, from: i64, to: i64) -> i64 {
    let mut spans: Vec<(i64, i64)> = windows
        .iter()
        .filter(|w| covers(w, component))
        .map(|w| (w.starts_at.max(from), w.ends_at.min(to)))
        .filter(|(a, b)| a < b)
        .collect();
    spans.sort_unstable();
    let mut total = 0;
    let mut cur: Option<(i64, i64)> = None;
    for (a, b) in spans {
        cur = match cur {
            Some((ca, cb)) if a <= cb => Some((ca, cb.max(b))),
            Some((ca, cb)) => {
                total += cb - ca;
                Some((a, b))
            }
            None => Some((a, b)),
        };
    }
    total + cur.map_or(0, |(a, b)| b - a)
}

/// The checks of `checks` that count for availability: those outside a
/// window of `windows` covering `component`.
#[must_use]
pub fn counted<'a>(checks: &'a [Check], windows: &[Window], component: &str) -> Vec<&'a Check> {
    checks
        .iter()
        .filter(|c| running_at(windows, component, c.0).is_none())
        .collect()
}

/// The project's window covering `component` at `t`, if any.
#[must_use]
pub fn in_maintenance(meta: &Meta, project: &str, component: &str, t: i64) -> Option<Window> {
    let windows = meta
        .maintenance_windows(project, t, t.saturating_add(1))
        .unwrap_or_default();
    running_at(&windows, component, t).cloned()
}

/// Seconds of `[from, to)` that were planned maintenance of `component`
/// in the project (0 when the store cannot be read).
#[must_use]
pub fn maintenance_seconds(meta: &Meta, project: &str, component: &str, from: i64, to: i64) -> i64 {
    if to <= from {
        return 0;
    }
    let windows = meta
        .maintenance_windows(project, from, to)
        .unwrap_or_default();
    overlap_seconds(&windows, component, from, to)
}

/// The window that holds an incident of `rule` at `t`: no announcement,
/// no reminder while it runs.
#[must_use]
pub fn holding(meta: &Meta, project: &str, rule: &str, t: i64) -> Option<Window> {
    in_maintenance(meta, project, held_component(rule)?, t)
}

/// The latest end, within `(after, upto]`, of a window that held `rule`:
/// an incident announced then is still failing after its maintenance.
#[must_use]
pub fn ended_between(meta: &Meta, project: &str, rule: &str, after: i64, upto: i64) -> Option<i64> {
    let component = held_component(rule)?;
    meta.maintenance_windows(project, after, upto.saturating_add(1))
        .unwrap_or_default()
        .iter()
        .filter(|w| covers(w, component) && w.ends_at > after && w.ends_at <= upto)
        .map(|w| w.ends_at)
        .max()
}

/// A window's dates in a time zone (D-017): `3 Dec, 22:00–22:30`, or
/// `3 Dec 22:00 – 4 Dec 02:00` across days. An unknown zone reads as
/// UTC.
#[must_use]
pub fn describe(w: &Window, tz: &str) -> String {
    let tz: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
    let at = |t: i64| chrono::DateTime::from_timestamp(t, 0).map(|d| d.with_timezone(&tz));
    match (at(w.starts_at), at(w.ends_at)) {
        (Some(a), Some(b)) if a.date_naive() == b.date_naive() => {
            format!("{}–{}", a.format("%-d %b, %H:%M"), b.format("%H:%M"))
        }
        (Some(a), Some(b)) => format!(
            "{} – {}",
            a.format("%-d %b %H:%M"),
            b.format("%-d %b %H:%M")
        ),
        _ => String::new(),
    }
}

/// A length in words: `45 min`, `2 h`, `1 h 30 min`.
#[must_use]
pub fn length(seconds: i64) -> String {
    let m = (seconds.max(0) + 59) / 60;
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(a: i64, b: i64, comps: &[&str]) -> Window {
        Window {
            id: 1,
            starts_at: a,
            ends_at: b,
            components: comps.iter().map(|c| (*c).to_owned()).collect(),
            text: String::new(),
            created_at: 0,
            created_by: String::new(),
            announced_at: None,
        }
    }

    #[test]
    fn overlapping_windows_count_once_and_are_clipped_to_the_range() {
        let ws = [
            w(100, 200, &["production"]),
            w(150, 300, &["production"]),
            w(400, 500, &["staging"]),
        ];
        assert_eq!(overlap_seconds(&ws, PRODUCTION, 0, 1000), 200);
        assert_eq!(overlap_seconds(&ws, PRODUCTION, 250, 1000), 50);
        assert_eq!(overlap_seconds(&ws, "staging", 0, 450), 50);
        assert_eq!(overlap_seconds(&ws, "preview", 0, 1000), 0);
    }

    #[test]
    fn a_window_holds_only_the_availability_checks_of_its_components() {
        assert_eq!(held_component("down"), Some(PRODUCTION));
        assert_eq!(held_component("env:staging:down"), Some("staging"));
        assert_eq!(held_component("cert_expired"), None);
        assert_eq!(held_component("insight:slow_pages:/a"), None);
        let ws = [w(100, 200, &["staging"])];
        assert!(running_at(&ws, "staging", 100).is_some());
        assert!(running_at(&ws, "staging", 200).is_none());
        assert!(running_at(&ws, PRODUCTION, 150).is_none());
    }
}

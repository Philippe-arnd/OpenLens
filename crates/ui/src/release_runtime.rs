//! A release's **Runtime** line (#631, the UI of #610): its largest RSS
//! and growth, its typical event-loop delay p99 and its restarts per day,
//! each next to the release before. Every number comes from the verdict
//! (`RuntimeShown`); memory and loop delay are shown, never judged, and a
//! crash loop is judged in the Backend group. Without runtime stats there
//! is no line, and on the Release page one line says how to get them.

use super::*;
use crate::routes_page::ms;
use openlens_engine::runtime::RuntimeShown;

/// Bytes as shown.
fn mem(b: i64) -> String {
    crate::settings::bytes(u64::try_from(b).unwrap_or(0))
}

/// A growth as a signed percentage, a real minus.
fn growth(g: f64) -> String {
    format!("{:+.0}%", g * 100.0).replacen('-', "−", 1)
}

/// Restarts per day as shown.
fn per_day(v: f64) -> String {
    format!("{v:.1} a day")
}

/// One part of the line: what, the value now, and the value before.
fn part(what: &str, now: String, was: Option<String>, previous: Option<&str>) -> Markup {
    html! {
        span {
            (what) " " b { (now) }
            @if let Some(w) = was {
                span.muted { " (was " (w) @if let Some(p) = previous { " on " (p) } ")" }
            }
        }
    }
}

/// The Release page's line: each number against the release before
/// (`previous`).
pub(crate) fn line(rt: &RuntimeShown, previous: Option<&str>) -> Markup {
    let rss = rt.rss_max_bytes.map(|b| match rt.rss_growth {
        Some(g) => format!("{}, {}", mem(b), growth(g)),
        None => mem(b),
    });
    let restarts = rt.restarts_per_day.map_or_else(
        || "not counted yet: under an hour since the rollout".to_owned(),
        per_day,
    );
    html! {
        div {
            p.small {
                strong { (icon("code")) " Runtime" } " · "
                @if let Some(r) = rss {
                    (part("Largest RSS", r, rt.previous_rss_max_bytes.map(mem), previous)) " · "
                }
                @if let Some(d) = rt.loop_delay_p99_ms {
                    (part("Event-loop delay p99", ms(d), rt.previous_loop_delay_p99_ms.map(ms), previous)) " · "
                }
                (part("Restarts", restarts, rt.previous_restarts_per_day.map(per_day), previous))
            }
            p.muted.small { "Memory and event-loop delay are shown, not judged: they follow uptime and load. Only restarts going up sharply (a crash loop) make a release regressed." }
        }
    }
}

/// The Release page's line when the release sent no runtime stats.
pub(crate) fn missing() -> Markup {
    html! {
        p.muted.small {
            "No runtime stats for this release: memory, event-loop delay and restarts show here once the Node SDK sends them. Its Express middleware does; another server calls "
            code { "ol.captureRuntime()" } " once at start-up."
        }
    }
}

/// The Releases list's line, compact; nothing without runtime stats.
pub(crate) fn compact_line(rt: Option<&RuntimeShown>) -> Markup {
    let Some(rt) = rt else {
        return html! {};
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = rt.rss_max_bytes {
        parts.push(match rt.rss_growth {
            Some(g) => format!("RSS {} {}", mem(b), growth(g)),
            None => format!("RSS {}", mem(b)),
        });
    }
    if let Some(d) = rt.loop_delay_p99_ms {
        parts.push(format!("loop p99 {}", ms(d)));
    }
    if let Some(r) = rt.restarts_per_day {
        parts.push(match rt.previous_restarts_per_day {
            Some(p) => format!("restarts {r:.1} a day (was {p:.1})"),
            None => format!("restarts {r:.1} a day"),
        });
    }
    html! {
        @if !parts.is_empty() {
            div.muted.small title="Runtime: memory and loop delay shown, not judged" { "Runtime: " (parts.join(" · ")) }
        }
    }
}


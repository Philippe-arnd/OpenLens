//! Key numbers (#543): one tile per KPI. The whole tile links to its page
//! (a plain card on a share link); it shows the value and, when the board
//! compares its window with the one before (D-055), a delta that carries
//! an icon and a sign besides its colour, and what it is compared with.
//! The definition (#405) opens from an ⓘ button as a native popover, next
//! to the link rather than inside it (no nested interactive content).

use crate::*;

/// How a number moved.
pub(super) struct Delta {
    /// `good`, `bad` or `flat`.
    pub(super) tone: &'static str,
    /// `up`, `down` or `right` (flat).
    pub(super) glyph: &'static str,
    /// The signed change: "+12%", "−0.4 pts".
    pub(super) text: String,
}

/// `d` with its sign, a real minus.
fn signed(d: f64, decimals: usize) -> String {
    format!("{d:+.decimals$}").replacen('-', "−", 1)
}

/// `now` against `before`. `points`: shares compared in percentage
/// points, else the relative change. `higher_good`: up is better. None
/// when there is nothing to compare with.
pub(super) fn delta(now: f64, before: f64, higher_good: bool, points: bool) -> Option<Delta> {
    if !now.is_finite() || !before.is_finite() {
        return None;
    }
    let (text, flat) = if points {
        let d = (now - before) * 100.0;
        let decimals = if d.abs() < 10.0 { 1 } else { 0 };
        (format!("{} pts", signed(d, decimals)), d.abs() < 0.5)
    } else {
        if before <= 0.0 {
            return None;
        }
        let d = (now - before) / before * 100.0;
        (format!("{}%", signed(d, 0)), d.abs() < 5.0)
    };
    if flat {
        return Some(Delta {
            tone: "flat",
            glyph: "right",
            text,
        });
    }
    let up = now > before;
    Some(Delta {
        tone: if up == higher_good { "good" } else { "bad" },
        glyph: if up { "up" } else { "down" },
        text,
    })
}

/// A delta: icon, signed change, and the verdict in words for screen
/// readers (the colour says it to the eye).
pub(super) fn delta_mark(d: &Delta) -> Markup {
    let word = match d.tone {
        "good" => "better",
        "bad" => "worse",
        _ => "about the same",
    };
    html! {
        span class={ "delta " (d.tone) } title=(word) {
            (icon(d.glyph)) (d.text) span.sr-only { " (" (word) ")" }
        }
    }
}

/// What a tile shows besides its label.
pub(super) struct Kpi {
    /// The value (a `small` may follow the number).
    pub(super) value: Markup,
    /// A small chart next to the value, or nothing.
    pub(super) spark: Markup,
    /// One line under the value.
    pub(super) sub: String,
    /// The delta and what it is compared with ("vs the 10 days before").
    pub(super) moved: Option<(Delta, String)>,
}

/// The ids of a tile's definition button and popover.
fn def_id(label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("kpi-{slug}")
}

/// A KPI tile that opens `href` (#409). It is marked `is-alert` when its
/// delta is bad and `alert` says that matters (sessions with errors).
pub(super) fn kpi_to(href: &str, label: &str, k: Kpi, alert: bool) -> Markup {
    let alerting = alert && k.moved.as_ref().is_some_and(|(d, _)| d.tone == "bad");
    let cls = if alerting {
        "card kpi is-alert"
    } else {
        "card kpi"
    };
    let id = def_id(label);
    let inner = html! {
        div.kpi-head { span { (label) } }
        div.kpi-mid { div.kpi-value { (k.value) } (k.spark) }
        @if !k.sub.is_empty() { p.kpi-sub { (k.sub) } }
        @if let Some((d, against)) = &k.moved {
            div.kpi-foot { (delta_mark(d)) span { (against) } }
        }
    };
    html! {
        div.kpi-slot {
            @if shared() { div class=(cls) { (inner) } } @else { a class=(cls) href=(href) { (inner) } }
            @if let Some(def) = crate::glossary::definition(label) {
                button.info type="button" id={ (id) "-btn" } popovertarget=(id) aria-label={ "Definition: " (label) } { (icon("info")) }
                div.kpi-pop popover="auto" id=(id) { strong { (label) } " " (def) }
            }
        }
    }
}

/// The active users line next to the value: the last days, as a
/// polyline (decorative: the chart card shows the same days).
fn spark(series: &[(String, i64)]) -> Markup {
    if series.len() < 2 {
        return html! {};
    }
    let max = series.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1) as f64;
    let step = 84.0 / (series.len() - 1) as f64;
    let points: Vec<String> = series
        .iter()
        .enumerate()
        .map(|(i, (_, n))| {
            format!(
                "{:.1},{:.1}",
                i as f64 * step,
                26.0 - *n as f64 / max * 24.0
            )
        })
        .collect();
    html! {
        svg.spark viewBox="0 0 84 28" aria-hidden="true" focusable="false" {
            polyline pathLength="1" points=(points.join(" ")) fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round" {}
        }
    }
}

/// The key numbers.
pub(super) fn key_numbers(pid: &str, f: &Filters, b: &Board) -> Markup {
    // The window and the one before, when the board compares them and
    // the earlier one had sessions.
    let before = b
        .window
        .as_ref()
        .filter(|w| w.previous.sessions.count > 0)
        .map(|w| {
            (
                &w.previous.sessions,
                format!("vs the {} before", super::days_before(w)),
            )
        });
    let now = &b.sessions;
    let period = |fixed: &str| super::period(b, fixed);
    let users = &b.users;
    let dau_sub = {
        let mut sub = String::new();
        if users.known > 0 {
            sub.push_str(&format!(
                "of {} known users (90 days) · ",
                compact(users.known)
            ));
        }
        sub.push_str(&format!(
            "{} weekly · {} monthly ({})",
            compact(users.wau),
            compact(users.mau),
            pct(users.mau_share())
        ));
        if users.resurrected > 0 {
            sub.push_str(&format!(" · {} came back", compact(users.resurrected)));
        }
        if users.anonymous > 0 {
            sub.push_str(&format!(" · +{} not logged in", compact(users.anonymous)));
        }
        sub
    };
    let moved = |now: f64, then: f64, up_good: bool| {
        let (_, against) = before.as_ref()?;
        Some((delta(now, then, up_good, false)?, against.clone()))
    };
    let prev = before.as_ref().map(|(s, _)| *s);
    html! {
        section.key-numbers aria-labelledby="kpi-h" {
            h2.eyebrow id="kpi-h" { "Key numbers" } // no_detail: each tile opens its page
            div.kpis {
                (kpi_to(&detail_url(pid, "/audience", f), "Daily active users", Kpi {
                    value: html! { (compact(users.dau)) @if users.known > 0 { small { (pct(users.dau_share())) } } },
                    spark: spark(&b.dau_series),
                    sub: dau_sub,
                    moved: None,
                }, false))
                (kpi_to(&detail_url(pid, "/audience", f), "Stickiness", Kpi {
                    value: html! { (pct(users.stickiness)) },
                    spark: html! {},
                    sub: "last 30 days".to_owned(),
                    moved: None,
                }, false))
                (kpi_to(&detail_url(pid, "/events", f), "Avg session", Kpi {
                    value: html! { (duration(now.avg_secs)) },
                    spark: html! {},
                    sub: format!("{} sessions · {} bounce", compact(now.count), pct(now.bounce_rate)),
                    moved: prev.and_then(|p| moved(now.avg_secs, p.avg_secs, true)),
                }, false))
                (kpi_to(&detail_url(pid, "/features", f), "Tasks per user per day", Kpi {
                    value: html! { (format!("{:.1}", now.tasks_per_user_day)) },
                    spark: html! {},
                    sub: format!("feature uses, {}", period("last 7 days")),
                    moved: prev.and_then(|p| moved(now.tasks_per_user_day, p.tasks_per_user_day, true)),
                }, false))
                (kpi_to(&detail_url(pid, "/errors", f), "Sessions with errors", Kpi {
                    value: html! { (pct(now.error_rate)) },
                    spark: html! {},
                    sub: period("last 7 days"),
                    moved: prev.and_then(|p| {
                        Some((delta(now.error_rate, p.error_rate, false, true)?, format!("was {}", pct(p.error_rate))))
                    }),
                }, true))
            }
        }
    }
}

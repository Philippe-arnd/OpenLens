//! The board's charts (#543): active users per day and the retention
//! line (the release comparison left for the Releases page, #620).
//! Every length is an SVG attribute and every shade a class (the CSP
//! forbids `style`).

use crate::*;

/// The day chart's drawing box (stretched to the card's width).
const CHART_W: f64 = 560.0;
const CHART_H: f64 = 140.0;

/// The height of a day's bar: proportional to its count, at least a
/// sliver, so a quiet day still shows where the baseline is.
fn bar_height(v: i64, max: f64) -> f64 {
    let h = v.max(0) as f64 / max.max(1.0) * (CHART_H - 2.0);
    h.max(if v > 0 { 2.0 } else { 1.0 })
}

/// Active users per day, as bars with the day and the count in each
/// bar's `<title>`, and the day numbers under them.
pub(super) fn dau_chart(series: &[(String, i64)]) -> Markup {
    let n = series.len().max(1) as f64;
    let max = series.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1) as f64;
    let slot = CHART_W / n;
    let bw = slot * 0.72;
    let label: Vec<String> = series
        .iter()
        .map(|(d, v)| format!("{d} {}", compact(*v)))
        .collect();
    html! {
        div.chart {
            svg.bars viewBox={ "0 0 " (CHART_W) " " (CHART_H) } preserveAspectRatio="none" role="img"
                aria-label={ "Daily active users: " (label.join(", ")) } {
                @for (i, (day, v)) in series.iter().enumerate() {
                    @let h = bar_height(*v, max);
                    rect class=(if i + 1 == series.len() { "b now" } else { "b" })
                        x=(format!("{:.1}", i as f64 * slot + (slot - bw) / 2.0)) y=(format!("{:.1}", CHART_H - h))
                        width=(format!("{bw:.1}")) height=(format!("{h:.1}")) rx="3" {
                        title { (day) ": " (compact(*v)) (if *v == 1 { " user" } else { " users" }) }
                    }
                }
            }
            div.axis aria-hidden="true" {
                @for (day, _) in series {
                    span { (day.get(8..).map_or(day.as_str(), |d| d.trim_start_matches('0'))) }
                }
            }
        }
    }
}

/// The active users card: the day chart, the changes of those days (the
/// timeline's marks), the window's users against the days before (#588:
/// no longer under the page title), and who uses the app.
pub(super) fn active_users(pid: &str, f: &Filters, b: &Board, marks: &[Mark]) -> Markup {
    let on: Vec<&Mark> = marks
        .iter()
        .filter(|m| b.dau_series.iter().any(|(d, _)| *d == m.day))
        .take(8)
        .collect();
    html! {
        section.card.pad.stack aria-labelledby="au-h" {
            div.section-head {
                h2 id="au-h" { (to(&detail_url(pid, "/audience", f), "Active users per day")) }
                @if !shared() && !on.is_empty() { a.small href={ "/p/" (pid) "/timeline" } { "Timeline" } }
            }
            (dau_chart(&b.dau_series))
            @if !on.is_empty() {
                div.markers {
                    @for m in &on {
                        @let kind = if m.kind == "alert" { "alert" } else if m.kind == "release" { "release" } else { "change" };
                        span class={ "marker " (kind) } { i aria-hidden="true" {} (m.day) ": " (m.text) }
                    }
                }
            }
            p.muted.small {
                @match &b.window {
                    Some(w) => { (w.label) ": " (super::window_users(w)) "." }
                    None => { "Last 14 days." }
                }
                " Hover a bar for the day."
            }
        }
    }
}

/// A share as a bar and its number (the bar's width is an SVG attribute).
pub(crate) fn barcell(frac: f64) -> Markup {
    bar_with(frac, &pct(frac))
}

/// A bar filled to `frac` (0..1) followed by `text` (a count, a share):
/// the width is an SVG attribute, as the CSP forbids inline styles.
pub(crate) fn bar_with(frac: f64, text: &str) -> Markup {
    let w = (frac.clamp(0.0, 1.0) * 100.0).max(1.5);
    html! {
        div.barcell {
            svg viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true" focusable="false" {
                rect.bg width="100" height="6" rx="3" {}
                rect.fg width=(format!("{w:.1}")) height="6" rx="3" {}
            }
            span.num { (text) }
        }
    }
}

/// A share's shade, in tenths (`p0`…`p10`): the cell's class.
pub(crate) fn tenth(share: f64) -> u8 {
    (share.clamp(0.0, 1.0) * 10.0).round() as u8
}

/// Do they come back (#619): one line, week-N return of the oldest and
/// the newest complete cohorts and which way it went (the engine picks
/// them), and the link to People › Retention, which holds the table.
pub(super) fn retention_line(pid: &str, f: &Filters, b: &Board) -> Markup {
    // "5 Oct": the week a cohort was first seen.
    let cohort = |w: &str| {
        chrono::NaiveDate::parse_from_str(w, "%Y-%m-%d")
            .map_or_else(|_| w.to_owned(), |d| d.format("%-d %b").to_string())
    };
    html! {
        section.card.pad.stack aria-labelledby="ret-h" {
            div.section-head {
                h2 id="ret-h" { (to(&detail_url(pid, "/retention", f), "Do they come back?")) }
            }
            @match &b.retention_trend {
                Some(t) => p {
                    "Week " (t.week) ": " strong { (pct(t.oldest.1)) } " of the " (cohort(&t.oldest.0)) " cohort, "
                    strong { (pct(t.newest.1)) } " of the " (cohort(&t.newest.0)) " cohort"
                    @match t.trend {
                        Some(openlens_engine::Trend::Falling) => { " " span.tag.tone-warn { (icon("down")) "Falling" } }
                        Some(openlens_engine::Trend::Rising) => { " " span.tag.tone-good { (icon("up")) "Rising" } }
                        None => { " " span.muted { "(no change)" } }
                    }
                },
                None => p.muted {
                    "Not enough complete cohorts yet: the line compares two weekly cohorts once the same week after their first has fully passed for both."
                },
            }
        }
    }
}

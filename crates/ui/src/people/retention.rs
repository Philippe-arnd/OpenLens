//! Retention (#619): the weekly cohorts, moved from the Focus board (which
//! keeps one line, `board::charts::retention_line`), and how long users
//! stay, drawn as a survival curve (it was a list of days on Churn).

use super::{card, head};
use crate::board::tenth;
use crate::*;

/// Weekly retention cohorts: users first seen each week (rows) and the
/// share of them active again N weeks later (columns). The share is
/// always printed, the shade only repeats it.
pub(crate) fn cohort_table(b: &Board) -> Markup {
    let rows: Vec<_> = b.retention.iter().filter(|r| r.users > 0).collect();
    let cols = rows.iter().map(|r| r.weeks.len()).max().unwrap_or(0);
    let tz = b.filters.as_ref().map_or("UTC", |f| f.tz.as_str());
    card(
        "coh-h",
        "Do they come back?",
        html! { span.muted.small { "Weekly cohorts, last 8 weeks" } },
        html! {
            @if rows.is_empty() || cols == 0 {
                p.muted { "No cohort has a week behind it yet: a cohort is the users first seen in one week, and it shows here once the next week has started." }
            } @else {
                div.scroll {
                    table.heat {
                        thead { tr {
                            th scope="col" { "First seen (week of)" }
                            (th_num("Users"))
                            @for k in 1..=cols { th.c scope="col" { "Week " (k) } }
                        } }
                        tbody { @for r in &rows {
                            tr {
                                th.mono scope="row" { (r.week) }
                                td.num.muted { (compact(r.users)) }
                                @for k in 0..cols {
                                    @if let Some(share) = r.weeks.get(k) {
                                        td.c { span class={ "hc p" (tenth(*share)) } { (pct(*share)) } }
                                    } @else {
                                        td {}
                                    }
                                }
                            }
                        } }
                    }
                }
            }
            p.muted.small { "The share of each week's new users active again N weeks later. The last week of each row is still running. Weeks start on Monday (" (tz) ")." }
        },
    )
}

/// The survival curve's drawing box and margins (room for the labels).
const KM_W: f64 = 560.0;
const KM_H: f64 = 180.0;
const KM_LEFT: f64 = 40.0;
const KM_RIGHT: f64 = 12.0;
const KM_TOP: f64 = 8.0;
const KM_BOTTOM: f64 = 22.0;

/// The share still active N days after the first visit (Kaplan–Meier), as
/// a step line: the share holds until the next measured day, then drops.
/// Every length is an SVG attribute (the CSP forbids `style`); the
/// `<desc>` lists every point for screen readers.
fn survival_curve(points: &[(i64, f64)]) -> Markup {
    let last = points.iter().map(|p| p.0).max().unwrap_or(0).max(1) as f64;
    let (pw, ph) = (KM_W - KM_LEFT - KM_RIGHT, KM_H - KM_TOP - KM_BOTTOM);
    let x = |d: i64| KM_LEFT + d.max(0) as f64 / last * pw;
    let y = |s: f64| KM_TOP + (1.0 - s.clamp(0.0, 1.0)) * ph;
    let path: String = points
        .iter()
        .enumerate()
        .map(|(i, (d, s))| {
            if i == 0 {
                format!("M{:.1} {:.1}", x(*d), y(*s))
            } else {
                format!(" H{:.1} V{:.1}", x(*d), y(*s))
            }
        })
        .collect();
    let desc: Vec<String> = points
        .iter()
        .map(|(d, s)| format!("day {d}: {}", pct(*s)))
        .collect();
    let (x0, x1) = (KM_LEFT, KM_W - KM_RIGHT);
    html! {
        svg.survival viewBox={ "0 0 " (KM_W) " " (KM_H) } role="img" aria-labelledby="km-t km-d" {
            title id="km-t" { "Share of users still active, by days since their first visit" }
            desc id="km-d" { (desc.join(", ")) }
            @for s in [1.0, 0.5, 0.0] {
                line.grid x1=(x0) x2=(x1) y1=(format!("{:.1}", y(s))) y2=(format!("{:.1}", y(s))) {}
                text x=(KM_LEFT - 6.0) y=(format!("{:.1}", y(s) + 4.0)) text-anchor="end" { (format!("{:.0}%", s * 100.0)) }
            }
            @for (d, _) in points.iter().filter(|p| p.0 % 30 == 0) {
                text x=(format!("{:.1}", x(*d))) y=(KM_H - 6.0) text-anchor="middle" { "day " (d) }
            }
            path.km d=(path) {}
            @for (d, s) in points {
                circle cx=(format!("{:.1}", x(*d))) cy=(format!("{:.1}", y(*s))) r="3" { title { "Day " (d) ": " (pct(*s)) " still active" } }
            }
        }
    }
}

/// How long users stay: the curve, and its key points in words.
fn how_long(c: &openlens_engine::ChurnReport) -> Markup {
    let at = |day: i64| c.survival.iter().find(|p| p.0 == day).map(|p| p.1);
    let key: Vec<(i64, f64)> = [7, 30, 90]
        .into_iter()
        .filter_map(|d| Some((d, at(d)?)))
        .collect();
    card(
        "stay-h",
        "How long they stay",
        html! {},
        html! {
            @if c.users == 0 || c.survival.is_empty() {
                p.muted { "Nobody to follow yet: the curve counts logged-in users of the last 90 days from their first visit." }
            } @else {
                (survival_curve(&c.survival))
                p {
                    @for (i, (d, s)) in key.iter().enumerate() {
                        @if i > 0 { " · " }
                        "after " (d) " days: " strong { (pct(*s)) }
                    }
                    @if !key.is_empty() { ". " }
                    "Half the users are gone " (c.median_days.map_or_else(|| "after more than 90 days".to_owned(), |d| format!("by day {d}"))) "."
                }
                p.muted.small { "Share still active N days after their first visit (Kaplan–Meier: users who joined recently count only for the days they could have stayed). A user has left after " (c.window) " days without using the app." }
            }
        },
    )
}

/// People › Retention (#619): do new users come back, week after week,
/// and how long do they stay.
pub fn retention(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    c: &openlens_engine::ChurnReport,
) -> Markup {
    layout_in(
        "Retention",
        Some(p),
        context_bar(p.id, b, f, "/retention", false),
        html! {
            div.board.people {
                (head("Retention", html! { "Whether new users come back, week after week, and how long they stay, in " strong { (f.environment) } ". The cohorts: users first seen in the last 8 weeks. The curve: logged-in users of the last 90 days." }))
                (cohort_table(b))
                (how_long(c))
                p.small { a href=(detail_url(p.id, "/churn", f)) { "Who leaves, and when →" } }
            }
        },
    )
}

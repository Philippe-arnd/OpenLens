//! The board's charts and tables (#543): active users per day, the
//! feature pulse, the cohorts, who uses what, and the release comparison.
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
/// timeline's marks), and who uses the app.
pub(super) fn active_users(pid: &str, f: &Filters, b: &Board, marks: &[Mark]) -> Markup {
    let on: Vec<&Mark> = marks
        .iter()
        .filter(|m| b.dau_series.iter().any(|(d, _)| *d == m.day))
        .take(8)
        .collect();
    let roles: Vec<(String, i64)> = b
        .roles
        .iter()
        .map(|r| {
            (
                r.role.clone().unwrap_or_else(|| "not identified".into()),
                r.users,
            )
        })
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
            p.muted.small { (b.window.as_ref().map_or("Last 14 days", |w| w.label.as_str())) ". Hover a bar for the day." }
            @if !roles.is_empty() {
                div.who {
                    span.muted { "Who uses the app, " (super::period(b, "last 30 days")) ":" }
                    @for (role, n) in &roles {
                        span { strong.num { (compact(*n)) } " " span.muted { (role) } }
                    }
                }
            }
        }
    }
}

/// A share as a bar and its number (the bar's width is an SVG attribute).
pub(super) fn barcell(frac: f64) -> Markup {
    let w = (frac.clamp(0.0, 1.0) * 100.0).max(1.5);
    html! {
        div.barcell {
            svg viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true" focusable="false" {
                rect.bg width="100" height="6" rx="3" {}
                rect.fg width=(format!("{w:.1}")) height="6" rx="3" {}
            }
            span.num { (pct(frac)) }
        }
    }
}

/// Feature pulse: for each used feature, how many of the active users
/// adopt it, how often and on how many days they come back to it, and how
/// many reach its success event; then the declared features nobody used.
pub(super) fn feature_pulse(pid: &str, f: &Filters, b: &Board) -> Markup {
    // Adoption is over the active users of the same period: the last 30
    // days (MAU), or the release window (D-055).
    let mau = b
        .window
        .as_ref()
        .map_or(b.users.mau, |w| w.current.users)
        .max(1) as f64;
    let ratio = |a: i64, n: i64| a as f64 / n.max(1) as f64;
    let used: Vec<_> = b.features.iter().filter(|x| x.users > 0).collect();
    let unused: Vec<_> = b
        .features
        .iter()
        .filter(|x| x.declared && x.users == 0)
        .collect();
    html! {
        section.card.pad.stack aria-labelledby="fp-h" {
            div.section-head { h2 id="fp-h" { (to(&detail_url(pid, "/features", f), "Feature pulse")) } }
            @if used.is_empty() {
                p.muted { "No feature has been used yet. Features come from " code { "data-feature" } " attributes and the feature map." }
            }
            @if !used.is_empty() || !unused.is_empty() {
                div.scroll {
                    table.pulse {
                        thead { tr {
                            th scope="col" { "Feature" }
                            th scope="col" { "Adoption" }
                            (th_num("Users"))
                            (th_num("Uses / user"))
                            (th_num("Days / user"))
                            (th_num("Completion"))
                            (th_num("Task time"))
                        } }
                        tbody {
                            @for x in &used {
                                tr {
                                    th scope="row" { (feature_link(pid, &x.key, x.name.as_deref().unwrap_or(&x.key), x.declared, f)) }
                                    td.adopt { (barcell((x.users as f64 / mau).min(1.0))) }
                                    td.num { (compact(x.users)) }
                                    td.num { (format!("{:.1}", ratio(x.uses, x.users))) }
                                    td.num { (format!("{:.1}", ratio(x.user_days, x.users))) }
                                    td.num {
                                        @if x.declared && x.completed_users > 0 { (pct(ratio(x.completed_users, x.users).min(1.0))) }
                                        @else { span.muted title="No success event declared or reached yet" { "—" } }
                                    }
                                    td.num {
                                        @if let Some(t) = b.task_times.iter().find(|t| t.feature == x.key) {
                                            (duration(t.median_secs)) br; span.muted.small { "p90 " (duration(t.p90_secs)) }
                                        } @else { span.muted { "—" } }
                                    }
                                }
                            }
                            @for x in &unused {
                                tr.dim {
                                    th scope="row" { (feature_link(pid, &x.key, x.name.as_deref().unwrap_or(&x.key), true, f)) }
                                    td colspan="6" { span.tag.tone-warn { (icon("ban")) "Never used, " (super::period(b, "last 30 days")) } }
                                }
                            }
                        }
                    }
                }
            }
            @if !used.is_empty() {
                p.muted.small { (b.window.as_ref().map_or("Last 30 days", |w| w.label.as_str())) ". Adoption is the share of active users (" (if b.window.is_some() { "in that period" } else { "MAU" }) ") who used the feature; completion is the share of them who reached its success event; task time runs from the first use to that event (within 30 minutes)." }
            }
            @if !unused.is_empty() {
                p.muted.small { "Never used: declared in the feature map, but nobody reached them. Hidden, broken, or not needed?" }
            }
        }
    }
}

/// A share's shade, in tenths (`p0`…`p10`): the cell's class.
pub(super) fn tenth(share: f64) -> u8 {
    (share.clamp(0.0, 1.0) * 10.0).round() as u8
}

/// Weekly retention cohorts: users first seen each week (rows) and the
/// share of them active again N weeks later (columns). A disclosure, open
/// by default; the share is always printed, the shade only repeats it.
pub(super) fn cohorts(pid: &str, f: &Filters, b: &Board) -> Markup {
    let rows: Vec<_> = b.retention.iter().filter(|r| r.users > 0).collect();
    let cols = rows.iter().map(|r| r.weeks.len()).max().unwrap_or(0);
    if rows.len() < 2 || cols == 0 {
        return html! {};
    }
    html! {
        details.card.disclose open {
            summary {
                h2 { "Do they come back?" } // no_detail: a disclosure; its link is in the body
                span.muted.small { "Weekly cohorts: the share of each week's new users active again N weeks later" }
                (icon("chev"))
            }
            div.body {
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
                p.muted.small {
                    "Weeks start on Monday (" (b.filters.as_ref().map_or("UTC", |f| f.tz.as_str())) ")."
                    @if !shared() { " " a href=(detail_url(pid, "/churn", f)) { "Who leaves, and when →" } }
                }
            }
        }
    }
}

/// Roles × features: the share of each role's users who used each
/// feature in the last 30 days. One hue, darker = more adoption; the number
/// is always printed, so colour is never the only cue.
pub(super) fn role_matrix(pid: &str, f: &Filters, b: &Board) -> Markup {
    let roles: Vec<(&str, i64)> = b
        .roles
        .iter()
        .filter_map(|r| Some((r.role.as_deref()?, r.users)))
        .filter(|(_, n)| *n > 0)
        .take(6)
        .collect();
    let feats: Vec<&openlens_engine::FeatureUse> =
        b.features.iter().filter(|f| f.users > 0).take(10).collect();
    if roles.len() < 2 || feats.is_empty() {
        return html! {};
    }
    let cell = |role: &str, feat: &str| {
        b.role_features
            .iter()
            .find(|(r, f, _)| r == role && f == feat)
            .map_or(0, |x| x.2)
    };
    let flt = f;
    html! {
        section.card.pad.stack aria-labelledby="who-h" {
            div.section-head { h2 id="who-h" { (to(&detail_url(pid, "/features", f), "Who uses what")) } }
            div.scroll {
                table.heat {
                    thead { tr { th scope="col" { "Feature" } @for (r, n) in &roles { th.c scope="col" { (r) br; span.muted.small { (compact(*n)) " users" } } } } }
                    tbody { @for f in &feats {
                        tr {
                            th scope="row" { (feature_link(pid, &f.key, f.name.as_deref().unwrap_or(&f.key), f.declared, flt)) }
                            @for (r, n) in &roles {
                                @let share = cell(r, &f.key) as f64 / (*n).max(1) as f64;
                                td.c title={ (pct(share)) " of " (r) " users used " (f.name.as_deref().unwrap_or(&f.key)) } {
                                    span class={ "hc p" (tenth(share)) } { @if share > 0.0 { (pct(share)) } @else { "—" } }
                                }
                            }
                        }
                    } }
                }
            }
            p.muted.small { "Share of each role's users who used the feature, " (super::period(b, "last 30 days")) ". Empty cells are features a role never touches." }
        }
    }
}

/// One compared metric: label, new value, old value, whether higher is
/// better, and how to format it.
type Compared = (&'static str, f64, f64, bool, fn(f64) -> String);

/// The latest release against the one before, metric by metric, when
/// both have at least 8 sessions.
fn compared(rel: &[openlens_engine::ReleaseRow]) -> Option<[Compared; 4]> {
    let [new, old, ..] = rel else { return None };
    if new.sessions < 8 || old.sessions < 8 {
        return None;
    }
    let rows: [Compared; 4] = [
        ("Avg session", new.avg_secs, old.avg_secs, true, duration),
        (
            "Tasks / session",
            new.tasks_per_session,
            old.tasks_per_session,
            true,
            |v| format!("{v:.2}"),
        ),
        ("Bounce", new.bounce_rate, old.bounce_rate, false, pct),
        (
            "Sessions with errors",
            new.error_rate,
            old.error_rate,
            false,
            pct,
        ),
    ];
    Some(rows)
}

/// The releases: a closed disclosure whose summary counts what got worse
/// and better on the latest release, holding the metric deltas (arrow,
/// sign and colour, and the earlier value) and the release table.
pub(super) fn releases(pid: &str, f: &Filters, b: &Board) -> Markup {
    let rel = &b.releases;
    let rows = compared(rel);
    let deltas: Vec<(Compared, Option<super::tiles::Delta>)> = rows
        .iter()
        .flatten()
        .map(|c| (*c, super::tiles::delta(c.1, c.2, c.3, false)))
        .collect();
    let count = |tone: &str| {
        deltas
            .iter()
            .filter(|(_, d)| d.as_ref().is_some_and(|d| d.tone == tone))
            .count()
    };
    let (worse, better) = (count("bad"), count("good"));
    html! {
        details.card.disclose {
            summary {
                @if let [new, old, ..] = rel.as_slice() {
                    h2 { (new.release) " compared with " (old.release) } // no_detail: a disclosure; its links are in the body
                } @else {
                    h2 { "Releases" } // no_detail: a disclosure; its links are in the body
                }
                @if worse > 0 { span.tag.tone-bad { (icon("down")) (worse) " worse" } }
                @if better > 0 { span.tag.tone-good { (icon("up")) (better) " better" } }
                (icon("chev"))
            }
            div.body {
                @if rel.is_empty() {
                    p.muted { "No release tagged yet. Pass " code { "release" } " to the SDK to compare releases." }
                } @else {
                    @if deltas.is_empty() {
                        p.muted.small { "Comparison appears once both of the latest releases have at least 8 sessions." }
                    } @else {
                        ul.rel-deltas {
                            @for ((label, n, o, _, fmt), d) in &deltas {
                                li {
                                    span.muted { (label) }
                                    strong { (fmt(*n)) }
                                    @if let Some(d) = d { (super::tiles::delta_mark(d)) }
                                    span.muted { "was " (fmt(*o)) }
                                }
                            }
                        }
                    }
                    div.scroll {
                        table {
                            thead { tr { th scope="col" { "Release" } (th_num("Users")) (th_num("Sessions")) (th_num("Avg session")) (th_num("Bounce")) (th_num("Tasks / session")) (th_num("Sessions with errors")) th scope="col" { "First seen" } } }
                            tbody { @for (i, r) in rel.iter().enumerate() {
                                tr {
                                    th scope="row" { (row_link(&detail_url(pid, "", &Filters { release: Some(r.release.clone()), ..f.clone() }), &r.release)) @if i == 0 { " " span.tag.tone-accent { "Latest" } } }
                                    td.num { (compact(r.users)) }
                                    td.num { (compact(r.sessions)) }
                                    td.num { (duration(r.avg_secs)) }
                                    td.num { (pct(r.bounce_rate)) }
                                    td.num { (format!("{:.2}", r.tasks_per_session)) }
                                    td.num { (pct(r.error_rate)) }
                                    td.muted { (local_time(&r.first_seen, &f.tz, false)) }
                                }
                            } }
                        }
                    }
                }
                @if !shared() { p.small { a href=(detail_url(pid, "/timeline", f)) { "Releases on the timeline →" } } }
            }
        }
    }
}

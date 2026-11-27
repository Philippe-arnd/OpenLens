//! The board's Features card (#618): one table instead of the feature
//! pulse, who uses what and the unused features. Per feature: adoption,
//! users, completion, uses per user, a mini bar per role and the signals
//! the engine derives from the insights (`Board::feature_rows`). A column
//! whose value is the same on every row is left out. Every length is an
//! SVG attribute (the CSP forbids `style`).

use crate::*;
use openlens_engine::FeatureUse;

/// The mini bars' height, and each bar's width and step.
const BAR_H: f64 = 22.0;
const BAR_W: f64 = 10.0;
const BAR_STEP: f64 = 13.0;

/// Why a feature shows no completion or task time: its success event fires
/// on entry (#612).
const ON_ENTRY: &str = "Every user reaches the success event: it probably fires on entry, so there is no task to time. See Map health.";

/// The map health finding for a success event that fires on entry.
fn on_entry_finding(pid: &str, key: &str) -> String {
    format!(
        "/p/{pid}/features#{}",
        crate::features::finding_anchor("success_always", Some(key))
    )
}

/// One column that can be left out: its header, and per row the cell and
/// the value compared across rows.
struct Col {
    head: Markup,
    cells: Vec<(Markup, String)>,
}

impl Col {
    /// Worth a column: its value differs from one row to another.
    fn varies(&self) -> bool {
        let mut keys = self.cells.iter().map(|(_, k)| k);
        let first = keys.next();
        self.cells.len() < 2 || keys.any(|k| Some(k) != first)
    }
}

/// The roles drawn as mini bars: the identified ones, most users first
/// (at most three). Fewer than two say nothing adoption does not.
fn bar_roles(b: &Board) -> Vec<(&str, i64)> {
    let roles: Vec<(&str, i64)> = b
        .roles
        .iter()
        .filter_map(|r| Some((r.role.as_deref()?, r.users)))
        .filter(|(r, n)| *n > 0 && !r.starts_with("unmatched:"))
        .take(3)
        .collect();
    if roles.len() < 2 { Vec::new() } else { roles }
}

/// The mini bars of one feature: one bar per role, its height the share
/// of the role's users who used it; a role at 0% is a red dash. The
/// shares are the SVG's text alternative.
fn role_bars(b: &Board, roles: &[(&str, i64)], x: &FeatureUse) -> (Markup, String) {
    let share = |role: &str, n: i64| {
        b.role_features
            .iter()
            .find(|(r, f, _)| r == role && *f == x.key)
            .map_or(0.0, |c| c.2 as f64 / n.max(1) as f64)
            .min(1.0)
    };
    let shares: Vec<(&str, f64)> = roles.iter().map(|(r, n)| (*r, share(r, *n))).collect();
    let label = shares
        .iter()
        .map(|(r, s)| format!("{r} {}", pct(*s)))
        .collect::<Vec<_>>()
        .join(", ");
    let w = BAR_STEP * shares.len() as f64 - (BAR_STEP - BAR_W);
    let svg = html! {
        svg.rolebar width=(w) height=(BAR_H) viewBox={ "0 0 " (w) " " (BAR_H) } role="img" aria-label=(label) {
            title { (label) }
            @for (i, (_, s)) in shares.iter().enumerate() {
                @let h = if *s > 0.0 { (s * BAR_H).max(3.0) } else { 2.0 };
                rect class=[(*s <= 0.0).then_some("z")] x=(i as f64 * BAR_STEP) y=(format!("{:.1}", BAR_H - h))
                    width=(BAR_W) height=(format!("{h:.1}")) rx="2" {}
            }
        }
    };
    (html! { td { (svg) } }, label)
}

/// The Completion cell (#612: "—" and the Map health finding when the
/// success event fires on entry, never a task time of 0).
fn completion(pid: &str, b: &Board, x: &FeatureUse) -> (Markup, String) {
    if x.users > 0 && x.success_on_entry() {
        let cell = html! {
            td.num {
                @if shared() { span.muted title=(ON_ENTRY) { "—" } }
                @else { a.muted href=(on_entry_finding(pid, &x.key)) title=(ON_ENTRY) { "—" } }
            }
        };
        return (cell, "on entry".into());
    }
    if x.declared && x.completed_users > 0 {
        let share = pct((x.completed_users as f64 / x.users.max(1) as f64).min(1.0));
        let cell = html! {
            td.num title=[b.task_time(&x.key).map(|t| format!("Task time: median {}, p90 {}", duration(t.median_secs), duration(t.p90_secs)))] {
                (share)
            }
        };
        return (cell, share);
    }
    let why = if x.users == 0 {
        "Nobody used it"
    } else {
        "No success event declared or reached yet"
    };
    (
        html! { td.num { span.muted title=(why) { "—" } } },
        "—".into(),
    )
}

/// The Features card (#618).
pub(super) fn features(pid: &str, f: &Filters, b: &Board) -> Markup {
    // Adoption is over the active users of the same period: the last 30
    // days (MAU), or the release window (D-055).
    let active = b
        .window
        .as_ref()
        .map_or(b.users.mau, |w| w.current.users)
        .max(1);
    let rows = b.feature_rows();
    let roles = bar_roles(b);
    let mut cols = vec![
        Col {
            head: th_def("Adoption"),
            cells: Vec::new(),
        },
        Col {
            head: th_num("Users"),
            cells: Vec::new(),
        },
        Col {
            head: th_num("Completion"),
            cells: Vec::new(),
        },
        Col {
            head: th_num("Uses / user"),
            cells: Vec::new(),
        },
    ];
    let mut role_col = Col {
        head: html! { th scope="col" { (roles.iter().map(|r| r.0).collect::<Vec<_>>().join(" · ")) } },
        cells: Vec::new(),
    };
    for r in &rows {
        let x = r.feature;
        let adoption = (x.users as f64 / active as f64).min(1.0);
        let per_user = if x.users > 0 {
            format!("{:.1}", x.uses as f64 / x.users as f64)
        } else {
            "—".to_owned()
        };
        let days = x.user_days as f64 / x.users.max(1) as f64;
        let cells = [
            (
                html! { td.adopt title={ (compact(x.users)) (if x.users == 1 { " user" } else { " users" }) } { (super::charts::barcell(adoption)) } },
                pct(adoption),
            ),
            (html! { td.num { (compact(x.users)) } }, compact(x.users)),
            completion(pid, b, x),
            (
                html! { td.num title=[(x.users > 0).then(|| format!("On {days:.1} days per user"))] { (per_user) } },
                per_user.clone(),
            ),
        ];
        for (c, cell) in cols.iter_mut().zip(cells) {
            c.cells.push(cell);
        }
        if !roles.is_empty() {
            role_col.cells.push(role_bars(b, &roles, x));
        }
    }
    if !roles.is_empty() {
        cols.push(role_col);
    }
    let cols: Vec<Col> = cols.into_iter().filter(Col::varies).collect();
    let any_signal = rows.iter().any(|r| !r.signals.is_empty());
    html! {
        section.card.pad.stack aria-labelledby="feat-h" {
            div.section-head {
                h2 id="feat-h" { (to(&detail_url(pid, "/features", f), "Features")) }
                // What left the board (#620): the paths to a first success
                // on Feature flow; the roles × features matrix on the map.
                @if !shared() {
                    span.small {
                        a href=(detail_url(pid, "/flows?to=success", f)) { "Paths to success" }
                        " · "
                        a href=(detail_url(pid, "/features", f)) { "Feature map" }
                    }
                }
            }
            @if rows.is_empty() {
                p.muted { "No feature has been used yet. Features come from " code { "data-feature" } " attributes and the feature map." }
            } @else {
                div.scroll {
                    table.pulse {
                        thead { tr {
                            th scope="col" { "Feature" }
                            @for c in &cols { (c.head) }
                            @if any_signal { th scope="col" { "Signals" } }
                        } }
                        tbody {
                            @for (i, r) in rows.iter().enumerate() {
                                tr {
                                    th scope="row" { (feature_link(pid, &r.feature.key, r.feature.name.as_deref().unwrap_or(&r.feature.key), r.feature.declared, f)) }
                                    @for c in &cols { @if let Some((cell, _)) = c.cells.get(i) { (cell) } }
                                    @if any_signal {
                                        td { div.tag-list { @for s in &r.signals { span class={ "tag tone-" (s.tone) } { (s.label) } } } }
                                    }
                                }
                            }
                        }
                    }
                }
                p.muted.small {
                    "Adoption is the share of the " (compact(active)) " people active " (super::period(b, "in the last 30 days"))
                    " who used the feature at least once"
                    @if !roles.is_empty() { "; each role bar, the same share among that role's users" }
                    ". Signals repeat what the cards above found about a feature. A column that reads the same on every row is left out."
                }
            }
        }
    }
}

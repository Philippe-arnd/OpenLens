//! Outcomes (#616): five questions, one number each, and Revenue when
//! there is revenue. The whole tile links to its page (a plain card on a
//! share link); it shows the value and what it is compared with: a delta
//! that carries an icon and a sign besides its colour, a grade against
//! its reference, or nothing (no arrow) when there is no comparison. No
//! tile draws a sparkline: only active users have a daily series, and
//! the usage chart below shows it. The definition (#405) opens from an ⓘ
//! button as a native popover, next to the link rather than inside it
//! (no nested interactive content).

use crate::*;

/// How a number moved.
pub(crate) struct Delta {
    /// `good`, `bad` or `flat`.
    pub(crate) tone: &'static str,
    /// `up`, `down` or `right` (flat).
    pub(crate) glyph: &'static str,
    /// The signed change: "+12%", "−0.4 pts".
    pub(crate) text: String,
}

/// `d` with its sign, a real minus.
fn signed(d: f64, decimals: usize) -> String {
    format!("{d:+.decimals$}").replacen('-', "−", 1)
}

/// `now` against `before`. `points`: shares compared in percentage
/// points, else the relative change. `higher_good`: up is better. None
/// when there is nothing to compare with.
pub(crate) fn delta(now: f64, before: f64, higher_good: bool, points: bool) -> Option<Delta> {
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
pub(crate) fn delta_mark(d: &Delta) -> Markup {
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
pub(crate) struct Kpi {
    /// The value (a `small` may follow the number).
    pub(crate) value: Markup,
    /// One line under the value.
    pub(crate) sub: String,
    /// The delta and what it is compared with ("vs the 10 days before").
    pub(crate) moved: Option<(Delta, String)>,
    /// A grade instead of a delta (`good`, `needs-improvement`, `poor`)
    /// and its reference ("good ≤ 2.5 s").
    pub(crate) graded: Option<(String, String)>,
    /// A second line under the first (the Quality tile's server side).
    pub(crate) more: Option<String>,
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

/// A board tile that opens `href` (#409) and answers `question` (#616).
/// It is marked `is-alert` when its delta is bad and `alert` says that
/// matters (sessions with errors).
pub(super) fn kpi_to(href: &str, label: &str, question: &str, k: Kpi, alert: bool) -> Markup {
    kpi_card(Some(href), Some(question), label, k, alert)
}

/// A plain tile when `href` is none: a detail page's own numbers (People
/// screens, #550), which lead nowhere else.
pub(crate) fn kpi_tile(href: Option<&str>, label: &str, k: Kpi, alert: bool) -> Markup {
    kpi_card(href, None, label, k, alert)
}

fn kpi_card(
    href: Option<&str>,
    question: Option<&str>,
    label: &str,
    k: Kpi,
    alert: bool,
) -> Markup {
    let alerting = alert && k.moved.as_ref().is_some_and(|(d, _)| d.tone == "bad");
    let cls = if alerting {
        "card kpi is-alert"
    } else {
        "card kpi"
    };
    let id = def_id(label);
    let inner = html! {
        div.kpi-head { span { @if let Some(q) = question { span.eyebrow { (q) } } (label) } }
        div.kpi-value { (k.value) }
        @if !k.sub.is_empty() { p.kpi-sub { (k.sub) } }
        @if let Some(m) = &k.more { p.kpi-sub { (m) } }
        @if let Some((d, against)) = &k.moved {
            div.kpi-foot { (delta_mark(d)) span { (against) } }
        } @else if let Some((rating, against)) = &k.graded {
            div.kpi-foot { (status(rating)) span { (against) } }
        }
    };
    html! {
        div.kpi-slot {
            @match href.filter(|_| !shared()) {
                Some(href) => a class=(cls) href=(href) { (inner) },
                None => div class=(cls) { (inner) },
            }
            @if let Some(def) = crate::glossary::definition(label) {
                button.info type="button" id={ (id) "-btn" } popovertarget=(id) aria-label={ "Definition: " (label) } { (icon("info")) }
                div.kpi-pop popover="auto" id=(id) { strong { (label) } " " (def) }
            }
        }
    }
}

/// "no value yet": a tile with nothing to count says so instead of a
/// zero (rule 11).
fn none_yet() -> Markup {
    html! { "—" }
}

/// The previous release's row of the releases table, against the
/// window's release (or the release filter): the Value tile's
/// comparison, the unit the verdict and the Releases page use.
fn previous_release<'a>(b: &'a Board, f: &Filters) -> Option<&'a openlens_engine::ReleaseRow> {
    let current = b
        .window
        .as_ref()
        .and_then(|w| w.release.as_deref())
        .or(f.release.as_deref())?;
    let at = b.releases.iter().position(|r| r.release == current)?;
    b.releases.get(at + 1).filter(|r| r.sessions > 0)
}

/// The Reach line under the active users: known users, the role split
/// and today's active users so far (#613: never the headline).
fn reach_sub(b: &Board, f: &Filters, period: &str) -> String {
    let users = &b.users;
    let mut parts = vec![period.to_owned()];
    if users.known > 0 {
        parts.push(format!("of {} known", compact(users.known)));
    }
    // The split is of everyone: with a role filter it says nothing more.
    if f.role.is_none() {
        let roles: Vec<String> = b
            .roles
            .iter()
            .filter_map(|r| Some(format!("{}: {}", r.role.as_deref()?, compact(r.users))))
            .take(3)
            .collect();
        if !roles.is_empty() {
            parts.push(roles.join(", "));
        }
    }
    parts.push(format!("{} today, so far", compact(users.dau)));
    parts.join(" · ")
}

/// The Quality tile's server side (#623): the share of server requests
/// answered 5xx over the same window, or that there is none to read;
/// nothing when the board did not read them.
fn server_line(b: &Board) -> Option<String> {
    let s = b.server.as_ref()?;
    Some(match s.error_rate() {
        Some(r) => format!("{} of server requests answered 5xx", pct(r)),
        None => "No server request stats in this window".to_owned(),
    })
}

/// The outcomes.
pub(super) fn outcomes(
    pid: &str,
    f: &Filters,
    b: &Board,
    revenue: Option<&openlens_engine::Revenue>,
) -> Markup {
    // The window and the one before, when the board compares them and
    // the earlier one had sessions.
    let compared = b.window.as_ref().filter(|w| w.previous.sessions.count > 0);
    let against = compared.map(|w| format!("vs the {} before", super::days_before(w)));
    let now = &b.sessions;
    let period = |fixed: &str| super::period(b, fixed);
    let moved = |now: f64, then: f64, up_good: bool| {
        Some((delta(now, then, up_good, false)?, against.clone()?))
    };
    // Reach: the window's users (#613), else the last 30 days' (MAU).
    let reach = match &b.window {
        Some(w) => Kpi {
            value: html! { (compact(w.current.users)) },
            sub: reach_sub(b, f, &period("")),
            moved: compared
                .and_then(|p| moved(w.current.users as f64, p.previous.users as f64, true)),
            graded: None,
            more: None,
        },
        None => Kpi {
            value: html! { (compact(b.users.mau)) },
            sub: reach_sub(b, f, "last 30 days"),
            moved: None,
            graded: None,
            more: None,
        },
    };
    // Value: tasks per session against the previous release, else the
    // window before.
    let value = if now.count == 0 {
        Kpi {
            value: none_yet(),
            sub: format!("no session {}", period("in the last 7 days")),
            moved: None,
            graded: None,
            more: None,
        }
    } else {
        let vs = match previous_release(b, f) {
            Some(r) => delta(now.tasks_per_session, r.tasks_per_session, true, false)
                .map(|d| (d, format!("{}: {:.2}", r.release, r.tasks_per_session))),
            None => compared.and_then(|p| {
                moved(
                    now.tasks_per_session,
                    p.previous.sessions.tasks_per_session,
                    true,
                )
            }),
        };
        Kpi {
            value: html! { (format!("{:.2}", now.tasks_per_session)) },
            sub: format!("completed tasks, {}", period("last 7 days")),
            moved: vs,
            graded: None,
            more: None,
        }
    };
    // Retention: the Churn page's rate and median lifetime; no comparison.
    let retention = match b.churn.as_ref().and_then(|c| Some((c, c.rate()?))) {
        Some((c, rate)) => {
            let mut sub = format!(
                "{} of {} users left in {} days",
                compact(c.churned_recent),
                compact(c.active_at_start),
                c.window
            );
            if let Some(d) = c.median_days {
                sub.push_str(&format!(" · half are gone by day {d}"));
            }
            Kpi {
                value: html! { (pct(rate)) },
                sub,
                moved: None,
                graded: None,
                more: None,
            }
        }
        None => Kpi {
            value: none_yet(),
            sub: format!(
                "nobody was active {} days ago yet",
                b.churn.as_ref().map_or(14, |c| c.window)
            ),
            moved: None,
            graded: None,
            more: None,
        },
    };
    // Quality: sessions with errors against the window before.
    let quality = Kpi {
        value: if now.count == 0 {
            none_yet()
        } else {
            html! { (pct(now.error_rate)) }
        },
        sub: period("last 7 days"),
        moved: compared.and_then(|p| {
            let was = p.previous.sessions.error_rate;
            Some((
                delta(now.error_rate, was, false, true)?,
                format!("was {}", pct(was)),
            ))
        }),
        graded: None,
        more: server_line(b),
    };
    // Speed: LCP at p75, graded against its public threshold.
    let lcp = b.vitals.iter().find(|v| v.name == "LCP" && v.samples > 0);
    let good = openlens_engine::vitals_guide("LCP").map_or(2500.0, |g| g.good);
    let speed = match lcp {
        Some(v) => Kpi {
            value: html! { (format!("{:.1}", v.p75 / 1000.0)) small { "s" } },
            sub: format!("{} measurements, last 7 days", compact(v.samples)),
            moved: None,
            graded: Some((v.rating.clone(), format!("good ≤ {} s", good / 1000.0))),
            more: None,
        },
        None => Kpi {
            value: none_yet(),
            sub: "no measurement in the last 7 days: they arrive when visitors leave a page"
                .to_owned(),
            moved: None,
            graded: None,
            more: None,
        },
    };
    // Revenue: a sixth tile only when there is revenue.
    let money_tile = revenue.and_then(|r| {
        let m = r.totals.first()?;
        let mut sub = format!(
            "{} payers · {} per active user, last 30 days",
            compact(m.payers),
            money(m.per_active_user, &m.currency)
        );
        if r.totals.len() > 1 {
            sub.push_str(&format!(" · {} more currencies", r.totals.len() - 1));
        }
        Some(Kpi {
            value: html! { (money(m.total, &m.currency)) },
            sub,
            moved: None,
            graded: None,
            more: None,
        })
    });
    // Plans (#712, D-113): a tile only when the feature map declares plans.
    let plans_tile = b.plans.as_ref().map(|p| {
        let mut sub = format!("{} downgrades, last 30 days", compact(p.downgrades));
        if !p.gaps.is_empty() {
            sub.push_str(&format!(
                " · {} plan values the map does not match",
                p.gaps.len()
            ));
        }
        Kpi {
            value: html! { (compact(p.upgrades)) },
            sub,
            moved: delta(p.upgrades as f64, p.prev_upgrades as f64, true, false).map(|d| {
                (
                    d,
                    format!("was {} the 30 days before", compact(p.prev_upgrades)),
                )
            }),
            graded: None,
            more: None,
        }
    });
    html! {
        section.outcomes aria-labelledby="out-h" {
            h2.eyebrow id="out-h" { "Outcomes" } // no_detail: each tile opens its page
            div.kpis {
                (kpi_to(&detail_url(pid, "/audience", f), "Active users", "Reach", reach, false))
                (kpi_to(&detail_url(pid, "/features", f), "Tasks per session", "Value", value, false))
                (kpi_to(&detail_url(pid, "/churn", f), "Churn rate", "Retention", retention, false))
                (kpi_to(&detail_url(pid, "/errors", f), "Sessions with errors", "Quality", quality, true))
                (kpi_to(&detail_url(pid, "/pages", f), "Largest paint p75", "Speed", speed, false))
                @if let Some(k) = money_tile {
                    (kpi_to(&detail_url(pid, "/revenue", f), "Revenue", "Income", k, false))
                }
                @if let Some(k) = plans_tile {
                    (kpi_to(&detail_url(pid, "/audience", f), "Upgrades", "Plans", k, false))
                }
            }
        }
    }
}

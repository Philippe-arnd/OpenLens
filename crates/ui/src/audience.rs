//! Feature flow and uptime. Moved out of `lib.rs` as is (#307); the
//! People screens (Audience, Acquisition, Churn, Revenue) live in
//! `people` (#550).

use super::*;

/// Feature flow (#224): a Sankey of how sessions move between features,
/// drawn in SVG on the server, with a table view.
pub fn flows(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    flow: &openlens_engine::Flow,
    features: &[(String, String)],
    from: Option<&str>,
    to_success: bool,
) -> Markup {
    use openlens_engine::flows::{EXIT, SUCCESS};
    let label = |k: &str| -> String {
        features
            .iter()
            .find(|(key, _)| key == k)
            .map_or_else(|| k.to_owned(), |(_, n)| n.clone())
    };
    let cols = flow.steps.len().max(1);
    let (w, h, nw, gap) = (1000.0_f64, 440.0_f64, 12.0_f64, 10.0_f64);
    let total = flow
        .steps
        .iter()
        .map(|c| c.iter().map(|n| n.1).sum::<i64>())
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let scale = |col: &Vec<(String, i64)>| (h - gap * (col.len().saturating_sub(1)) as f64) / total;
    let k = flow.steps.iter().map(scale).fold(f64::INFINITY, f64::min);
    let x = |i: usize| {
        if cols == 1 {
            0.0
        } else {
            i as f64 * (w - nw - 150.0) / (cols - 1) as f64
        }
    };
    // Node boxes: (step, name) → (y, height).
    let mut boxes: std::collections::HashMap<(usize, String), (f64, f64)> =
        std::collections::HashMap::new();
    for (i, col) in flow.steps.iter().enumerate() {
        let mut y = 0.0;
        for (n, c) in col {
            let hh = (*c as f64 * k).max(2.0);
            boxes.insert((i, n.clone()), (y, hh));
            y += hh + gap;
        }
    }
    let mut out_off: std::collections::HashMap<(usize, String), f64> =
        std::collections::HashMap::new();
    let mut in_off: std::collections::HashMap<(usize, String), f64> =
        std::collections::HashMap::new();
    let mut ribbons: Vec<(String, String, String, i64, bool)> = Vec::new();
    for l in &flow.links {
        let (Some(a), Some(bx)) = (
            boxes.get(&(l.step, l.from.clone())),
            boxes.get(&(l.step + 1, l.to.clone())),
        ) else {
            continue;
        };
        let th = (l.sessions as f64 * k).max(1.0);
        let oa = out_off.entry((l.step, l.from.clone())).or_insert(0.0);
        let ob = in_off.entry((l.step + 1, l.to.clone())).or_insert(0.0);
        let (x0, x1) = (x(l.step) + nw, x(l.step + 1));
        let (y0, y1) = (a.0 + *oa, bx.0 + *ob);
        *oa += th;
        *ob += th;
        let xm = (x0 + x1) / 2.0;
        let d = format!(
            "M{x0:.1} {y0:.1}C{xm:.1} {y0:.1} {xm:.1} {y1:.1} {x1:.1} {y1:.1}L{x1:.1} {:.1}C{xm:.1} {:.1} {xm:.1} {:.1} {x0:.1} {:.1}Z",
            y1 + th,
            y1 + th,
            y0 + th,
            y0 + th
        );
        ribbons.push((d, l.from.clone(), l.to.clone(), l.sessions, l.to == EXIT));
    }
    let base = format!("/p/{}/flows", p.id);
    layout_in(
        "Feature flow",
        Some(p),
        context_bar(p.id, b, f, "/flows", false),
        html! {
            div.heading { div {
                h1 { "Feature flow" }
                p.muted { "How sessions move between features, last 30 days, " strong { (f.environment) } ": "
                    (compact(flow.sessions)) " sessions, " (compact(flow.succeeded)) " reached a success event. Repeats are merged; 5 steps at most; the 8 widest per step, the rest is Other." }
            } }
            nav.tabs aria-label="View" {
                a href=(base) aria-current=[(from.is_none() && !to_success).then_some("page")] { "From the start" }
                a href={ (base) "?to=success" } aria-current=[to_success.then_some("page")] { "Paths to success" }
            }
            form.filters method="get" action=(base) aria-label="After a feature" {
                label { "After the feature "
                    select name="from" {
                        option value="" { "—" }
                        @for (k, n) in features { option value=(k) selected[from == Some(k.as_str())] { (n) } }
                    }
                }
                input type="hidden" name="env" value=(f.environment);
                button type="submit" { "Show" }
            }
            section.card.wide {
                @if flow.sessions == 0 { p.muted { "No session used a feature in this window. Features come from the feature map and " code { "data-feature" } "." } }
                @else {
                    div.sankey-scroll {
                        svg.sankey viewBox={ "-2 -2 " (w + 4.0) " " (h + 4.0) } role="img" aria-label="Feature flow (the table below has the numbers)" {
                            @for (d, a, bn, n, drop) in &ribbons {
                                path class=(if *drop { "ribbon drop" } else { "ribbon" }) d=(d) { title { (label(a)) " → " (label(bn)) ": " (compact(*n)) " sessions" } }
                            }
                            @for (i, col) in flow.steps.iter().enumerate() {
                                @for (n, c) in col {
                                    @let (y, hh) = boxes.get(&(i, n.clone())).copied().unwrap_or((0.0, 0.0));
                                    @let step = format!("node s{}", i.min(4));
                                    @let cls = if n == SUCCESS { "node ok" } else if n == EXIT { "node exit" } else { step.as_str() };
                                    rect class=(cls) x=(format!("{:.1}", x(i))) y=(format!("{y:.1}")) width=(nw) height=(format!("{hh:.1}")) rx="2" { title { (label(n)) ": " (compact(*c)) } }
                                    @if hh >= 11.0 {
                                        text.slabel x=(format!("{:.1}", x(i) + nw + 5.0)) y=(format!("{:.1}", y + hh / 2.0 + 4.0)) { (label(n)) " · " (compact(*c)) }
                                    }
                                }
                            }
                        }
                    }
                    p.muted.small.flow-key {
                        span.key.step { "Feature (darker: later step)" } " "
                        span.key.ok { "Success event" } " "
                        span.key.exit { "Left the app" }
                    }
                }
            }
            @if flow.sessions > 0 {
                details.card.disclose.flow-table {
                    summary {
                        h2 { "As a table" }
                        span.muted.small { (flow.links.len()) " moves between steps" }
                        (icon("chev"))
                    }
                    div.body { div.scroll { table {
                        thead { tr { th.num scope="col" { "Step" } th scope="col" { "From" } th scope="col" { "To" } th.num scope="col" { "Sessions" } } }
                        tbody { @for l in &flow.links {
                            tr {
                                td.num { (l.step + 1) }
                                td { (label(&l.from)) }
                                td { @if l.to == EXIT { span.status.bad { "exit" } } @else if l.to == SUCCESS { span.status.good { (label(&l.to)) } } @else { (label(&l.to)) } }
                                td.num { (compact(l.sessions)) }
                            }
                        } }
                    } } }
                }
            }
        },
    )
}

/// One check as shown: (time, ok, status, ms, error).
pub type CheckRow = (String, bool, Option<i64>, Option<i64>, Option<String>);

/// Uptime (#256): the app's state, availability, response times and
/// incidents.
pub struct UptimeView<'a> {
    /// `up`, `down`, `maintenance`, `unknown` or `off`.
    pub state: &'a str,
    /// Since when (a time, in the reader's zone).
    pub since: Option<String>,
    /// The probed URL, when the probe is on.
    pub url: Option<String>,
    /// Availability over 24 h and 7 days (share of good checks).
    pub avail: (Option<f64>, Option<f64>),
    /// Planned maintenance left out of each figure, in words (#563).
    pub left_out: (Option<String>, Option<String>),
    /// The planned maintenance card (#563).
    pub maintenance: MaintenanceView,
    /// p50 and p95 response time over 24 h, ms.
    pub ms: (Option<i64>, Option<i64>),
    /// (rule, start, end, length in minutes, cause), newest first: one
    /// page of every incident (#559).
    pub incidents: Vec<(String, String, Option<String>, i64, String)>,
    /// The `before` of the next page of incidents, when there may be one.
    pub older: Option<i64>,
    /// Whether this page of incidents is not the first.
    pub paged: bool,
    /// The last checks, newest first: (time, ok, status, ms, error).
    pub last: Vec<CheckRow>,
    /// The last 90 UTC days, oldest first (#559).
    pub days: Vec<openlens_engine::commands::uptime::Day>,
    /// Availability over those days (share of good checks).
    pub days_avail: Option<f64>,
    /// The announcements section (#562), from [`notices`].
    pub notices: Option<Markup>,
    /// The availability card (#560), from [`availability_card`].
    pub availability: Option<Markup>,
    /// The Jobs card (#609).
    pub jobs: CheckInsView,
}

/// A day bar's tooltip: the day, its availability and incidents.
fn day_title(d: &openlens_engine::commands::uptime::Day) -> String {
    match (&d.stats, d.availability) {
        (Some(s), Some(a)) => {
            let mut t = format!("{} (UTC): {:.2}% available", d.day, a * 100.0);
            if s.incidents > 0 {
                let n = s.incidents;
                let mins = s.down_seconds / 60;
                t.push_str(&format!(
                    ", {n} incident{}, {mins} min down",
                    if n == 1 { "" } else { "s" }
                ));
            }
            t
        }
        _ => format!("{} (UTC): not checked", d.day),
    }
}

/// The Uptime page.
pub fn uptime(p: &ProjectInfo<'_>, v: &UptimeView<'_>) -> Markup {
    layout(
        "Uptime",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Uptime" }
                (crate::live_screens::uptime_status(v.state, v.since.as_deref()))
                @if let Some(u) = &v.url {
                    p.muted.small { "Checking " span.mono { (u) } " every minute. Down after two failed checks in a row (no answer, or HTTP 5xx)." }
                }
            } }
            @if v.url.is_none() {
                (crate::live_screens::app_address_step(p.id))
            } @else {
                // no_detail (#409): this page is the uptime detail already.
                section.tiles aria-label="Availability" {
                    (tile("mint", "task", "Last 24 hours", &v.avail.0.map_or_else(|| "—".into(), |a| format!("{:.2}%", a * 100.0)), &v.left_out.0.as_ref().map_or_else(|| "available".into(), |m| format!("available · {m} of planned maintenance not counted"))))
                    (tile("sky", "clock", "Last 7 days", &v.avail.1.map_or_else(|| "—".into(), |a| format!("{:.2}%", a * 100.0)), &v.left_out.1.as_ref().map_or_else(|| "available".into(), |m| format!("available · {m} of planned maintenance not counted"))))
                    (tile("violet", "bolt", "Response time", &v.ms.0.map_or_else(|| "—".into(), |m| format!("{m} ms")), &v.ms.1.map_or_else(|| "p50, last 24 hours".into(), |m| format!("p50 · p95 {m} ms"))))
                }
            }
            @if !v.days.is_empty() {
                section.card.wide {
                    h2 { (icon("pulse")) "Last 90 days" }
                    div.uptime-days role="img" aria-label=(format!("Daily availability over {} days; {}", v.days.len(), v.days_avail.map_or_else(|| "not checked yet".to_owned(), |a| format!("{:.2}% available", a * 100.0)))) {
                        @for d in &v.days { span class={ "uptime-day " (d.state) } title=(day_title(d)) {} }
                    }
                    p.muted.small {
                        @if let Some(a) = v.days_avail { strong { (format!("{:.2}%", a * 100.0)) } " available over the days checked. " }
                        "Green: no down time. Amber: down less than an hour. Red: down an hour or more. Grey: not checked. Days are UTC; hover a day for its numbers."
                    }
                }
            }
            (check_ins_card(p, &v.jobs))
            @if let Some(a) = &v.availability { (a) }
            (crate::maintenance::maintenance_card(p.id, &v.maintenance))
            @if let Some(n) = &v.notices { (n) }
            (crate::live_screens::incidents_table(p.id, &v.incidents, v.older, v.paged))
            @if !v.last.is_empty() {
                section.card.wide {
                    h2 { (icon("clock")) "Latest checks" }
                    ul.plain-list.small { @for (t, ok, status, ms, err) in v.last.iter().take(15) {
                        li { @if *ok { span.tag.tone-good { (icon("task")) "ok" } } @else { span.tag.tone-bad { (icon("alert")) "failed" } } " " (t)
                            @if let Some(s) = status { " · HTTP " (s) } @if let Some(m) = ms { " · " (m) " ms" } @if let Some(e) = err { " · " span.muted { (e) } } }
                    } }
                }
            }
        },
    )
}

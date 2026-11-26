//! Audience, flows, live, uptime, acquisition, churn and revenue.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// Who the users are (#225): a world map of users per country, then
/// devices, browsers, systems, languages and screens.
pub fn audience(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    a: &openlens_engine::Audience,
    geo: bool,
) -> Markup {
    let max = a.countries.iter().map(|c| c.1).max().unwrap_or(1).max(1) as f64;
    let name = |iso: &str| -> String {
        world::COUNTRIES
            .iter()
            .find(|c| c.0 == iso)
            .map_or_else(|| iso.to_owned(), |c| c.1.to_owned())
    };
    let users = a.users.max(1) as f64;
    // Five steps of one hue, on a square-root scale (a few big countries
    // would leave everyone else pale on a linear one).
    let step = |n: i64| -> u8 {
        if n <= 0 {
            0
        } else {
            1 + ((n as f64 / max).sqrt() * 4.0).min(4.0) as u8
        }
    };
    let (w, h) = world::SIZE;
    let list = |title: &str, glyph: &str, rows: &[(String, i64)], by: Option<&str>| {
        html! {
            section.card {
                h2.violet { (icon(glyph)) (title) }
                @if rows.is_empty() { p.muted.small { "Nothing yet." } }
                @else {
                    table.plain.bars { tbody { @for (v, n) in rows.iter().take(8) {
                        tr {
                            th scope="row" { (v) }
                            td.meter { (meter(*n as f64 / users)) }
                            td.num { (pct(*n as f64 / users)) }
                        }
                    } } }
                    @if let Some(by) = by { p.small { a href={ "/p/" (p.id) "/breakdown?by=" (by) "&env=" (url_enc(&f.environment)) } { "Break down by " (title.to_lowercase()) " →" } } }
                }
            }
        }
    };
    layout(
        "Audience",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Audience" }
                p.muted { (compact(a.users)) " users with a page view in the last 30 days, in " strong { (f.environment) } ". IP addresses are used for the country and never stored." }
            } }
            (filter_bar(p.id, b, f, "/audience"))
            section.card.wide {
                h2.violet { (icon("globe")) "Where they are" }
                @if !geo { p.small.muted { "The country database is not installed on this server (it ships in the Docker image; see the deploy guide), so countries are unknown." } }
                svg.world viewBox={ "0 0 " (w) " " (h) } role="img" aria-label="Users per country (the table below has the numbers)" {
                    @for (iso, cname, d) in world::COUNTRIES {
                        @let n = a.countries.iter().find(|c| c.0 == *iso).map_or(0, |c| c.1);
                        path class={ "c" (step(n)) } d=(d) { title { (cname) ": " (compact(n)) (if n == 1 { " user" } else { " users" }) } }
                    }
                }
                div.legend-scale aria-hidden="true" { span { "fewer" } @for i in 1..=5u8 { i class={ "c" (i) } {} } span { "more users" } }
                @if !a.countries.is_empty() {
                    details.table-view {
                        summary { "As a table" }
                        table.plain { thead { tr { th { "Country" } (th_num("Users")) (th_num("Share")) } }
                            tbody { @for (iso, n) in &a.countries { tr { th scope="row" { (name(iso)) " " span.muted.small { (iso) } } td.num { (compact(*n)) } td.num { (pct(*n as f64 / users)) } } } } }
                    }
                }
                p.muted.small { "IP geolocation by " a href="https://db-ip.com" { "DB-IP" } " (CC BY 4.0). Map outlines: Natural Earth." }
            }
            div.grid {
                (list("Devices", "board", &a.devices, Some("device")))
                (list("Browsers", "globe", &a.browsers, Some("browser")))
                (list("Systems", "layers", &a.os, Some("os")))
                (list("Languages", "idea", &a.languages, None))
                (list("Screens", "board", &a.screens, None))
            }
        },
    )
}

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
    layout(
        "Feature flow",
        Some(p),
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
            (filter_bar(p.id, b, f, "/flows"))
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
                                    @let cls = if n == SUCCESS { "node ok" } else if n == EXIT { "node exit" } else { "node" };
                                    rect class=(cls) x=(format!("{:.1}", x(i))) y=(format!("{y:.1}")) width=(nw) height=(format!("{hh:.1}")) rx="2" { title { (label(n)) ": " (compact(*c)) } }
                                    @if hh >= 11.0 {
                                        text.slabel x=(format!("{:.1}", x(i) + nw + 5.0)) y=(format!("{:.1}", y + hh / 2.0 + 4.0)) { (label(n)) " · " (compact(*c)) }
                                    }
                                }
                            }
                        }
                    }
                    details.table-view {
                        summary { "As a table" }
                        table.plain {
                            thead { tr { th.num { "Step" } th { "From" } th { "To" } (th_num("Sessions")) } }
                            tbody { @for l in &flow.links {
                                tr { td.num { (l.step + 1) } td { (label(&l.from)) } td { @if l.to == EXIT { span.status.bad { "exit" } } @else { (label(&l.to)) } } td.num { (compact(l.sessions)) } }
                            } }
                        }
                    }
                }
            }
        },
    )
}

/// Right now (#228): who is using the app, refreshed every 10 seconds by
/// the browser (a `Refresh` header: no script).
pub fn live(p: &ProjectInfo<'_>, f: &Filters, l: &openlens_engine::Live) -> Markup {
    let list = |title: &str, glyph: &str, rows: &[(String, i64)]| {
        html! {
            section.card {
                h2.violet { (icon(glyph)) (title) }
                @if rows.is_empty() { p.muted.small { "Nothing in the last 30 minutes." } }
                @else { table.plain { tbody { @for (v, n) in rows {
                    tr { th scope="row" { span.mono { (v) } } td.num { (compact(*n)) } }
                } } } }
            }
        }
    };
    layout(
        "Live",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Live" }
                p.muted { "Last 30 minutes in " strong { (f.environment) } ". Refreshes every 10 seconds; events show up within the flush delay (a few seconds)." }
            } }
            section.tiles aria-label="Right now" {
                // Each opens its detail with the filters kept (#409).
                (tile_to(&detail_url(p.id, "/events", f), "violet", "users", "Active now", &compact(l.users_5m), "users, last 5 minutes"))
                (tile_to(&detail_url(p.id, "/events", f), "sky", "clock", "Last 30 minutes", &compact(l.users_30m), "users"))
                (tile_to(&detail_url(p.id, "/errors", f), "coral", "alert", "Errors", &compact(l.errors_30m), "last 30 minutes"))
            }
            div.grid {
                (list("Releases", "release", &l.releases))
                (list("Roles", "users", &l.roles))
                (list("Features", "star", &l.features))
                (list("Pages", "route", &l.pages))
            }
            section.card.wide {
                h2.mint { (icon("events")) "Latest events" }
                @if l.events.is_empty() { p.muted { "Quiet." } }
                @else {
                    div.table-scroll { table.plain.events {
                        thead { tr { th { "When (" (tz_label(&f.tz)) ")" } th { "Event" } th { "User" } th { "Page" } th { "Release" } } }
                        tbody { @for r in &l.events {
                            @let fam = openlens_engine::family(&r.event);
                            tr {
                                td.muted.mono { (local_time(&r.timestamp, &f.tz, true)) }
                                td { span class={ "chip fam " (fam) } { (icon(family_icon(fam))) span.sr-only { (family_label(fam)) ": " } (r.event) } }
                                td.mono { a href={ "/p/" (p.id) "/people/" (url_enc(&r.distinct_id)) } { (r.distinct_id) } }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                            }
                        } }
                    } }
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
    /// `up`, `down`, `unknown` or `off`.
    pub state: &'a str,
    /// Since when (a time, in the reader's zone).
    pub since: Option<String>,
    /// The probed URL, when the probe is on.
    pub url: Option<String>,
    /// Availability over 24 h and 7 days (share of good checks).
    pub avail: (Option<f64>, Option<f64>),
    /// p50 and p95 response time over 24 h, ms.
    pub ms: (Option<i64>, Option<i64>),
    /// (rule, start, end, length in minutes, cause), newest first.
    pub incidents: Vec<(String, String, Option<String>, i64, String)>,
    /// The last checks, newest first: (time, ok, status, ms, error).
    pub last: Vec<CheckRow>,
    /// The announcements section (#562), from [`notices`].
    pub notices: Option<Markup>,
}

/// The Uptime page.
pub fn uptime(p: &ProjectInfo<'_>, v: &UptimeView<'_>) -> Markup {
    let (cls, word) = match v.state {
        "up" => ("good", "Up"),
        "down" => ("bad", "Down"),
        "off" => ("muted", "Not checked"),
        _ => ("muted", "Waiting for the first check"),
    };
    layout(
        "Uptime",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Uptime" }
                p { span class={ "status big " (cls) } { (word) } @if let Some(s) = &v.since { " since " (s) } }
            } }
            @if v.url.is_none() {
                section.card.wide { p { "Set the app address in " a href={ "/p/" (p.id) "/settings" } { "Settings" } " and OpenLens checks it every minute. Without it, only the traffic tells (after four weeks of history)." } }
            } @else {
                // no_detail (#409): this page is the uptime detail already.
                section.tiles aria-label="Availability" {
                    (tile("mint", "task", "Last 24 hours", &v.avail.0.map_or_else(|| "—".into(), |a| format!("{:.2}%", a * 100.0)), "available"))
                    (tile("sky", "clock", "Last 7 days", &v.avail.1.map_or_else(|| "—".into(), |a| format!("{:.2}%", a * 100.0)), "available"))
                    (tile("violet", "bolt", "Response time", &v.ms.0.map_or_else(|| "—".into(), |m| format!("{m} ms")), &v.ms.1.map_or_else(|| "p50, last 24 hours".into(), |m| format!("p50 · p95 {m} ms"))))
                }
                p.muted.small { "Checking " span.mono { (v.url.as_deref().unwrap_or("")) } " every minute. Down after two failed checks in a row (no answer, or HTTP 5xx)." }
            }
            @if let Some(n) = &v.notices { (n) }
            section.card.wide {
                h2.coral { (icon("alert")) "Incidents and alerts, last 30 days" }
                @if v.incidents.is_empty() { p.muted { "None." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "What" } th { "Started" } th { "Ended" } (th_num("Length")) th { "Cause" } } }
                        tbody { @for (rule, start, end, mins, cause) in &v.incidents {
                            tr {
                                td { span class={ "status " (if end.is_none() { "bad" } else { "muted" }) } { (match rule.as_str() {
                                    "traffic" => "traffic stopped",
                                    "down" => "down",
                                    r if r.starts_with("error:") => "new error",
                                    r if r.starts_with("env:") => "environment check",
                                    _ => "regression",
                                }) } }
                                td.small { (start) }
                                td.small { @match end { Some(e) => (e), None => strong { "ongoing" } } }
                                td.num { (mins) " min" }
                                td.small { (cause) }
                            }
                        } }
                    } }
                }
            }
            @if !v.last.is_empty() {
                section.card.wide {
                    h2 { (icon("clock")) "Latest checks" }
                    ul.plain-list.small { @for (t, ok, status, ms, err) in v.last.iter().take(15) {
                        li { span class={ "status " (if *ok { "good" } else { "bad" }) } { (if *ok { "ok" } else { "failed" }) } " " (t)
                            @if let Some(s) = status { " · HTTP " (s) } @if let Some(m) = ms { " · " (m) " ms" } @if let Some(e) = err { " · " span.muted { (e) } } }
                    } }
                }
            }
        },
    )
}

/// Where users come from (#226), ranked by the share reaching the value
/// moment, not by visits.
pub fn acquisition(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    rows: &[openlens_engine::AcquisitionRow],
    by: &str,
    last: bool,
) -> Markup {
    let base = format!("/p/{}/acquisition", p.id);
    let env = url_enc(&f.environment);
    let dims = [
        ("channel", "Channel"),
        ("source", "Source"),
        ("campaign", "Campaign"),
        ("referrer", "Referrer"),
    ];
    let total: i64 = rows.iter().map(|r| r.users).sum();
    layout(
        "Acquisition",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Where users come from" }
                p.muted { "Last 90 days, " strong { (f.environment) } ", by " (if last { "last touch (direct visits don't count)" } else { "first touch" }) ". Ranked by the share that reaches the value moment, not by visits." }
            } }
            nav.tabs aria-label="Group by" {
                @for (k, l) in dims {
                    a href={ (base) "?by=" (k) "&touch=" (if last { "last" } else { "first" }) "&env=" (env) } aria-current=[(k == by).then_some("page")] { (l) }
                }
                " · "
                a href={ (base) "?by=" (by) "&touch=first&env=" (env) } aria-current=[(!last).then_some("page")] { "First touch" }
                a href={ (base) "?by=" (by) "&touch=last&env=" (env) } aria-current=[last.then_some("page")] { "Last touch" }
            }
            (filter_bar(p.id, b, f, &format!("/acquisition?by={by}&touch={}", if last { "last" } else { "first" })))
            section.card.wide {
                @if rows.is_empty() { p.muted { "No landing with a channel yet. The SDK sends the referrer and the campaign parameters on the first page view of a visit (SDK builds from 2026-11-23 on)." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { (dims.iter().find(|d| d.0 == by).map_or("Channel", |d| d.1)) } (th_num("Users")) (th_num("Reached value")) th { span.sr-only { "Share" } } (th_num("Came back")) (th_num("Churned")) } }
                        tbody { @for r in rows {
                            tr {
                                th scope="row" { span.mono { (r.value) } @if r.users < 20 { " " span.chip title="Under 20 users: the rates move a lot" { "low sample" } } }
                                td.num { (compact(r.users)) span.muted.small { " · " (pct(r.users as f64 / total.max(1) as f64)) } }
                                td.num { strong { (pct(r.value_rate())) } }
                                td.meter { (meter(r.value_rate())) }
                                td.num { (pct(r.returned as f64 / r.users.max(1) as f64)) }
                                td.num { @if r.judged > 0 { (pct(r.churned as f64 / r.judged as f64)) } @else { span.muted { "—" } } }
                            }
                        } }
                    } }
                    p.muted.small { "Reached value: a success event of the feature map (any feature use when none is declared). Came back: active on two days or more. Churned: of the users first seen before the churn window, those inactive for longer than it now." }
                }
            }
        },
    )
}

/// A flag made from the users at risk, and how its arms do (#269).
pub struct Audience {
    /// Flag key.
    pub flag: String,
    /// Switched on.
    pub enabled: bool,
    /// Listed users who get it.
    pub treated: i64,
    /// Of them, active in the last 7 days.
    pub treated_active: i64,
    /// Listed users held out (control).
    pub held: i64,
    /// Of them, active in the last 7 days.
    pub held_active: i64,
}

/// What the churn page shows besides the report (#269).
pub struct ChurnExtras {
    /// Admins export and make audiences.
    pub admin: bool,
    /// The map's cancel event, if declared.
    pub churn_event: Option<String>,
    /// Answers of surveys shown on it.
    pub exit: Vec<openlens_engine::SurveyResults>,
    /// Flags made from the users at risk.
    pub audiences: Vec<Audience>,
}

/// Churn (#247): how many leave, after how long, at which moment, with
/// what friction, and who is at risk now; why they leave and who came
/// back (#269).
pub fn churn(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    c: &openlens_engine::ChurnReport,
    x: &ChurnExtras,
) -> Markup {
    let rate = c.churned_recent as f64 / c.active_at_start.max(1) as f64;
    let net = c.new_recent + c.resurrected - c.churned_recent;
    let leave_max = c.leave_week.iter().map(|x| x.1).max().unwrap_or(1).max(1) as f64;
    layout(
        "Churn",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Churn" }
                p.muted { "A user has churned after " strong { (c.window) " days" } " without using the app (" code { "churn_after_days" } " in the feature map). Last 90 days, " strong { (f.environment) } ", logged-in users." }
            } }
            @if let Some(s) = c.suggested {
                @if s != c.window {
                    p.notice { "Your users usually come back within " strong { (s) " days" } " (9 gaps out of 10 between active days are shorter): " code { "churn_after_days: " (s) } " would fit this app better. Nothing changes until the feature map says so." }
                }
            }
            (filter_bar(p.id, b, f, "/churn"))
            // The churn page is the detail (#409): only "At risk now" opens
            // its list below; the others stay plain on purpose (no_detail).
            section.tiles aria-label="How many" {
                (tile("coral", "ban", "Churn rate", &pct(rate), &format!("{} of {} users active {} days ago left", compact(c.churned_recent), compact(c.active_at_start), c.window)))
                (tile("mint", "users", "Net change", &format!("{}{}", if net >= 0 { "+" } else { "" }, compact(net)), &format!("{} new + {} back − {} gone", compact(c.new_recent), compact(c.resurrected), compact(c.churned_recent))))
                (tile_to("#at-risk", "sun", "alert", "At risk now", &compact(c.at_risk.len() as i64), "quiet for over half the window, less active than before"))
                (tile("violet", "clock", "Median lifetime", &c.median_days.map_or_else(|| "over 90 days".into(), |d| format!("{d} days")), "half the users are gone by then"))
            }
            div.grid {
                section.card {
                    h2.coral { (icon("ban")) "Churned per week" }
                    @if c.by_week.is_empty() { p.muted.small { "Nobody has churned in the window." } }
                    @else { (sparkline(&c.by_week)) p.muted.small { "Users whose last active day fell that week." } }
                }
                section.card {
                    h2.violet { (icon("clock")) "How long they stay" }
                    table.plain.bars { tbody { @for (d, s) in &c.survival {
                        tr { th scope="row" { "day " (d) } td.meter { (meter(*s)) } td.num { (pct(*s)) } }
                    } } }
                    p.muted.small { "Share still active N days after their first visit (Kaplan–Meier: users who joined recently count only for the days they could have stayed)." }
                }
                section.card {
                    h2.sun { (icon("route")) "When they leave" }
                    @if c.leave_week.is_empty() { p.muted.small { "Nobody has churned yet." } }
                    @else {
                        table.plain.bars { tbody { @for (wk, n) in &c.leave_week {
                            tr { th scope="row" { @if *wk >= 12 { "week 12+" } @else { "week " (wk + 1) } } td.meter { (meter(*n as f64 / leave_max)) } td.num { (compact(*n)) } }
                        } } }
                        p.muted.small { "Week of life in which churned users were last active. A spike in week 1 is onboarding; at the end of a trial, the trial." }
                    }
                }
                section.card {
                    h2.coral { (icon("alert")) "Friction before leaving" }
                    p { "On their last active day, " strong { (pct(c.errors_at_end.0)) } " of churned users hit an error, against " (pct(c.errors_at_end.1)) " of users who stayed." }
                    @if !c.friction.is_empty() {
                        div.table-scroll { table.plain {
                            thead { tr { th { "On the last day" } (th_num("Churned")) (th_num("Stayed")) } }
                            tbody { @for (k, ch, st) in &c.friction {
                                tr { th scope="row" { (k) } td.num { strong { (pct(*ch)) } } td.num { (pct(*st)) } }
                            } }
                        } }
                    }
                    p.small { a href={ "/p/" (p.id) "/errors" } { "Errors →" } " · " a href={ "/p/" (p.id) "/flows" } { "Feature flow (exits) →" } }
                }
            }
            section.card.wide {
                h2.coral { (icon("idea")) "Top reasons" }
                @if c.reasons.is_empty() { p.muted { "No clear reason yet: each comparison needs at least 8 users on both sides, and a gap of 5 points or more." } }
                @else {
                    ol { @for r in &c.reasons { li { strong { (r.title) } br; span.muted.small { (r.evidence) } } } }
                    p.muted.small { "Ranked by the gap between users who left and users who stayed. These are correlations: a feature people try early may be a sign of fit rather than its cause. Replay a churned session at the place named to check." }
                }
            }
            @if !c.lift.is_empty() {
                section.card.wide {
                    h2.mint { (icon("star")) "Features tried in the first 14 days" }
                    div.table-scroll { table.plain {
                        thead { tr { th { "Feature" } (th_num("Tried it")) (th_num("Churned")) (th_num("Did not")) (th_num("Churned")) } }
                        tbody { @for l in &c.lift {
                            tr {
                                th scope="row" { code { (l.feature) } }
                                td.num { (compact(l.adopters)) } td.num { strong { (pct(l.churn_adopters)) } }
                                td.num { (compact(l.non_adopters)) } td.num { (pct(l.churn_non)) }
                            }
                        } }
                    } }
                    p.muted.small { "Only users first seen 14 days ago or more, so everyone had the same time to try each feature (a user who stays longer has more time to try anything)." }
                }
            }
            div.grid {
                @if !c.by_release.is_empty() {
                    section.card {
                        h2.sky { (icon("release")) "By arrival release" }
                        table.plain {
                            thead { tr { th { "Release" } (th_num("Users")) (th_num("Churned")) } }
                            tbody { @for (r, n, s) in &c.by_release { tr { th scope="row" { code { (r) } } td.num { (compact(*n)) } td.num { (pct(*s)) } } } }
                        }
                    }
                }
                @if !c.by_role.is_empty() {
                    section.card {
                        h2.violet { (icon("users")) "By role" }
                        table.plain {
                            thead { tr { th { "Role" } (th_num("Users")) (th_num("Still here at 30 days")) } }
                            tbody { @for (r, n, s) in &c.by_role { tr { th scope="row" { (r) } td.num { (compact(*n)) } td.num { (pct(*s)) } } } }
                        }
                    }
                }
                section.card {
                    h2.mint { (icon("auto")) "Who came back" }
                    @if c.winback.users == 0 { p.muted { "Nobody came back after churning in the last 30 days." } }
                    @else {
                        p { strong { (c.winback.users) } " users came back in the last 30 days, active " (format!("{:.1}", c.winback.mean_active_days)) " days since on average." }
                        p.small { "On release: " @for (i, (r, n)) in c.winback.by_release.iter().enumerate() { @if i > 0 { ", " } code { (r) } " (" (n) ")" } }
                        p.small { "First used: " @for (i, (r, n)) in c.winback.by_feature.iter().enumerate() { @if i > 0 { ", " } code { (r) } " (" (n) ")" } }
                    }
                }
            }
            section.card.wide {
                h2.sun { (icon("events")) "Exit survey" }
                @match &x.churn_event {
                    None => p.muted { "Declare the app's cancel event in the feature map (" code { "churn_event: subscription_cancelled" } "): users who send it count as churned at once, and a survey triggered on it becomes the exit survey shown here." },
                    Some(ev) => {
                        p.small { code { (ev) } " · " (c.cancelled) " users cancelled in the window." }
                        @if x.exit.is_empty() { p.muted { "No survey is triggered on " code { (ev) } " yet: add one on the Surveys page to ask why they leave." } }
                        @for s in &x.exit {
                            p { strong { (s.key) } }
                            ul.plain-list.small {
                                @for (a, n) in &s.answers { li { (a) " · " (n) } }
                                @for t in &s.latest_text { li.muted { "“" (t) "”" } }
                            }
                        }
                    }
                }
            }
            section.card.wide {
                h2.violet { (icon("layers")) "The last thing they did" }
                @if c.last_place.is_empty() { p.muted { "Nobody has churned in the window." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "Feature or page" } (th_num("Churned users")) (th_num("Share")) (th_num("Users who stayed")) th { span.sr-only { "Actions" } } } }
                        tbody { @for (place, n, share, base) in &c.last_place {
                            tr {
                                th scope="row" { span.mono { (place) } }
                                td.num { (compact(*n)) }
                                td.num { strong { (pct(*share)) } }
                                td.num { (pct(*base)) }
                                td { @if *share > base * 1.5 && *n >= 3 { span.status.bad { "more than usual" } } }
                            }
                        } }
                    } }
                    p.muted.small { "Where churned users were last active, against where users who stayed were last seen. A place far above its usual share is where to look (and replay)." }
                }
            }
            section.card.wide #at-risk {
                h2.sun { (icon("users")) "At risk now" }
                @if c.at_risk.is_empty() { p.muted { "Nobody: active users keep their pace." } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "User" } th { "Role" } th { "Last active" } (th_num("Quiet for")) (th_num("Active days, last window")) (th_num("Window before")) } }
                        tbody { @for r in &c.at_risk {
                            tr {
                                td.mono { a href={ "/p/" (p.id) "/people/" (url_enc(&r.distinct_id)) } { (r.distinct_id) } }
                                td { (r.role.as_deref().unwrap_or("—")) }
                                td.small { (r.last_day) }
                                td.num { (r.idle_days) " days" }
                                td.num { (r.recent_days) }
                                td.num { (r.before_days) }
                            }
                        } }
                    } }
                    p.muted.small { "Open a profile to see their last sessions and replays. Win them back before the window closes." }
                    @if x.admin {
                        p.small {
                            a.button href={ "/p/" (p.id) "/churn/at-risk.csv" } { "CSV" } " "
                            a.button href={ "/p/" (p.id) "/churn/at-risk.json" } { "JSON" }
                        }
                        form.inline method="post" action={ "/p/" (p.id) "/churn/audience" } {
                            label { "Make a flag for them, holding out " input.narrow type="number" name="holdout" min="0" max="50" value="20"; " % as a control group " }
                            button.soft type="submit" { "Make the flag" }
                        }
                        p.muted.small { "The flag starts off and lists these users (frozen now). Put the nudge behind " code { "openlens.isEnabled(key)" } " and switch it on; the held-out users never get it, so you can compare." }
                    }
                }
            }
            @if !x.audiences.is_empty() {
                section.card.wide {
                    h2.violet { (icon("flag")) "Win-back flags" }
                    div.table-scroll { table.plain {
                        thead { tr { th { "Flag" } (th_num("Nudged")) (th_num("Active, last 7 days")) (th_num("Held out")) (th_num("Active, last 7 days")) } }
                        tbody { @for a in &x.audiences {
                            tr {
                                th scope="row" { a href={ "/p/" (p.id) "/flags-edit?key=" (a.flag) } { code { (a.flag) } } @if !a.enabled { " " span.chip { "off" } } }
                                td.num { (a.treated) } td.num { strong { (pct(a.treated_active as f64 / a.treated.max(1) as f64)) } }
                                td.num { (a.held) } td.num { (pct(a.held_active as f64 / a.held.max(1) as f64)) }
                            }
                        } }
                    } }
                }
            }
        },
    )
}

/// An amount with its currency, rounded to the unit above 100.
pub fn money(v: f64, cur: &str) -> String {
    let n = if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    };
    if cur == "?" { n } else { format!("{n} {cur}") }
}

/// Revenue on success events (#231).
pub fn revenue(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    r: Option<&openlens_engine::Revenue>,
) -> Markup {
    let table = |title: &str, glyph: &str, rows: &[(String, String, f64)]| {
        html! {
            section.card {
                h2.mint { (icon(glyph)) (title) }
                table.plain { tbody { @for (k, c, v) in rows {
                    tr { th scope="row" { span.mono { (k) } } td.num { (money(*v, c)) } }
                } } }
            }
        }
    };
    layout(
        "Revenue",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Revenue" }
                p.muted { "Amounts carried by success events (" code { "revenue" } " or " code { "value" } ", with an optional " code { "currency" } "), last 30 days, " strong { (f.environment) } "." }
            } }
            (filter_bar(p.id, b, f, "/revenue"))
            @match r {
                None => section.card.wide { p { "No success event carries an amount yet. Send one with the event, for example " code { "openlens.track(\"order_paid\", { revenue: 49, currency: \"EUR\" })" } ", and declare it as a feature's success event." } },
                Some(r) => {
                    // no_detail (#409): the breakdowns below are the detail.
                    section.tiles aria-label="Totals" {
                        @for m in &r.totals {
                            (tile("mint", "star", &format!("Revenue ({})", m.currency), &money(m.total, &m.currency), &format!("{} orders · {} payers · {} per active user", compact(m.orders), compact(m.payers), money(m.per_active_user, &m.currency))))
                        }
                    }
                    @if r.totals.len() > 1 { p.muted.small { "Currencies are not summed. Set " code { "revenue: { currency: EUR, rates: { USD: 0.92 } }" } " in the feature map to show one total." } }
                    @if r.converted { p.muted.small { "Converted into the display currency with the fixed rates of the feature map." } }
                    div.grid {
                        (table("By feature", "layers", &r.by_feature))
                        (table("By role", "users", &r.by_role))
                        (table("By release", "release", &r.by_release))
                    }
                }
            }
        },
    )
}

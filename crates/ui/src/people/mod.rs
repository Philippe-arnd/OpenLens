//! The People screens (#550): Audience, Retention (#619), Churn,
//! Acquisition and Revenue,
//! built from the board's v2 parts (KPI tiles, bar cells, the lead card,
//! table cards and disclosures). Moved out of `audience.rs`, which keeps
//! Feature flow and Uptime.

use crate::board::{Kpi, barcell, kpi_tile};
use crate::tables::{Col, TableCard};
use crate::*;

mod accounts;
mod churn;
mod retention;
pub use churn::*;
pub use retention::*;

#[cfg(test)]
mod tests;

/// A People page's head: its title and one line under it.
fn head(title: &str, line: Markup) -> Markup {
    html! {
        div.page-head { div { h1 { (title) } p { (line) } } }
    }
}

/// The key numbers of a page, as the board's tiles.
fn key_numbers(tiles: Markup) -> Markup {
    html! {
        section.key-numbers aria-labelledby="kpi-h" {
            h2.eyebrow id="kpi-h" { "Key numbers" }
            div.kpis { (tiles) }
        }
    }
}

/// A tile that leads nowhere else: the page is its detail.
fn plain_tile(label: &str, value: Markup, sub: String) -> Markup {
    kpi_tile(
        None,
        label,
        Kpi {
            value,
            graded: None,
            more: None,
            sub,
            moved: None,
        },
        false,
    )
}

/// Rows of (value, count) as bar cells, each the share of `of`, with the
/// count after it.
fn share_rows<'a>(rows: impl IntoIterator<Item = &'a (String, i64)>, of: f64) -> Markup {
    let of = of.max(1.0);
    html! {
        div.scroll { table.share-bars { tbody { @for (v, n) in rows {
            tr {
                th scope="row" { (v) }
                td.bar { (barcell(*n as f64 / of)) }
                td.num.muted { (compact(*n)) }
            }
        } } } }
    }
}

/// A card: a title (`id` names it) and a body.
fn card(id: &str, title: &str, aside: Markup, body: Markup) -> Markup {
    html! {
        section.card.pad.stack aria-labelledby=(id) {
            div.section-head { h2 id=(id) { (title) } (aside) }
            (body)
        }
    }
}

/// Who the users are (#225): the key numbers and a world map of users
/// per country, then devices, browsers, systems, languages and screens,
/// and the accounts' health when the app sends `account` (#719).
pub fn audience(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    a: &openlens_engine::Audience,
    geo: bool,
    accounts: Option<&openlens_engine::accounts::Accounts>,
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
    let env = url_enc(&f.environment);
    let list = |id: &str, title: &str, rows: &[(String, i64)], by: Option<&str>| {
        let aside = html! {
            @if let Some(by) = by.filter(|_| !rows.is_empty()) {
                a href={ "/p/" (p.id) "/breakdown?by=" (by) "&env=" (env) } { "Break down by " (title.to_lowercase()) " →" }
            }
        };
        let body = html! {
            @if rows.is_empty() { p.muted.small { "Nothing yet." } }
            @else { (share_rows(rows.iter().take(8), users)) }
        };
        card(id, title, aside, body)
    };
    let tiles = html! {
        (plain_tile("Users with a page view", html! { (compact(a.users)) }, format!("last 30 days, in {}", f.environment)))
        (plain_tile("Countries", html! { (a.countries.len()) },
            (if geo { "with at least one user" } else { "the country database is not installed" }).to_owned()))
        @match a.countries.first() {
            Some((iso, n)) => (plain_tile("Top country", html! { (name(iso)) small { (pct(*n as f64 / users)) } }, format!("{} users", compact(*n)))),
            None => (plain_tile("Top country", html! { "—" }, "no country known yet".to_owned())),
        }
        @match a.devices.first() {
            Some((d, n)) => (plain_tile("Top device", html! { (d) small { (pct(*n as f64 / users)) } },
                a.devices.get(1).map_or_else(|| format!("{} users", compact(*n)), |(d2, n2)| format!("then {d2}, {}", pct(*n2 as f64 / users))))),
            None => (plain_tile("Top device", html! { "—" }, "no device known yet".to_owned())),
        }
    };
    layout_in(
        "Audience",
        Some(p),
        context_bar(p.id, b, f, "/audience", false),
        html! {
            div.board.people {
                (head("Audience", html! { (compact(a.users)) " users with a page view in the last 30 days, in " strong { (f.environment) } ". IP addresses are used for the country and never stored." }))
                (key_numbers(tiles))
                section.card.pad.stack aria-labelledby="map-h" {
                    div.section-head { h2 id="map-h" { "Where they are" } }
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
                            div.scroll { table { thead { tr { th scope="col" { "Country" } (th_num("Users")) (th_num("Share")) } }
                                tbody { @for (iso, n) in &a.countries { tr { th scope="row" { (name(iso)) " " span.muted.small { (iso) } } td.num { (compact(*n)) } td.num { (pct(*n as f64 / users)) } } } } } }
                        }
                    }
                    p.muted.small { "IP geolocation by " a href="https://db-ip.com" { "DB-IP" } " (CC BY 4.0). Map outlines: Natural Earth." }
                }
                div.people-grid {
                    (list("aud-devices", "Devices", &a.devices, Some("device")))
                    (list("aud-browsers", "Browsers", &a.browsers, Some("browser")))
                    (list("aud-os", "Systems", &a.os, Some("os")))
                    (list("aud-lang", "Languages", &a.languages, None))
                    (list("aud-screens", "Screens", &a.screens, None))
                }
                @if let Some(x) = accounts { (accounts::section(x)) }
            }
        },
    )
}

/// What Acquisition can group by: (query value, label).
const DIMS: [(&str, &str); 4] = [
    ("channel", "Channel"),
    ("source", "Source"),
    ("campaign", "Campaign"),
    ("referrer", "Referrer"),
];

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
    let touch = if last { "last" } else { "first" };
    // A tab keeps the page's filters (environment, role, release, team).
    let tab = |by: &str, touch: &str| {
        detail_url(
            p.id,
            &format!("/acquisition?by={}&touch={touch}", url_enc(by)),
            f,
        )
    };
    let dim = DIMS.iter().find(|d| d.0 == by).map_or("Channel", |d| d.1);
    let total: i64 = rows.iter().map(|r| r.users).sum();
    let sum = |g: fn(&openlens_engine::AcquisitionRow) -> i64| rows.iter().map(g).sum::<i64>();
    let share = |part: i64, of: i64| part as f64 / of.max(1) as f64;
    // The rows come ranked; the best is the first with a fair sample.
    let fair = rows.iter().find(|r| r.users >= 20);
    let best = fair.or(rows.first());
    let tiles = html! {
        (plain_tile("Users acquired", html! { (compact(total)) }, format!("by {touch} touch, last 90 days")))
        (plain_tile("Reached value", html! { (pct(share(sum(|r| r.reached), total))) }, "reached a success event".to_owned()))
        (plain_tile("Came back", html! { (pct(share(sum(|r| r.returned), total))) }, "active on two days or more".to_owned()))
        @if let Some(r) = best {
            (plain_tile(&format!("Best {}", dim.to_lowercase()), html! { (r.value) small { (pct(r.value_rate())) } },
                format!("of its {} users reached value{}", compact(r.users), if fair.is_some() { "" } else { " (a small sample)" })))
        }
    };
    let body = html! {
        @for r in rows {
            tr {
                th scope="row" { span.mono { (r.value) } @if r.users < 20 { " " span.tag.tone-warn title="Under 20 users: the rates move a lot" { "low sample" } } }
                td.num { (compact(r.users)) span.muted.small { " · " (pct(share(r.users, total))) } }
                td.bar { (barcell(r.value_rate())) }
                td.num { (pct(share(r.returned, r.users))) }
                td.num { @if r.judged > 0 { (pct(share(r.churned, r.judged))) } @else { span.muted { "—" } } }
            }
        }
    };
    layout_in(
        "Acquisition",
        Some(p),
        context_bar(
            p.id,
            b,
            f,
            &format!("/acquisition?by={by}&touch={touch}"),
            false,
        ),
        html! {
            div.board.people {
                (head("Where users come from", html! { "Last 90 days, " strong { (f.environment) } ", by " (if last { "last touch (direct visits don't count)" } else { "first touch" }) ". Ranked by the share that reaches the value moment, not by visits." }))
                div.people-tabs {
                    nav.tabs aria-label="Group by" {
                        @for (k, l) in DIMS {
                            a href=(tab(k, touch)) aria-current=[(k == by).then_some("page")] { (l) }
                        }
                    }
                    nav.tabs aria-label="Touch" {
                        a href=(tab(by, "first")) aria-current=[(!last).then_some("page")] { "First touch" }
                        a href=(tab(by, "last")) aria-current=[last.then_some("page")] { "Last touch" }
                    }
                }
                @if rows.is_empty() {
                    section.card.empty aria-labelledby="no-landing" {
                        (crate::tables::empty_art())
                        div {
                            h2 id="no-landing" { "No landing with a channel yet" }
                            p { "The SDK sends the referrer and the campaign parameters on the first page view of a visit (SDK builds from 2026-11-23 on)." }
                        }
                    }
                } @else {
                    (key_numbers(tiles))
                    (TableCard {
                        label: "Where users come from",
                        head: html! { h2 { "By " (dim.to_lowercase()) } span.muted.small { (rows.len()) " rows" } },
                        cols: &[Col::Shown(dim), Col::Num("Users"), Col::Shown("Reached value"), Col::Num("Came back"), Col::Num("Churned")],
                        body,
                        foot: html! { div.pad { p.muted.small { "Reached value: a success event of the feature map (any feature use when none is declared). Came back: active on two days or more. Churned: of the users first seen before the churn window, those inactive for longer than it now." } } },
                    }.render())
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
    // A breakdown: each amount as a bar, the share of its currency's total.
    let table =
        |id: &str, title: &str, rows: &[(String, String, f64)], r: &openlens_engine::Revenue| {
            let of = |cur: &str| {
                r.totals
                    .iter()
                    .find(|m| m.currency == cur)
                    .map_or(1.0, |m| m.total)
            };
            let body = html! {
                @if rows.is_empty() { p.muted.small { "Nothing yet." } }
                @else {
                    div.scroll { table.share-bars { tbody { @for (k, c, v) in rows {
                        tr {
                            th scope="row" { span.mono { (k) } }
                            td.bar { (barcell(if of(c) > 0.0 { *v / of(c) } else { 0.0 })) }
                            td.num { (money(*v, c)) }
                        }
                    } } } }
                }
            };
            card(id, title, html! {}, body)
        };
    layout_in(
        "Revenue",
        Some(p),
        context_bar(p.id, b, f, "/revenue", false),
        html! {
            div.board.people {
                (head("Revenue", html! { "Amounts carried by success events (" code { "revenue" } " or " code { "value" } ", with an optional " code { "currency" } "), last 30 days, " strong { (f.environment) } "." }))
                @match r {
                    // One explained empty state (#544): which event, with the snippet.
                    None => section.card.empty aria-labelledby="no-revenue" {
                        (crate::tables::empty_art())
                        div {
                            h2 id="no-revenue" { "No payment recorded yet" }
                            p { "Revenue comes from a feature's success event that carries an amount in " code { "revenue" } " (or " code { "value" } "), with an optional " code { "currency" } ". Send it from the app when the payment succeeds:" }
                            pre.copy { code { "openlens.track(\"order_paid\", { revenue: 49, currency: \"EUR\" });" } }
                            p { "Then declare " code { "order_paid" } " as that feature's success event in the feature map." }
                            div.actions { a.btn.primary href={ "/p/" (p.id) "/features" } { "Open the feature map" } }
                        }
                    },
                    Some(r) => {
                        (key_numbers(html! {
                            @for m in &r.totals {
                                (plain_tile(&format!("Revenue ({})", m.currency), html! { (money(m.total, &m.currency)) },
                                    format!("{} orders · {} payers · {} per active user", compact(m.orders), compact(m.payers), money(m.per_active_user, &m.currency))))
                            }
                        }))
                        @if r.totals.len() > 1 { p.muted.small { "Currencies are not summed. Set " code { "revenue: { currency: EUR, rates: { USD: 0.92 } }" } " in the feature map to show one total." } }
                        @if r.converted { p.muted.small { "Converted into the display currency with the fixed rates of the feature map." } }
                        div.people-grid {
                            (table("rev-feature", "By feature", &r.by_feature, r))
                            (table("rev-role", "By role", &r.by_role, r))
                            (table("rev-release", "By release", &r.by_release, r))
                        }
                    }
                }
            }
        },
    )
}

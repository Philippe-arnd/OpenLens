//! Web Vitals on the Pages page (#620): the p75 of each vital over 7
//! days, its rating and, for one that is not good, why and what to do
//! (#408). It was a card on the board; the board keeps the Speed tile,
//! which opens this page.

use crate::*;

/// The card: one row per measured vital, the explanation under each one
/// that is not good. `vitals`: the engine's report, every vital.
pub(crate) fn web_vitals(
    pid: &str,
    f: &Filters,
    vitals: &[openlens_engine::VitalReport],
) -> Markup {
    let measured: Vec<&openlens_engine::VitalReport> =
        vitals.iter().filter(|v| v.samples > 0).collect();
    html! {
        section.card.pad.stack aria-labelledby="wv-h" {
            div.section-head { h2 id="wv-h" { "Web Vitals, p75 over 7 days" } }
            @if measured.is_empty() {
                p.muted { "No measurements yet. They arrive when visitors leave a page." }
            } @else {
                div.scroll {
                    table.plain {
                        tbody { @for v in &measured {
                            tr {
                                th scope="row" { (v.name) " " (vital_info(v.name)) }
                                td.num { @if let Some(p) = v.p75 { (openlens_engine::vitals::show(v.name, p)) } }
                                td { (status(v.rating)) }
                                td.num.muted { (compact(v.samples)) " samples" }
                            }
                            @if !matches!(v.rating, "good" | "not measured") {
                                tr { td colspan="4" { (vital_why(pid, f, v)) } }
                            }
                        } }
                    }
                }
            }
        }
    }
}

/// *Why and what to do* under a Web Vital that is not good (#408): the
/// worst pages at p75, the p75 per device, the dominant cause and the
/// guilty elements when the SDK sent attribution (#467), then the actions,
/// most likely first. A `<details>`, so it opens without script.
pub(crate) fn vital_why(pid: &str, f: &Filters, r: &openlens_engine::VitalReport) -> Markup {
    let show = |v| openlens_engine::vitals::show(r.name, v);
    html! {
        details.why {
            summary { "Why and what to do" }
            @if !r.worst_pages.is_empty() {
                p.small { "Worst pages at p75, last 7 days:" }
                ol.small { @for pg in &r.worst_pages {
                    li {
                        (row_link(&detail_url(pid, &format!("/events?family=vital&path={}", url_enc(&pg.path)), f), &pg.path))
                        " · " strong { (show(pg.p75)) } " · " (status(pg.rating)) span.muted { " · " (compact(pg.samples)) " samples" }
                    }
                } }
            } @else if openlens_engine::vitals_guide(r.name).is_some_and(|g| g.pages) {
                p.small.muted { "No page has enough measurements yet to rank them." }
            }
            @if !r.devices.is_empty() {
                p.small { "By device at p75: "
                    @for (i, d) in r.devices.iter().enumerate() {
                        @if i > 0 { " · " }
                        (d.device) " " strong { (show(d.p75)) } " " (status(d.rating))
                    }
                }
            }
            @if let Some(d) = &r.dominant_cause {
                p.small { "Most likely cause: " strong { (d.what) }
                    @if let Some(ph) = &d.phase {
                        span.muted { " (" (ph.label) " is " (pct(ph.share)) " of " (r.name) " in " (compact(ph.samples)) " slow measurements of the worst pages)" }
                    } @else if let Some(t) = d.ttfb_share {
                        span.muted { " (the first byte is " (pct(t)) " of LCP on " (d.pages) (if d.pages == 1 { " page" } else { " pages" }) ")" }
                    }
                }
            }
            @if !r.elements.is_empty() {
                p.small { "Elements most often behind slow measurements:" }
                ol.small { @for c in &r.elements {
                    li {
                        code { (c.element) }
                        @if let Some(k) = &c.kind { " · " (k) }
                        @if let Some(res) = &c.resource { " · " code { (res) } }
                        " on " (row_link(&detail_url(pid, &format!("/events?family=vital&path={}", url_enc(&c.path)), f), &c.path))
                        span.muted { " · " (compact(c.samples)) " slow" }
                    }
                } }
            }
            p.small { "What to do, most likely first:" }
            ol.small { @for a in &r.actions { li { (a) } } }
        }
    }
}

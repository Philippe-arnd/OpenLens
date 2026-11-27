//! The board's other cards (#543), in the v2 parts: Web Vitals, revenue,
//! role gaps, funnels, flags and paths to value. Since #588 they follow
//! the release comparison as closed disclosures, so the board reads like
//! the mockup down to it; each body ends with the link to its page
//! (#409), left out on a share link.

use crate::*;

/// A secondary card (#588): a closed disclosure whose summary is the
/// title and a short hint, and whose body ends with `more`, a link to
/// the card's page (not on a share link).
pub(super) fn disclose(
    id: &str,
    title: &str,
    hint: Markup,
    (href, more): (&str, &str),
    body: Markup,
) -> Markup {
    html! {
        details.card.disclose.secondary {
            summary {
                h2 id=(id) { (title) } // no_detail: a disclosure; its link ends the body
                (hint)
                (icon("chev"))
            }
            div.body {
                (body)
                @if !shared() { p.small { a href=(href) { (more) " →" } } }
            }
        }
    }
}

/// A summary's hint, in muted small text.
pub(super) fn hint(text: &str) -> Markup {
    html! { span.muted.small { (text) } }
}

/// Web Vitals, revenue and role gaps, one disclosure each.
pub(super) fn others(
    pid: &str,
    f: &Filters,
    b: &Board,
    revenue: Option<&openlens_engine::Revenue>,
) -> Markup {
    let poor = b.vitals.iter().filter(|v| v.rating != "good").count();
    let vitals_hint = if b.vitals.is_empty() {
        hint("No measurements yet")
    } else if poor == 0 {
        html! { span.tag.tone-good { "All good" } }
    } else {
        html! { span.tag.tone-warn { (poor) " to improve" } }
    };
    html! {
        (disclose("wv-h", "Web Vitals (p75)", vitals_hint, (&detail_url(pid, "/pages", f), "Pages"), html! {
                @if b.vitals.is_empty() { p.muted { "No measurements yet. They arrive when visitors leave a page." } }
                @else {
                    div.scroll {
                        table.plain {
                            tbody { @for v in &b.vitals {
                                tr {
                                    th scope="row" { (v.name) " " (vital_info(&v.name)) }
                                    td.num { @if v.name == "CLS" { (format!("{:.3}", v.p75)) } @else { (format!("{:.0} ms", v.p75)) } }
                                    td { (status(&v.rating)) }
                                    td.num.muted { (compact(v.samples)) " samples" }
                                }
                                @if let Some(r) = b.vitals_why.iter().find(|r| r.name == v.name) {
                                    tr { td colspan="4" { (vital_why(pid, f, r)) } }
                                }
                            } }
                        }
                    }
                }
        }))
        @if let Some(r) = revenue {
            @let total = r.totals.first().map(|m| money(m.total, &m.currency)).unwrap_or_default();
            (disclose("rev-h", "Revenue, last 30 days", hint(&total), (&detail_url(pid, "/revenue", f), "By feature, role and release"), html! {
                table.plain { tbody { @for m in &r.totals {
                    tr { th scope="row" { (money(m.total, &m.currency)) } td.small.muted { (compact(m.payers)) " payers · " (money(m.per_active_user, &m.currency)) " per active user" } }
                } } }
            }))
        }
        @if !b.role_gaps.is_empty() {
            (disclose("gaps-h", "Role gaps", html! { span.tag.tone-warn { (b.role_gaps.len()) " unmapped" } }, (&detail_url(pid, "/breakdown?by=role", f), "Break down by role"), html! {
                p.small { "These raw role values match no role in the feature map, so they are counted apart:" }
                p.tag-list { @for (raw, n) in &b.role_gaps { span.tag { code { (raw) } " · " (compact(*n)) " users" } } }
                p.muted.small { "Ask your agent to map them under " code { "roles:" } " in openlens.features.yaml." }
            }))
        }
    }
}

/// Funnels from each feature's declared steps.
pub(super) fn funnels(pid: &str, f: &Filters, b: &Board) -> Markup {
    if b.funnels.is_empty() {
        return html! {};
    }
    let n = b.funnels.len();
    let count = format!("{n} declared {}", if n == 1 { "funnel" } else { "funnels" });
    disclose(
        "fun-h",
        "Funnels",
        hint(&count),
        (&detail_url(pid, "/flows", f), "Flows"),
        html! {
            @for fun in &b.funnels {
                h3.small { (fun.feature) }
                div.scroll {
                    table.pulse {
                        tbody { @for (i, st) in fun.steps.iter().enumerate() {
                            tr {
                                th scope="row" { span.muted { (i + 1) ". " } (st.event) }
                                td.adopt { (super::charts::barcell(st.conversion)) }
                                td.num { (compact(st.users)) }
                                td.num.muted { @if i > 0 { (duration(st.median_secs)) } }
                            }
                        } }
                    }
                }
            }
            p.muted.small { "From each feature's declared steps, in order, within a day, last 30 days. Time is the median from the previous step." }
        },
    )
}

/// Each recent flag's sessions against everyone else's.
pub(super) fn flags(pid: &str, f: &Filters, b: &Board) -> Markup {
    if b.flag_cohorts.is_empty() {
        return html! {};
    }
    let n = b.flag_cohorts.len();
    let count = format!("{n} recent {}", if n == 1 { "flag" } else { "flags" });
    disclose(
        "flags-h",
        "Flags: with vs without",
        hint(&count),
        (&detail_url(pid, "/flags", f), "Flags"),
        html! {
            div.scroll {
                table.plain {
                    thead { tr { th scope="col" { "Flag" } (th_num("Users with / without")) (th_num("Avg session")) (th_num("Tasks / session")) (th_num("Sessions with errors")) (th_def("Confidence")) } }
                    tbody { @for c in &b.flag_cohorts {
                        tr {
                            th scope="row" { code { (c.flag) } }
                            td.num { (compact(c.with.users)) " / " (compact(c.without.users)) }
                            td.num { (duration(c.with.avg_secs)) " / " (duration(c.without.avg_secs)) }
                            td.num { (format!("{:.2}", c.with.tasks_per_session)) " / " (format!("{:.2}", c.without.tasks_per_session)) }
                            td.num { (pct(c.with.error_rate)) " / " (pct(c.without.error_rate)) }
                            td.small { (confidence(match (c.p_errors, c.p_tasks) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) })) }
                        }
                    } }
                }
            }
            p.muted.small { "Sessions where the flag was on, against all other sessions, last 30 days." }
        },
    )
}

/// Paths to value: what users did just before their first success.
pub(super) fn journeys(pid: &str, f: &Filters, b: &Board) -> Markup {
    if b.journeys.is_empty() {
        return html! {};
    }
    disclose(
        "paths-h",
        "Paths to value",
        hint("What users did just before their first success"),
        (&detail_url(pid, "/flows", f), "Flows"),
        html! {
            p.muted.small { "What users did just before reaching each success for the first time (last 30 days)." }
            ul.journeys {
                @for j in &b.journeys {
                    li {
                        @for step in &j.path { span.tag { (step) } span.arrow aria-hidden="true" { "→" } }
                        span.tag.tone-good { (icon("task")) (j.success) }
                        span.muted { (compact(j.users)) " users · " (pct(j.share)) " of those who got there" }
                    }
                }
            }
        },
    )
}

/// A p-value in words: whether the difference can be trusted yet.
pub(crate) fn confidence(p: Option<f64>) -> Markup {
    match p {
        None => {
            html! { span.muted title="Fewer than 30 sessions on one side" { "too few sessions" } }
        }
        Some(p) if p < openlens_engine::stats::ALPHA => {
            html! { strong title=(format!("p = {p:.3}")) { "significant" } }
        }
        Some(p) => {
            html! { span.muted title=(format!("p = {p:.3}")) { "not yet (p = " (format!("{p:.2}")) ")" } }
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

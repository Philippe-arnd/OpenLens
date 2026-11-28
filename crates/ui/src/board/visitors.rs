//! The board's Visitors block (#709, D-101): the anonymous traffic of the
//! window, per entry page and per channel (top 10 each), with sessions,
//! bounce rate and signup rate. Shown only when the window has anonymous
//! sessions: a project where everyone logs in never sees it. Plain
//! tables, no script.

use crate::*;
use openlens_engine::visitors::VisitorRow;

/// One table: the entry pages or the channels.
fn table(id: &str, title: &str, first: &str, rows: &[VisitorRow]) -> Markup {
    html! {
        div.stack {
            h3 id=(id) { (title) }
            div.scroll {
                table aria-labelledby=(id) {
                    thead { tr {
                        th scope="col" { (first) }
                        (th_num("Visitors"))
                        (th_num("Bounce"))
                        (th_num("Signup rate"))
                    } }
                    tbody {
                        @for r in rows {
                            tr {
                                th scope="row" { span.mono { (r.value) } }
                                td.num title={ (compact(r.sessions)) " sessions" } { (compact(r.visitors)) }
                                td.num { (pct(r.bounce_rate())) }
                                td.num title={ (compact(r.signed_up)) " signed up" } { (pct(r.signup_rate())) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The Visitors block, when the window has anonymous sessions.
pub(super) fn visitors(b: &Board) -> Markup {
    let Some(v) = b.visitors.as_ref() else {
        return html! {};
    };
    let period = super::period(b, "in the last 30 days");
    let bounce = v.bounces as f64 / v.sessions.max(1) as f64;
    html! {
        section.card.pad.stack aria-labelledby="visit-h" {
            div.section-head {
                h2 id="visit-h" { "Visitors" }
                span.small.muted {
                    (compact(v.sessions)) " anonymous sessions " (period) ", " (pct(bounce)) " bounced"
                }
            }
            div.split {
                (table("visit-pages", "Entry pages", "Page", &v.pages))
                (table("visit-channels", "Channels", "Channel", &v.channels))
            }
            p.muted.small {
                "An anonymous session is one sent before login. Its entry page is its first page view, its channel that page's landing. Signup rate: visitors linked by identify to an account created in the period ÷ visitors, a lower bound (a signup on another device leaves no link). Top 10 each."
            }
        }
    }
}

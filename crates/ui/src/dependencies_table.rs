//! Errors › Dependencies (#630, D-090): the external hosts the backend
//! calls, from `dependencies.list`: calls, share of failed calls, p95, and
//! the release before's value when it moved. The engine sorts them
//! (failing first, then the largest change). Only hosts: the engine never
//! reads back a path or a query. Without client stats, how to send them.

use openlens_engine::commands::dependencies::{Dependencies, Dependency};
use openlens_engine::commands::routes::Routes;

use super::*;
use crate::routes_page::{moved, ms};
use crate::tables::Col;

/// What the Errors page shows of the backend besides its error groups:
/// the routes' line and the outgoing dependencies, each absent when it
/// could not be read.
#[derive(Clone, Copy, Default)]
pub struct ErrorsBackend<'a> {
    /// `routes.list`.
    pub routes: Option<&'a Routes>,
    /// `dependencies.list`.
    pub dependencies: Option<&'a Dependencies>,
}

fn row(x: &Dependency, previous: Option<&str>) -> Markup {
    let c = x.change.as_ref();
    let fails = c.filter(|c| c.fails_more);
    let slower = c.filter(|c| c.slower);
    html! {
        tr {
            th scope="row" { code { (x.stat.host) } }
            td.num { (compact(x.stat.calls)) }
            td.num {
                (moved(pct(x.error_rate), fails.is_some(), fails.map(|c| pct(c.previous_error_rate)), previous))
                @if x.stat.no_response > 0 {
                    br; span.muted.small { (compact(x.stat.no_response)) " without a response" }
                }
            }
            td.num { (moved(ms(x.p95_ms), slower.is_some(), slower.map(|c| ms(c.previous_p95_ms)), previous)) }
        }
    }
}

/// The empty state: how to send outgoing call stats.
fn empty() -> Markup {
    html! {
        section.card aria-labelledby="no-deps" {
            h2 id="no-deps" { "No outgoing calls yet" }
            p {
                "This table lists the external services your backend calls, by host: how many calls, the share that failed "
                "(a 5xx or no response) and their p95. Call " code { "ol.captureOutgoing()" } " once at start-up with "
                code { "@openlens-obs/node" } ", or export OpenTelemetry's " code { "http.client.request.duration" }
                " over OTLP to " code { "/v1/metrics" } ". Only the host is kept, never the path or the query."
            }
        }
    }
}

/// The Dependencies table of the Errors page.
pub(crate) fn dependencies(d: &Dependencies) -> Markup {
    let previous = d.previous_release.as_deref();
    if d.hosts.is_empty() {
        return empty();
    }
    html! {
        (crate::tables::TableCard {
            label: "Dependencies",
            head: html! {
                h2 { "Dependencies" }
                span.muted.small { "Outgoing calls by host, " (d.environment) ", last " (d.days) " days" }
            },
            cols: &[Col::Shown("Host"), Col::Num("Calls"), Col::Num("Failed"), Col::Num("p95")],
            body: html! { @for x in &d.hosts { (row(x, previous)) } },
            foot: html! { div.pager { span {
                (d.hosts.len()) " " (if d.hosts.len() == 1 { "host" } else { "hosts" })
                " · failing first, then the largest change since the release before"
                @if d.worse > 0 {
                    " · " (d.worse) " got worse; that is shown, not judged, and usually not the release's fault"
                }
            } } },
        }
        .render())
    }
}

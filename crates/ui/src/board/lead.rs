//! The lead card of a regressed release (#615): the verdict in plain
//! words (built by the engine from its worst check), its checks grouped
//! by layer, the next step, and Create ticket, the errors on the release
//! and the full verdict.

use crate::*;
use openlens_engine::board_verdict::Lead;

/// The card.
pub(super) fn card(pid: &str, f: &Filters, l: &Lead) -> Markup {
    let rel = &l.release;
    let on = Filters {
        release: Some(rel.clone()),
        ..f.clone()
    };
    let ticket = format!(
        "/p/{pid}/tickets/new?kind=release&subject={}",
        url_enc(&format!("{}:{rel}", f.environment))
    );
    html! {
        article.card.lead {
            div.lead-top {
                span.rank aria-hidden="true" { "01" }
                span.tag.tone-bad { (icon("alert")) "Release regressed" }
                span.muted.small {
                    @match (&l.previous, l.sessions.1) {
                        (Some(p), Some(n)) => { (rel) " vs " (p) " · " (compact(l.sessions.0)) " vs " (compact(n)) " sessions" }
                        _ => { (rel) " · " (compact(l.sessions.0)) " sessions" }
                    }
                }
            }
            div.lead-text {
                h2 { span.sr-only { "Insight 1: " } (l.headline) } // no_detail: the actions below
                p.evidence { (l.evidence) }
            }
            (crate::release_verdict::layers(&l.layers))
            div.next {
                (icon("idea"))
                div {
                    strong { "Next step" }
                    p { (l.next_step) }
                }
            }
            @if !shared() {
                div.actions {
                    a.btn.primary href=(ticket) { "Create ticket" }
                    a.btn href=(detail_url(pid, "/errors", &on)) { "Errors on " (rel) }
                    a.btn.ghost href={ "/p/" (pid) "/releases?env=" (url_enc(&f.environment)) } { "Full verdict" }
                }
            }
        }
    }
}

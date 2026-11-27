//! The lead card of a regressed release (#615): the verdict in plain
//! words (built by the engine from its worst check), its checks grouped
//! by layer, the next step, and Create ticket, the errors on the release
//! and the full verdict.

use crate::*;
use openlens_engine::board_verdict::Lead;
use openlens_engine::verdict::Outcome;

/// A check's mark: its tone, glyph and the words a screen reader says.
fn mark(o: Outcome) -> (&'static str, Markup, &'static str) {
    match o {
        Outcome::Worse => ("bad", html! { "!" }, "worse"),
        Outcome::Better => ("good", icon("task"), "better"),
        Outcome::Same => ("muted", html! { "=" }, "no change that counts"),
        Outcome::TooFew => ("muted", html! { "?" }, "too few to say"),
        Outcome::Info => ("muted", html! { "·" }, "shown, not judged"),
    }
}

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
            @if !l.layers.is_empty() {
                div.layers {
                    @for g in &l.layers {
                        div.layer {
                            h3 { (icon(if g.name == "Features" { "layers" } else { "globe" })) (g.name) }
                            @for r in &g.rows {
                                @let (tone, glyph, says) = mark(r.outcome);
                                div class={ "check tone-" (tone) } {
                                    span.mark role="img" aria-label=(says) { (glyph) }
                                    span { (r.label) }
                                    span.fromto {
                                        @if let Some(p) = &r.from { (p) " → " }
                                        @if r.outcome == Outcome::Worse { b { (r.to) } } @else { (r.to) }
                                    }
                                }
                            }
                            @if g.more > 0 { p.muted.small { (g.more) " more in the full verdict" } }
                        }
                    }
                }
            }
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

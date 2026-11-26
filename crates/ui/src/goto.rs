//! Go to… (#541): the sidebar field that opens a section, a feature or a
//! person by name, and the page that lists what a name matched. The
//! section names are the sidebar's own ([`NAV`]); the project's features
//! and people are looked up by the `goto.lookup` engine command.

use maud::{Markup, html};

use openlens_engine::commands::goto::MAX_QUERY as GOTO_MAX;

use crate::{NAV, ProjectInfo, layout};

/// The most options the field's suggestion list carries.
const LIST_MAX: usize = 50;

/// Every section and tab name of the sidebar, once each (the first path
/// wins), sections before their tabs: (name, path under `/p/<id>`).
fn places() -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    let mut add = |label: &'static str, path: &'static str| {
        if !out.iter().any(|(l, _)| l.eq_ignore_ascii_case(label)) {
            out.push((label, path));
        }
    };
    for g in NAV {
        if let Some((first, _, _)) = g.tabs.first() {
            add(g.label, *first);
        }
        for (path, label, _) in g.tabs {
            add(*label, *path);
        }
    }
    out
}

/// A place Go to… can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoPlace {
    /// What it is called.
    pub label: String,
    /// Where it is (a path on this server).
    pub href: String,
    /// `Section`, `Feature` or `Person`.
    pub kind: &'static str,
}

/// The sections of project `id` that `q` names (ignoring case and the
/// spaces around it), then the ones whose name contains it.
pub fn goto_sections(id: &str, q: &str) -> (Vec<GotoPlace>, Vec<GotoPlace>) {
    let q = q.trim().to_lowercase();
    let (mut exact, mut partial) = (Vec::new(), Vec::new());
    if q.is_empty() {
        return (exact, partial);
    }
    for (label, path) in places() {
        let place = GotoPlace {
            label: label.to_owned(),
            href: format!("/p/{id}{path}"),
            kind: "Section",
        };
        let name = label.to_lowercase();
        if name == q {
            exact.push(place);
        } else if name.contains(&q) {
            partial.push(place);
        }
    }
    (exact, partial)
}

/// The sidebar's field: a plain GET form, its suggestions rendered by the
/// server. Only the section names are suggested: the layout has no feature
/// map at hand, and reading one for every page would cost every page a
/// lookup (features are still found when typed, by the route).
pub(crate) fn goto_form(p: &ProjectInfo<'_>) -> Markup {
    html! {
        form.goto method="get" action={ "/p/" (p.id) "/go" } role="search" {
            label.sr-only for="goto-q" { "Go to a section, feature or person" }
            input #goto-q type="search" name="q" list="goto-list" maxlength=(GOTO_MAX) placeholder="Go to…" autocomplete="off";
            datalist #goto-list {
                @for (label, _) in places().into_iter().take(LIST_MAX) {
                    option value=(label) {}
                }
            }
        }
    }
}

/// What `q` matched: several places, close ones, or nothing. `exact` says
/// whether `places` are exact matches (several of them) or only close ones.
pub fn goto_results(p: &ProjectInfo<'_>, q: &str, places: &[GotoPlace], exact: bool) -> Markup {
    layout(
        "Go to",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Go to…" }
                @if places.is_empty() {
                    p { "Nothing matches “" (q) "”." }
                } @else if exact {
                    p.muted { "Several places are called “" (q) "”. Pick one:" }
                } @else {
                    p.muted { "Nothing is called “" (q) "” exactly. Close names:" }
                }
            } }
            @if !places.is_empty() {
                ul.goto-results {
                    @for place in places {
                        li { a href=(place.href) { (place.label) } " " span.muted.small { (place.kind) } }
                    }
                }
            }
            p { a href={ "/p/" (p.id) } { "Back to the board" } }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_or_tab_name_is_found_whatever_its_case() {
        let (exact, _) = goto_sections("app", "  ERRORS ");
        assert_eq!(exact.len(), 1);
        assert!(exact[0].href.ends_with("/errors"));
        let (exact, _) = goto_sections("app", "feature map");
        assert!(exact.iter().any(|p| p.href.ends_with("/features")));
        let (exact, partial) = goto_sections("app", "feat");
        assert!(exact.is_empty());
        assert!(partial.len() > 1, "Feature flow and Feature map");
    }
}

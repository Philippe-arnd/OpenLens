//! One feature's page (#411): `Projects › project › Product › Feature map ›
//! area › feature`, with tabs (Overview, Flow, History). The breadcrumb and
//! the tabs are the shared components (#402, #403).

use super::*;

/// What a feature page shows.
pub struct FeaturePage<'a> {
    /// The project.
    pub p: &'a ProjectInfo<'a>,
    /// The feature as declared.
    pub feature: &'a openlens_featuremap::Feature,
    /// Its numbers, findings and neighbours.
    pub ctx: &'a openlens_engine::FeatureContext,
    /// `overview` or `history`.
    pub tab: &'a str,
    /// (version, saved at, author) of the map versions.
    pub versions: &'a [(i64, String, Option<String>)],
    /// The previous and next features of its area.
    pub prev: Option<(&'a str, &'a str)>,
    /// See `prev`.
    pub next: Option<(&'a str, &'a str)>,
}

fn trend(now: i64, before: i64) -> String {
    if before == 0 {
        return if now == 0 { "—".into() } else { "new".into() };
    }
    let d = (now - before) as f64 / before as f64 * 100.0;
    format!("{}{d:.0}%", if d >= 0.0 { "+" } else { "" })
}

/// The feature page.
pub fn feature_page(v: &FeaturePage<'_>) -> Markup {
    let f = v.feature;
    let base = format!("/p/{}/features/{}", v.p.id, f.key);
    let area = f.area.clone().unwrap_or_else(|| "Ungrouped".to_owned());
    let crumbs = Crumbs(vec![
        ("Projects".to_owned(), Some("/projects".to_owned())),
        (v.p.name.to_owned(), Some(format!("/p/{}", v.p.id))),
        (
            "Feature map".to_owned(),
            Some(format!("/p/{}/features", v.p.id)),
        ),
        (
            area.clone(),
            Some(format!(
                "/p/{}/features#area-{}",
                v.p.id,
                area.to_lowercase().replace(' ', "-")
            )),
        ),
        (f.name.clone(), None),
    ]);
    let tabs: Vec<(String, &str, bool)> = vec![
        (base.clone(), "Overview", v.tab == "overview"),
        (format!("/p/{}/flows?from={}", v.p.id, f.key), "Flow", false),
        (format!("{base}?tab=history"), "History", v.tab == "history"),
    ];
    layout_with(
        "Feature map",
        Some(v.p),
        &crumbs,
        html! {
            div.heading { div {
                h1 { (f.name) }
                p.muted { code { (f.key) } @if let Some(p) = &f.purpose { " · " (p) } }
            } }
            (components::tabs(&f.name, &tabs))
            @if v.tab == "history" {
                section.card.wide {
                    h2.sky { (icon("clock")) "Map versions" }
                    @if v.versions.is_empty() { p.muted { "No stored version yet." } }
                    @else {
                        table.plain {
                            thead { tr { th { "Version" } th { "Saved" } th { "By" } th { span.sr-only { "Open" } } } }
                            tbody { @for (n, at, by) in v.versions {
                                tr { td.num { (n) } td.mono { (at.get(..16).unwrap_or(at)) } td { (by.as_deref().unwrap_or("—")) }
                                     td { a href={ "/p/" (v.p.id) "/features?v=" (n) } { "Open" } } }
                            } }
                        }
                    }
                    @if !f.renamed_from.is_empty() { p.small { "Formerly: " @for (i, k) in f.renamed_from.iter().enumerate() { @if i > 0 { ", " } code { (k) } } } }
                }
            } @else {
                @if let Some(h) = &v.ctx.health {
                    section.tiles aria-label="Use, last 30 days" {
                        (tile("violet", "users", "Users", &compact(h.users), &format!("{} vs the 30 days before", trend(h.users, h.prev_users))))
                        (tile("sky", "task", "Uses", &compact(h.uses), &format!("{} vs the 30 days before", trend(h.uses, h.prev_uses))))
                        (tile("mint", "star", "Succeeded", &compact(h.success_users), &f.success.clone().unwrap_or_else(|| "no success event declared".into())))
                    }
                }
                div.grid {
                    section.card {
                        h2.violet { (icon("route")) "How it is reached" }
                        @if f.entry.routes.is_empty() { p.muted { "No route declared." } }
                        ul { @for r in &f.entry.routes { li { code { (r) } } } }
                        @if !f.steps.is_empty() { p.small { "Steps: " (f.steps.join(" → ")) } }
                        @if let Some(o) = &f.owner { p.small { "Owner: " (o) } }
                    }
                    section.card {
                        h2.mint { (icon("users")) "Who uses it" }
                        @if v.ctx.roles.is_empty() { p.muted { "Nobody in the last 30 days." } }
                        @else {
                            table.plain {
                                thead { tr { th { "Role" } th.num { "Users" } th.num { "Succeeded" } } }
                                tbody { @for (r, u, s) in &v.ctx.roles { tr { td { (r) } td.num { (compact(*u)) } td.num { (compact(*s)) } } } }
                            }
                        }
                    }
                    section.card {
                        h2.sky { (icon("layers")) "Pages" }
                        @if v.ctx.pages.is_empty() { p.muted { "No page seen." } }
                        ul { @for (path, n) in &v.ctx.pages { li { code { (path) } " · " (compact(*n)) } } }
                    }
                    section.card {
                        h2.sun { (icon("funnel")) "Related features" }
                        @if !f.requires.is_empty() { p.small { "Requires: " @for (i, k) in f.requires.iter().enumerate() { @if i > 0 { ", " } a href={ "/p/" (v.p.id) "/features/" (k) } { (k) } } } }
                        @if !v.ctx.required_by.is_empty() { p.small { "Required by: " @for (i, k) in v.ctx.required_by.iter().enumerate() { @if i > 0 { ", " } a href={ "/p/" (v.p.id) "/features/" (k) } { (k) } } } }
                        @if !v.ctx.area_peers.is_empty() { p.small { "Same area: " @for (i, k) in v.ctx.area_peers.iter().enumerate() { @if i > 0 { ", " } a href={ "/p/" (v.p.id) "/features/" (k) } { (k) } } } }
                    }
                }
                @if !v.ctx.findings.is_empty() {
                    section.card.wide {
                        h2.coral { (icon("alert")) "Findings" }
                        ul { @for x in &v.ctx.findings { li { strong { (x.level) } " · " (x.message) } } }
                    }
                }
            }
            nav.prev-next aria-label="Features of this area" {
                @if let Some((k, n)) = v.prev { a href={ "/p/" (v.p.id) "/features/" (k) } { "‹ " (n) } }
                @if let Some((k, n)) = v.next { a href={ "/p/" (v.p.id) "/features/" (k) } { (n) " ›" } }
            }
        },
    )
}

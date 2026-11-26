//! One feature's page (#411): `Projects › project › Product › Feature map ›
//! area › feature`, with tabs (Overview, Flow, Errors, Replays, Flags,
//! Releases, History), each carrying the board filters. The breadcrumb and
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
    /// The tab: one of [`TABS`].
    pub tab: &'a str,
    /// The board filters, carried by every tab and link.
    pub filters: &'a Filters,
    /// Of `ctx.sessions`, the ones with a recording.
    pub replays: &'a [&'a openlens_engine::maphealth::FeatureSession],
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

/// The tabs of a feature page (`?tab=`), the first being the default.
pub const TABS: &[&str] = &[
    "overview", "errors", "replays", "flags", "releases", "history",
];

/// The feature page.
pub fn feature_page(v: &FeaturePage<'_>) -> Markup {
    let f = v.feature;
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
    let fl = v.filters;
    let pid = v.p.id;
    let at = |tab: &str| {
        let path = format!("/features/{}", url_enc(&f.key));
        if tab == "overview" {
            detail_url(pid, &path, fl)
        } else {
            detail_url(pid, &format!("{path}?tab={tab}"), fl)
        }
    };
    let flow = detail_url(pid, &format!("/flows?from={}", url_enc(&f.key)), fl);
    let tabs: Vec<(String, &str, bool)> = vec![
        (at("overview"), "Overview", v.tab == "overview"),
        (flow, "Flow", false),
        (at("errors"), "Errors", v.tab == "errors"),
        (at("replays"), "Replays", v.tab == "replays"),
        (at("flags"), "Flags", v.tab == "flags"),
        (at("releases"), "Releases", v.tab == "releases"),
        (at("history"), "History", v.tab == "history"),
    ];
    // Prev / next stay on the same tab.
    let prev_next = |k: &str| {
        let path = format!("/features/{}", url_enc(k));
        if v.tab == "overview" {
            detail_url(pid, &path, fl)
        } else {
            detail_url(pid, &format!("{path}?tab={}", v.tab), fl)
        }
    };
    let share = |a: i64, n: i64| pct(a as f64 / n.max(1) as f64);
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
            @if v.tab == "errors" {
                section.card.wide {
                    h2.coral { (icon("alert")) "Errors in sessions that used it" }
                    @if v.ctx.errors.is_empty() { p.muted { "No error in those sessions in the last 30 days." } }
                    @else {
                        table.plain {
                            thead { tr { th { "Message" } (th_num("Times")) (th_num("Users")) th { span.sr-only { "Replay" } } } }
                            tbody { @for e in &v.ctx.errors {
                                tr { td { code { (e.message) } } td.num { (compact(e.count)) } td.num { (compact(e.users)) }
                                     td { @if let Some(sid) = &e.last_session { a href={ "/p/" (pid) "/replays/" (url_enc(sid)) } { "Latest session" } } } }
                            } }
                        }
                        p.small { a href=(detail_url(pid, "/errors", fl)) { "All error groups →" } }
                    }
                }
            } @else if v.tab == "replays" {
                section.card.wide {
                    h2.violet { (icon("play")) "Recorded sessions that used it" }
                    @if v.replays.is_empty() { p.muted { "No recording among the latest sessions that used it (replay off, no consent, or not kept)." } }
                    @else {
                        table.plain {
                            thead { tr { th { "Last use" } th { "User" } th { "Outcome" } th { span.sr-only { "Play" } } } }
                            tbody { @for r in v.replays {
                                tr { td.mono { (local_time(&r.at, &v.filters.tz, false)) }
                                     td.mono { a href={ "/p/" (pid) "/people/" (url_enc(&r.distinct_id)) } { (r.distinct_id) } }
                                     td { @if r.succeeded { span.chip { "succeeded" } } @else if f.success.is_some() { span.muted { "did not succeed" } } @else { span.muted { "—" } } }
                                     td { a href={ "/p/" (pid) "/replays/" (url_enc(&r.session_id)) } { "Play" } } }
                            } }
                        }
                    }
                }
            } @else if v.tab == "flags" {
                section.card.wide {
                    h2.sun { (icon("flag")) "Flags on while it was used" }
                    @if v.ctx.flags.is_empty() { p.muted { "No flag was on during its use in the last 30 days." } }
                    @else {
                        @let all = v.ctx.health.as_ref().map_or(0, |h| h.users);
                        table.plain {
                            thead { tr { th { "Flag" } (th_num("Users with it on")) (th_num("Succeeded")) } }
                            tbody { @for x in &v.ctx.flags {
                                tr { th scope="row" { a href={ "/p/" (pid) "/flags-edit?key=" (url_enc(&x.flag)) } { code { (x.flag) } } }
                                     td.num { (compact(x.users)) @if all > 0 { br; span.muted.small { (share(x.users, all)) " of its users" } } }
                                     td.num { @if f.success.is_some() { (share(x.success_users, x.users)) } @else { span.muted { "—" } } } }
                            } }
                        }
                        p.muted.small { "From the flags the SDK reports on each event. Compare with the feature's overall success on the Overview tab." }
                    }
                }
            } @else if v.tab == "releases" {
                section.card.wide {
                    h2.coral { (icon("release")) "Use per release" }
                    @if v.ctx.releases.is_empty() { p.muted { "Not used in the last 90 days." } }
                    @else {
                        table.plain {
                            thead { tr { th { "Release" } th { "Since" } (th_num("Users")) (th_num("Uses")) (th_num("Succeeded")) th { "Counted with" } } }
                            tbody { @for r in &v.ctx.releases {
                                tr { th scope="row" { @if r.release == "untagged" { span.muted { "untagged" } } @else { a href=(detail_url(pid, &format!("/features/{}?release={}", url_enc(&f.key), url_enc(&r.release)), &Filters { release: None, ..fl.clone() })) { code { (r.release) } } } }
                                     td.mono { (r.first_seen.get(..10).unwrap_or(&r.first_seen)) }
                                     td.num { (compact(r.users)) } td.num { (compact(r.uses)) }
                                     td.num { @if f.success.is_some() { (share(r.success_users, r.users)) } @else { span.muted { "—" } } }
                                     td.small { @match r.map_version {
                                         Some(n) => { a href={ "/p/" (v.p.id) "/features?v=" (n) } { "map v" (n) } @if r.before_first_map { span.muted { " (released before the first map)" } } @if r.not_in_map { span.muted { " · not in that map" } } },
                                         None => span.muted { "current map" },
                                     } } }
                            } }
                        }
                        p.muted.small { "Each release is counted with the map version in force when it came out, so a later push does not rewrite its numbers." }
                        p.small { a href=(detail_url(pid, "/timeline", fl)) { "Release timeline →" } }
                    }
                }
            } @else if v.tab == "history" {
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
                                thead { tr { th { "Role" } (th_num("Users")) (th_num("Succeeded")) } }
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
                @if !v.ctx.relations.is_empty() {
                    (relations(v.p.id, &v.ctx.relations))
                }
                @if !v.ctx.findings.is_empty() {
                    section.card.wide {
                        h2.coral { (icon("alert")) "Findings" }
                        ul { @for x in &v.ctx.findings { li { strong { (x.level) } " · " (x.message) } } }
                    }
                }
            }
            nav.prev-next aria-label="Features of this area" {
                @if let Some((k, n)) = v.prev { a href=(prev_next(k)) { "‹ " (n) } }
                @if let Some((k, n)) = v.next { a href=(prev_next(k)) { (n) " ›" } }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_replays_tab_prints_the_last_use_in_the_project_zone() {
        // #566: the tab cut the timestamp cast to text at 16 characters,
        // which could keep the ISO `T` and was always UTC.
        let p = ProjectInfo {
            id: "p1",
            name: "P",
            key: "k",
        };
        let feature = openlens_featuremap::Feature {
            key: "export".into(),
            name: "Export".into(),
            purpose: None,
            owner: None,
            entry: openlens_featuremap::Entry::default(),
            steps: vec![],
            success: None,
            area: None,
            requires: vec![],
            renamed_from: vec![],
        };
        let ctx = openlens_engine::FeatureContext::default();
        let filters = Filters {
            tz: "Europe/Paris".into(),
            ..Filters::default()
        };
        let s = openlens_engine::maphealth::FeatureSession {
            session_id: "s1".into(),
            distinct_id: "u1".into(),
            at: "2026-11-16T20:49:31.5".into(),
            succeeded: false,
        };
        let html = feature_page(&FeaturePage {
            p: &p,
            feature: &feature,
            ctx: &ctx,
            tab: "replays",
            filters: &filters,
            replays: &[&s],
            versions: &[],
            prev: None,
            next: None,
        })
        .into_string();
        assert!(!crate::tests::has_iso_time(&html), "{html}");
        // Paris is ahead of UTC in November.
        assert!(html.contains("2026-11-16 21:49"), "{html}");
    }
}

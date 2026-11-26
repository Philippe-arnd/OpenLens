//! The feature map page and its health.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// What the last 7 days say about a feature map (#221).
#[derive(Debug, Default)]
pub struct MapLive {
    /// (feature, uses, users).
    pub uses: Vec<(String, i64, i64)>,
    /// (success event, hits).
    pub success_hits: Vec<(String, i64)>,
    /// Raw role values no role of the map matches.
    pub unmapped_roles: Vec<String>,
}

/// Everything the feature map page shows.
pub struct MapView<'a> {
    /// Project.
    pub p: &'a ProjectInfo<'a>,
    /// The YAML shown in the editor.
    pub yaml: &'a str,
    /// The version shown (None for a draft).
    pub version: Option<i64>,
    /// The latest stored version.
    pub latest: Option<i64>,
    /// The parsed map, when valid.
    pub map: Option<&'a openlens_featuremap::FeatureMap>,
    /// Lint errors and warnings.
    pub issues: &'a [openlens_featuremap::Issue],
    /// Live status.
    pub live: &'a MapLive,
    /// Stored versions, newest first.
    pub history: &'a [openlens_meta::MapVersion],
    /// Measured health (30 days), #240.
    pub health: Option<&'a openlens_engine::MapHealth>,
    /// Admins edit.
    pub admin: bool,
    /// When checking a draft: the stored YAML to diff against.
    pub draft: Option<&'a str>,
    /// A notice to show.
    pub notice: Option<&'a str>,
}

/// A line diff of two texts: (`' '` same, `'-'` removed, `'+'` added).
/// Plain LCS, so bounded to 3000 lines a side (larger: whole replace).
pub fn line_diff<'a>(old: &'a str, new: &'a str) -> Vec<(char, &'a str)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > 3000 || b.len() > 3000 {
        return a
            .iter()
            .map(|l| ('-', *l))
            .chain(b.iter().map(|l| ('+', *l)))
            .collect();
    }
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1].saturating_add(1)
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(('-', a[i]));
            i += 1;
        } else {
            out.push(('+', b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| ('-', *l)));
    out.extend(b[j..].iter().map(|l| ('+', *l)));
    out
}

/// The measured health of the map (#240): score, findings with their fix,
/// areas and the adoption ladder.
pub(crate) fn map_health(pid: &str, h: &openlens_engine::MapHealth) -> Markup {
    let tone = if h.score >= 80 {
        "good"
    } else if h.score >= 50 {
        "warn"
    } else {
        "bad"
    };
    html! {
        section.card.wide {
            h2.mint { (icon("shield")) "Map health, last 30 days" }
            p {
                strong class={ "status " (tone) } { (h.score) " / 100" }
                " · " (pct(h.coverage)) " of pages and clicks belong to a feature"
                " · " (h.findings.len()) " findings"
            }
            @if h.findings.is_empty() {
                p.muted { "Nothing to fix: every feature is used, its success fires and no route overlaps." }
            } @else {
                ul.plain-list { @for f in &h.findings {
                    li {
                        span class={ "status " (if f.level == "error" { "bad" } else { "warn" }) } { (f.kind.replace('_', " ")) }
                        " " (f.message) br;
                        span.muted.small { (f.evidence) }
                        @if let Some(fix) = &f.fix { details { summary.small { "Suggested change" } pre.diff { (fix) } } }
                        " " a.small href={ "/p/" (pid) "/tickets/new?kind=map&subject=" (url_enc(&format!("{}:{}", f.kind, f.feature.as_deref().unwrap_or("")))) } { "Create ticket →" }
                    }
                } }
            }
            p.muted.small { "Score: 100, minus up to 40 for pages and clicks no feature claims, 10 per error and 4 per warning. Agents read the same report with " code { "openlens-cli features lint --live" } "." }
        }
        @if !h.areas.is_empty() {
            section.card {
                h2.violet { (icon("layers")) "Areas" }
                div.table-scroll { table.plain {
                    thead { tr { th { "Area" } (th_num("Features")) (th_num("Uses (30 d)")) (th_num("Users")) } }
                    tbody { @for a in &h.areas {
                        tr { th scope="row" { (a.area) } td.num { (a.features) } td.num { (compact(a.uses)) } td.num { (compact(a.users)) } }
                    } }
                } }
            }
        }
        @if !h.ladder.is_empty() {
            section.card {
                h2.sky { (icon("funnel")) "Adoption ladder" }
                div.table-scroll { table.plain {
                    thead { tr { th { "From" } th { "Then" } (th_num("Users")) (th_num("Went on")) (th_num("Skipped the first")) } }
                    tbody { @for r in &h.ladder {
                        tr {
                            th scope="row" { code { (r.from) } }
                            td { code { (r.to) } }
                            td.num { (compact(r.from_users)) }
                            td.num { strong { (pct(r.reached as f64 / r.from_users.max(1) as f64)) } }
                            td.num { (compact(r.skipped)) }
                        }
                    } }
                } }
                p.muted.small { "From the map's " code { "requires" } ": of the users of a feature, the share who went on to the one that requires it. Where most skip it, the requirement is probably wrong." }
            }
        }
        @if !h.relations.is_empty() { (relations(pid, &h.relations)) }
    }
}

/// The relations inferred between features (#283, #483): a wide card.
pub(crate) fn relations(pid: &str, rels: &[openlens_engine::Relation]) -> Markup {
    html! {
        section.card.wide {
            h2.violet { (icon("route")) "Used with" }
            div.table-scroll { table.plain {
                thead { tr { th { "Before" } th { "After" } (th_num("Sessions together")) (th_num("Lift")) (th_num("Comes first")) th { "Leads to value" } } }
                tbody { @for r in rels {
                    tr {
                        td { a href={ "/p/" (pid) "/features/" (r.first) } { code { (r.first) } } }
                        td { a href={ "/p/" (pid) "/features/" (r.then) } { code { (r.then) } } }
                        td.num { (compact(r.together)) }
                        td.num { (format!("×{:.1}", r.lift)) }
                        td.num { (pct(r.first_share)) }
                        td.small { @match (r.success_after, r.success_without) {
                            (Some(a), Some(w)) => { @if r.leads_to_value { span.status.good { "yes" } " " } (pct(a)) " after vs " (pct(w)) " without" },
                            _ => span.muted { "—" },
                        } }
                    }
                } }
            } }
            p.muted.small { "From every session of the last 30 days (with the filters above). Lift: how many times more often the two are used in the same session than chance. Leads to value: the second one succeeds more often after the first, and the difference is significant." }
        }
    }
}

/// One "vs baseline" cell of the variant readout.
pub(crate) fn versus(baseline: bool, p: Option<f64>, significant: bool) -> Markup {
    html! {
        @if baseline { span.muted { "—" } }
        @else { @match p {
            None => span.muted { "too few" },
            Some(p) if significant => { span.status.good { "differs" } " p=" (format!("{p:.3}")) },
            Some(p) => { span.muted { "no clear difference" } " p=" (format!("{p:.3}")) },
        } }
    }
}

/// The feature map page (#221).
pub fn feature_map(v: &MapView<'_>) -> Markup {
    let p = v.p;
    let errors = v.issues.iter().filter(|i| i.level == "error").count();
    let uses = |k: &str| v.live.uses.iter().find(|(f, _, _)| f == k);
    let hits = |e: &str| {
        v.live
            .success_hits
            .iter()
            .find(|(x, _)| x == e)
            .map_or(0, |(_, n)| *n)
    };
    let diff = v.draft.map(|old| line_diff(old, v.yaml));
    let changed = diff
        .as_ref()
        .is_some_and(|d| d.iter().any(|(c, _)| *c != ' '));
    layout(
        "Feature map",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Feature map" }
                    p.muted {
                        @match (v.draft.is_some(), v.version, v.latest) {
                            (true, _, _) => "Checking an edit, not saved yet.",
                            (_, Some(n), Some(l)) if n != l => { "Version " (n) " (latest is " a href={ "/p/" (p.id) "/features" } { (l) } ")." },
                            (_, Some(n), _) => { "Version " (n) ", used by every board." },
                            _ => "None yet: agents push it with openlens-cli features push, or write it below.",
                        }
                    }
                }
                @if v.latest.is_some() {
                    p { a.button href={ "/p/" (p.id) "/features/map.yaml" (v.version.map(|n| format!("?v={n}")).unwrap_or_default()) } { "YAML" }
                        " " a.button href={ "/p/" (p.id) "/features/map.json" (v.version.map(|n| format!("?v={n}")).unwrap_or_default()) } { "JSON" } }
                }
            }
            @if let Some(n) = v.notice { p.notice role="status" { (n) } }
            @if !v.issues.is_empty() {
                section.card.wide {
                    h2.coral { (icon("alert")) (errors) " errors · " (v.issues.len() - errors) " warnings" }
                    ul.plain-list { @for i in v.issues {
                        li { span class={ "status " (if i.level == "error" { "bad" } else { "warn" }) } { (i.level) } " " code { (i.path) } " " (i.message) }
                    } }
                }
            }
            @if !v.live.unmapped_roles.is_empty() {
                section.card.wide {
                    h2.sun { (icon("gap")) "Roles sent that no role matches" }
                    p { @for (i, r) in v.live.unmapped_roles.iter().enumerate() { @if i > 0 { ", " } code { (r) } } }
                    p.muted.small { "Add them to a role's " code { "match" } " list, or they count apart on the board." }
                }
            }
            @if let Some(m) = v.map {
                section.card.wide {
                    h2.violet { (icon("layers")) (m.features.len()) " features · " (m.roles.len()) " roles · churn after " (m.churn_days()) " days" }
                    div.table-scroll { table.plain {
                        thead { tr { th { "Feature" } th { "Entry" } th { "Success" } (th_num("Uses (7 d)")) (th_num("Users")) th { "Status" } } }
                        tbody { @for f in &m.features {
                            @let u = uses(&f.key);
                            @let h = f.success.as_deref().map(hits);
                            tr {
                                th scope="row" { strong { a href={ "/p/" (v.p.id) "/features/" (f.key) } { (f.name) } } br; code.small { (f.key) } @if let Some(o) = &f.owner { br; span.muted.small { (o) } } }
                                td.small {
                                    @for r in &f.entry.routes { code { (r) } " " }
                                    @for s in &f.entry.selectors { code { (s) } " " }
                                    @for c in &f.entry.calls { code { (c) } " " }
                                }
                                td.small { @match &f.success { Some(s) => { code { (s) } " · " (compact(h.unwrap_or(0))) } , None => span.muted { "—" } } }
                                td.num { (compact(u.map_or(0, |x| x.1))) }
                                td.num { (compact(u.map_or(0, |x| x.2))) }
                                td { @if u.is_none() { span.status.bad { "not seen" } }
                                     @else if h == Some(0) { span.status.warn { "no success" } }
                                     @else { span.status.good { "live" } } }
                            }
                        } }
                    } }
                    @if !m.roles.is_empty() {
                        p.small { "Roles: " @for (i, r) in m.roles.iter().enumerate() { @if i > 0 { " · " } strong { (r.key) } " ← " (r.matches.join(", ")) } }
                    }
                }
            }
            @if let Some(h) = v.health { (map_health(p.id, h)) }
            @if let Some(d) = &diff {
                section.card.wide {
                    h2.sky { (icon("route")) "Changes against the stored map" }
                    @if !changed { p.muted { "No change." } }
                    @else {
                        pre.diff { @for (c, l) in d {
                            @match c { '+' => { ins { "+ " (l) } }, '-' => { del { "- " (l) } }, _ => { span { "  " (l) } } }
                            "\n"
                        } }
                    }
                }
            }
            @if v.admin {
                section.card.wide {
                    h2.mint { (icon("code")) "Edit" }
                    form.sqlbox method="post" action={ "/p/" (p.id) "/features" } {
                        label for="map-yaml" { "openlens.features.yaml" }
                        textarea #map-yaml name="yaml" rows="18" spellcheck="false" { (v.yaml) }
                        label for="map-message" { "Why (kept with the version)" }
                        input #map-message type="text" name="message" maxlength="500" placeholder="e.g. split export into PDF and CSV";
                        div.actions {
                            button.soft type="submit" name="action" value="check" { "Check and show changes" }
                            button type="submit" name="action" value="save" { "Save as a new version" }
                        }
                    }
                    p.muted.small { "Saving a map with errors is refused; warnings are kept. The app's own file stays the source of truth: copy a change back there, or agents will push theirs over it." }
                }
            }
            @if !v.history.is_empty() {
                section.card.wide {
                    h2 { (icon("clock")) "History" }
                    ul.plain-list.small { @for h in v.history {
                        li { a href={ "/p/" (p.id) "/features?v=" (h.version) } { "Version " (h.version) } " · " (h.created_at.get(..16).unwrap_or(&h.created_at).replace('T', " ")) " UTC · " (h.bytes) " bytes"
                            @if let Some(a) = &h.author { " · " (a) }
                            @if Some(h.version) == v.latest { " · " span.status.good { "current" } }
                            @if let Some(m) = &h.message { br; span.muted { (m) } } }
                    } }
                }
            }
        },
    )
}

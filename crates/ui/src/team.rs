//! Projects, heatmaps, the digest, the team, About and surveys.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// One project in the switcher.
pub struct ProjectSummary {
    /// Project id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Events in the last 7 days, all environments.
    pub recent_events: i64,
    /// Latest event (UTC), if any.
    pub last_event: Option<String>,
    /// Time zone of the project's days (D-017).
    pub timezone: String,
}

/// All projects, busiest first.
pub fn projects(list: &[ProjectSummary], admin: bool) -> Markup {
    layout(
        "Projects",
        None,
        html! {
            div.heading {
                div { h1 { "Projects" } p.muted { "One per app. Pick one to open its Focus board." } }
                @if admin { a.button href="/users" { (icon("users")) "Team" } " " a.button href="/settings/storage" { (icon("layers")) "Storage and backups" } " " a.button href="/settings/tokens" { (icon("code")) "API tokens" } " " a.button href="/settings/ai" { (icon("spark")) "AI" } " " a.button href="/audit" { (icon("shield")) "Audit log" } }
            }
            @if admin {
                details.card.wide.new-project {
                    summary { (icon("plus")) "New project" }
                    form.invite method="post" action="/projects" {
                        label { "Name " input type="text" name="name" required maxlength="80" placeholder="My app"; }
                        label { "Time zone " input type="text" name="timezone" placeholder="Europe/Paris" maxlength="64" list="tz-all"; }
                        (zone_list())
                        button type="submit" { "Create" }
                    }
                    p.muted.small { "Agents do the same with " code { "openlens-cli init --name <app>" } ". Empty time zone: UTC." }
                }
            }
            ul.project-list {
                @for p in list {
                    li {
                        a.card href={ "/p/" (p.id) } {
                            h2.violet { img.picon src={ "/p/" (p.id) "/icon" } alt="" width="24" height="24"; (p.name) }
                            @if p.recent_events > 0 {
                                p { strong { (compact(p.recent_events)) } " events in the last 7 days" }
                            } @else {
                                p.muted { "No events in the last 7 days" }
                            }
                            @if let Some(t) = &p.last_event { p.muted.small { "Last event " (t.get(..16).unwrap_or(t)) " UTC" } }
                            p.muted.small { "Days in " (p.timezone) }
                        }
                    }
                }
            }
        },
    )
}

/// What the heatmap page shows around the map (#548).
pub struct HeatmapView<'a> {
    /// The asked mode: `clicks` (the default), `rage` or `scroll`.
    pub mode: &'a str,
    /// The most viewed pages: the picker's suggestions, and this page's
    /// most clicked labels and scroll depth when it is one of them.
    pub pages: &'a [openlens_engine::PageStats],
}

/// The click map as SVG dots, every click or the rage clicks only.
fn click_svg(map: &openlens_engine::ClickMap, rage_only: bool) -> Markup {
    let cols = openlens_engine::MAP_COLUMNS;
    let row = openlens_engine::MAP_ROW;
    let vw = if map.viewport > 0 { map.viewport } else { 1280 };
    let cells: Vec<(i64, i64, i64, i64)> = map
        .cells
        .iter()
        .copied()
        .filter(|c| !rage_only || c.3 > 0)
        .collect();
    let rows = cells.iter().map(|c| c.1).max().unwrap_or(0) + 2;
    let height = (rows * row).max(400);
    let weight = |c: (i64, i64, i64, i64)| if rage_only { c.3 } else { c.2 };
    let max = cells.iter().map(|c| weight(*c)).max().unwrap_or(1).max(1) as f64;
    let cw = vw as f64 / cols as f64;
    let what = if rage_only {
        "Rage clicks"
    } else {
        "Click density"
    };
    html! {
        svg.heatmap viewBox={ "0 0 " (vw) " " (height) } role="img" aria-label={ (what) " on " (map.path) } {
            rect.page x="0" y="0" width=(vw) height=(height) {}
            @for c in &cells {
                @let (cx, cy, n, rage) = *c;
                @let o = format!("{:.2}", (0.15 + 0.85 * (weight(*c) as f64 / max)).min(1.0));
                circle class=(if rage > 0 { "dot rage" } else { "dot" })
                    cx=(format!("{:.0}", (cx as f64 + 0.5) * cw)) cy=((cy * row) + row / 2)
                    r=(format!("{:.0}", (cw * 0.9).max(8.0))) fill-opacity=(o) {
                    title { (n) " clicks" @if rage > 0 { ", " (rage) " rage" } }
                }
            }
        }
    }
}

/// The most clicked labels (or the most rage-clicked), busiest first, as
/// a ranked list of bars.
fn ranked_labels(list: &[openlens_engine::Hotspot], rage: bool) -> Markup {
    let n = |h: &openlens_engine::Hotspot| if rage { h.rage } else { h.clicks };
    let mut v: Vec<&openlens_engine::Hotspot> = list.iter().filter(|h| n(*h) > 0).collect();
    v.sort_by_key(|h| std::cmp::Reverse(n(*h)));
    let max = v.first().map_or(1, |h| n(*h)).max(1) as f64;
    let unit = if rage { " rage" } else { " clicks" };
    html! {
        @if v.is_empty() { p.muted.small { "No clicked element on this page in this window." } }
        @else {
            ol.ranked-bars aria-label=(if rage { "Most rage-clicked elements" } else { "Most clicked elements" }) {
                @for (i, h) in v.iter().enumerate() {
                    li {
                        span.rank { (format!("{:02}", i + 1)) }
                        span.label { (h.label) }
                        (crate::board::bar_with(n(*h) as f64 / max, &format!("{}{unit}", compact(n(*h)))))
                    }
                }
            }
        }
    }
}

/// How far visits of the page scroll, as bars.
fn scroll_bars(pg: &openlens_engine::PageStats) -> Markup {
    html! {
        ol.ranked-bars aria-label="Scroll depth" {
            li { span.label { "Reach half the page" } (crate::board::bar_with(pg.reach_half, &pct(pg.reach_half))) }
            li { span.label { "Reach the end" } (crate::board::bar_with(pg.reach_end, &pct(pg.reach_end))) }
        }
        p.muted.small { "Median deepest scroll: " (format!("{:.0}", pg.median_scroll)) "% of the page." }
    }
}

/// A page's heatmap (#548): a page picker, one tab per mode the data has
/// (clicks, rage clicks, scroll depth), the click map drawn in SVG (no
/// script), and a link to see it over the real page when a recording of
/// that page exists. Without one, the most clicked elements and the scroll
/// depth stand in for the page.
pub fn heatmap(
    p: &ProjectInfo<'_>,
    map: &openlens_engine::ClickMap,
    overlay: Option<&str>,
    v: &HeatmapView<'_>,
) -> Markup {
    let vw = if map.viewport > 0 { map.viewport } else { 1280 };
    let page = v.pages.iter().find(|x| x.path == map.path);
    let has_rage = map.cells.iter().any(|c| c.3 > 0)
        || page.is_some_and(|pg| pg.hotspots.iter().any(|h| h.rage > 0));
    let has_scroll = page.is_some_and(|pg| pg.median_scroll >= 0.0);
    let mode = match v.mode {
        "rage" if has_rage => "rage",
        "scroll" if has_scroll => "scroll",
        _ => "clicks",
    };
    let base = crate::manage::heatmap_href(p.id, &map.path);
    let mut modes = vec![("clicks", "Clicks")];
    if has_rage {
        modes.push(("rage", "Rage clicks"));
    }
    if has_scroll {
        modes.push(("scroll", "Scroll depth"));
    }
    let no_labels = html! { p.muted.small { "Clicked elements and scroll depth are kept for the most viewed pages; this page is not one of them." } };
    layout(
        "Heatmap",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Heatmap" }
                p.muted { span.mono { (map.path) } " · " (compact(map.clicks)) " clicks with a position, last 30 days · median screen " (vw) " px wide" }
            } }
            form.card.refine method="get" action={ "/p/" (p.id) "/heatmap" } aria-label="Pick a page" {
                label { "Page"
                    input type="text" name="path" value=(map.path) list="heat-pages" required spellcheck="false";
                }
                datalist #heat-pages { @for pg in v.pages { option value=(pg.path) {} } }
                @if mode != "clicks" { input type="hidden" name="mode" value=(mode); }
                button.primary type="submit" { "Show" }
            }
            @if modes.len() > 1 {
                nav.tabs aria-label="Mode" {
                    @for (k, l) in &modes {
                        a href=(if *k == "clicks" { base.clone() } else { format!("{base}&mode={k}") }) aria-current=[(*k == mode).then_some("page")] { (l) }
                    }
                }
            }
            @if let Some(href) = overlay {
                p { a.button href=(href) { (icon("play")) "See it over the real page" } }
            } @else {
                p.muted.small.no-replay { (icon("play")) " No recorded visit of this page yet: with session replays on, the map is also drawn over the page as it was recorded." }
            }
            @match mode {
                "scroll" => {
                    section.card.wide aria-label="Scroll depth" {
                        h2 { "How far people scroll" }
                        @if let Some(pg) = page { (scroll_bars(pg)) }
                    }
                },
                _ => {
                    @let rage = mode == "rage";
                    @if !map.cells.iter().any(|c| !rage || c.3 > 0) {
                        section.card { p.muted { @if rage { "No rage click with a position on this page." } @else { "No clicks with a position on this page yet (positions are recorded by SDK 0.1 builds from 2026-11-22 on)." } } }
                    } @else {
                        section.card.wide {
                            (click_svg(map, rage))
                            p.muted.small { "Each dot is a 2% × 20 px area; darker means more " (if rage { "rage clicks" } else { "clicks" }) ". Red marks rage clicks. Clicks from every screen width are drawn together at the median width: the engine keeps one width per page, not per click, so there is no Mobile, Tablet or Desktop view." }
                        }
                    }
                    div.grid {
                        section.card aria-label="Most clicked" {
                            h2 { (if rage { "Most rage-clicked" } else { "Most clicked" }) }
                            @match page { Some(pg) => (ranked_labels(&pg.hotspots, rage)), None => (no_labels) }
                        }
                        @if overlay.is_none() && !rage {
                            section.card aria-label="Scroll depth" {
                                h2 { "How far people scroll" }
                                @match page.filter(|_| has_scroll) {
                                    Some(pg) => (scroll_bars(pg)),
                                    None => p.muted.small { "No scroll measured on this page." },
                                }
                            }
                        }
                    }
                },
            }
        },
    )
}

/// The heatmap drawn over a recorded visit of the page (player page).
pub fn heatmap_overlay(p: &ProjectInfo<'_>, path: &str, session: &str) -> Markup {
    layout(
        "Heatmap",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Click heatmap" }
                p.muted { span.mono { (path) } " · drawn over a recorded visit of this page (text is masked as recorded)" }
            } }
            link rel="stylesheet" href="/assets/player.css";
            section.card.wide {
                div #ol-player
                    data-src={ "/p/" (p.id) "/replays/" (session) "/events.json" }
                    data-heatmap={ "/p/" (p.id) "/heatmap.json?path=" (url_enc(path)) }
                    data-path=(path) {
                    p.muted { "Loading the page…" }
                }
            }
            script src="/assets/player.js" defer {}
        },
    )
}

/// The weekly digest page.
pub fn digest(p: &ProjectInfo<'_>, d: &openlens_engine::Digest, mailed: bool) -> Markup {
    let row = |label: &str, x: &openlens_engine::Delta| {
        let change = x.change();
        html! {
            tr {
                th scope="row" { (label) }
                td.num { strong { (compact(x.now)) } }
                td.num.muted { (compact(x.before)) }
                td.num {
                    @match change {
                        Some(c) if c.abs() >= 0.005 => span class=(if c > 0.0 { "up" } else { "down" }) { (format!("{:+.0}%", c * 100.0)) },
                        Some(_) => span.muted { "same" },
                        None => span.muted { "—" },
                    }
                }
            }
        }
    };
    layout(
        "Weekly digest",
        Some(p),
        html! {
            div.heading { div {
                h1 { "This week vs last" }
                p.muted { "Last 7 days against the 7 before, production. "
                    @if mailed { "Emailed every Monday." } @else { "Set OPENLENS_SMTP_URL and OPENLENS_DIGEST_TO to receive it by email every Monday." } }
            } }
            section.card.wide {
                table.plain {
                    thead { tr { th { span.sr-only { "Metric" } } (th_num("This week")) (th_num("Last week")) (th_num("Change")) } }
                    tbody {
                        (row("Active users", &d.users))
                        (row("Sessions", &d.sessions))
                        (row("Errors", &d.errors))
                        @for (k, t) in &d.tasks { (row(&format!("Completed: {k}"), t)) }
                    }
                }
            }
            @if !d.insights.is_empty() {
                section.card.wide {
                    h2.violet { (icon("idea")) "What to focus on" }
                    ul { @for i in &d.insights { li { strong { (i.title) } " · " span.muted { (i.evidence) } br; span.small { "Next step: " (i.next_step) } } } }
                }
            }
            @if !d.releases.is_empty() {
                section.card.wide {
                    h2 { "Releases this week" }
                    ul.plain-list { @for (name, line) in &d.releases { li { code { (name) } " — " (line) } } }
                }
            }
            @if !d.goals.is_empty() {
                section.card.wide {
                    h2.sun { (icon("star")) "Goals" }
                    ul.plain-list {
                        @for o in &d.objectives_at_risk { li { span.status.bad { "objective at risk" } " " (o) } }
                        @for (line, st) in &d.goals { li { (line) " — " strong { (st) } } }
                    }
                }
            }
        },
    )
}

/// One row of the team page (#216).
pub struct UserRow<'a> {
    /// The user.
    pub user: &'a openlens_meta::User,
    /// `active`, `invited`, `expired` or `disabled`.
    pub status: &'a str,
    /// When the pending invitation expires (UTC), if one is open.
    pub expires: Option<String>,
}

/// The team: invite, find, edit, disable and remove users (#216).
pub fn users(
    rows: &[UserRow<'_>],
    projects: &[String],
    new_link: Option<(&str, &str)>,
    q: &str,
    notice: Option<&str>,
) -> Markup {
    layout(
        "Team",
        None,
        html! {
            div.heading { div {
                h1 { "Team" }
                p.muted { "Admins change things (flags, settings, team); viewers look. The owner (admin token) always has access, so the last admin can't be removed or demoted." }
            } }
            @if let Some(n) = notice { p.notice role="status" { (n) } }
            @if let Some((email, link)) = new_link {
                section.card.wide {
                    h2.mint { (icon("task")) "Login link for " (email) }
                    p { "Send this one-time link (valid 7 days, 24 hours after a password reset):" }
                    pre.snippet { code { (link) } }
                }
            }
            section.card.wide {
                h2.violet { (icon("users")) "Invite someone" }
                form.invite method="post" action="/users" {
                    label { "Email " input type="email" name="email" required; }
                    label { "Name " input type="text" name="name" maxlength="80"; }
                    label { "Role " select name="role" { option value="viewer" { "viewer" } option value="admin" { "admin" } } }
                    label { "Projects " input type="text" name="projects" placeholder="all" list="project-ids"; }
                    datalist #project-ids { @for p in projects { option value=(p) {} } }
                    button type="submit" { "Create the login link" }
                }
                p.muted.small { "Projects: comma-separated ids (" (projects.join(", ")) "), or empty for all." }
            }
            section.card.wide {
                form.inline method="get" action="/users" role="search" {
                    label { "Find " input type="text" name="q" value=(q) placeholder="name or email"; }
                    button.ghost type="submit" { "Search" }
                }
                @if rows.is_empty() { p.muted { @if q.is_empty() { "Nobody invited yet." } @else { "Nobody matches." } } }
                @else {
                    div.table-scroll { table.plain {
                        thead { tr { th { "User" } th { "Role" } th { "Projects" } th { "Status" } th { "Last sign-in" } th { span.sr-only { "Actions" } } } }
                        tbody { @for r in rows {
                            @let u = r.user;
                            tr {
                                td { @if let Some(n) = &u.name { strong { (n) } br; } span.small { (u.email) } }
                                td { span.chip { (u.role) } }
                                td.small { @match &u.projects { Some(p) => (p.join(", ")), None => span.muted { "all" } } }
                                td.small {
                                    span class={ "status " (match r.status { "active" => "good", "disabled" => "bad", _ => "muted" }) } { (r.status) }
                                    @if let Some(e) = &r.expires { br; span.muted { "until " (e) } }
                                }
                                td.small.muted { (u.last_sign_in.as_deref().and_then(|t| t.get(..16)).map(|t| t.replace('T', " ")).unwrap_or_else(|| "never".into())) }
                                td { a.button href={ "/users/" (u.id) } { "Edit" } }
                            }
                        } }
                    } }
                }
            }
        },
    )
}

/// Edit one user: profile, access, sign-in, invitation and removal (#216).
pub fn user_edit(u: &openlens_meta::User, status: &str, projects: &[String]) -> Markup {
    let base = format!("/users/{}", u.id);
    layout(
        "Team",
        None,
        html! {
            div.heading { div {
                h1 { (u.name.as_deref().unwrap_or(&u.email)) }
                p.muted { a href="/users" { "← Team" } " · " (status) }
            } }
            section.card.wide {
                h2.violet { (icon("users")) "Profile and access" }
                form.settings method="post" action=(base) {
                    label { "Name " input type="text" name="name" value=[u.name.as_deref()] maxlength="80"; }
                    label { "Email " input type="email" name="email" value=(u.email) required maxlength="200"; }
                    label { "Role " select name="role" {
                        option value="viewer" selected[u.role == "viewer"] { "viewer" }
                        option value="admin" selected[u.role == "admin"] { "admin" }
                    } }
                    label { "Projects " input type="text" name="projects" value=(u.projects.as_ref().map(|p| p.join(",")).unwrap_or_default()) placeholder="all" list="project-ids"; }
                    datalist #project-ids { @for p in projects { option value=(p) {} } }
                    button type="submit" { "Save" }
                }
                p.muted.small { "Changing the email keeps the user's sessions and history." }
            }
            section.card.wide {
                h2.sky { (icon("shield")) "Sign-in" }
                div.actions {
                    form.inline method="post" action={ (base) "/link" } { button type="submit" { (if status == "invited" || status == "expired" { "Resend the invitation" } else { "New login link" }) } }
                    @if !u.disabled && status != "invited" {
                        form.inline method="post" action={ (base) "/reset-password" } { button.ghost type="submit" { "Reset the password" } }
                    }
                    @if status == "invited" {
                        form.inline method="post" action={ (base) "/revoke" } { button.ghost type="submit" { "Revoke the invitation" } }
                    }
                    @if u.disabled {
                        form.inline method="post" action={ (base) "/enable" } { button type="submit" { "Enable sign-in" } }
                    } @else {
                        form.inline method="post" action={ (base) "/disable" } { button.ghost type="submit" { "Disable sign-in" } }
                    }
                }
                p.muted.small { "Disabling ends their sessions at once and keeps the account; enabling lets them sign in again. Resetting the password ends their sessions and gives you a one-time link (24 h) for them to set a new one; no password is ever shown." }
            }
            section.card.wide.danger {
                h2.coral { (icon("alert")) "Remove" }
                form.inline method="post" action={ (base) "/delete" } {
                    label { "Type " code { (u.email) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                    button.danger type="submit" { "Remove from the team" }
                }
            }
        },
    )
}

/// Instance settings → About (#438): the version, whether a newer one is
/// out, and where the docs are.
pub fn about() -> Markup {
    let update = crate::update_available();
    layout(
        "About",
        None,
        html! {
            div.heading { div {
                h1 { "Instance settings" }
                p.muted { "What runs here, for the whole install." }
            } }
            section.card.wide {
                h2.violet { (icon("idea")) "About" }
                dl.facts {
                    dt { "Version" } dd { "OpenLens " (env!("CARGO_PKG_VERSION")) }
                    dt { "Updates" } dd {
                        @match &update {
                            Some(u) => (crate::update_notice(u)),
                            None => "Up to date, as far as the last daily check knows.",
                        }
                    }
                }
                p.small {
                    a href="/agents.md" { "Wiring guide for agents" } " · "
                    a href="https://github.com/Philippe-arnd/OpenLens/tree/main/docs" { "Documentation" } " · "
                    a href="https://github.com/Philippe-arnd/OpenLens/blob/main/SECURITY.md" { "Security policy" }
                }
            }
        },
    )
}

/// Micro-surveys and their answers.
pub fn surveys(
    p: &ProjectInfo<'_>,
    list: &[openlens_flags::survey::Survey],
    results: &[openlens_engine::SurveyResults],
) -> Markup {
    layout(
        "Surveys",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Micro-surveys" }
                    p.muted { "One question, right after a moment of value, to a sample of users, once each. Create them here or let agents do it (" code { "survey_set" } "); answers are " code { "$survey" } " events." }
                }
                a.button href={ "/p/" (p.id) "/surveys-edit" } { (icon("plus")) "New survey" }
            }
            @if list.is_empty() {
                section.card { p.muted { "No survey yet. Ask your agent, for example: “after entry_saved, ask 10% of users how useful writing felt, 1 to 5”." } }
            }
            @for s in list {
                @let r = results.iter().find(|r| r.key == s.key);
                @let answered: i64 = r.map_or(0, |r| r.answers.iter().map(|a| a.1).sum());
                @let max = r.map_or(1, |r| r.answers.iter().map(|a| a.1).max().unwrap_or(1)).max(1) as f64;
                section.card.wide {
                    h2.violet { (icon("idea")) (s.question) }
                    p.small.muted {
                        span.chip { (s.kind) } " after " code { (s.trigger) } " · " (s.sample) "% of "
                        @if s.roles.is_empty() { "everyone" } @else { (s.roles.join(", ")) }
                        " · " (compact(answered)) " answers"
                        @if let Some(r) = r { " · " (compact(r.dismissed)) " dismissed" }
                        " · " a href={ "/p/" (p.id) "/surveys-edit?key=" (s.key) } { "Edit" }
                    }
                    form.inline method="post" action={ "/p/" (p.id) "/surveys/" (s.key) } {
                        label { input type="checkbox" name="enabled" checked[s.enabled]; " on" }
                        label { "sample " input type="number" name="sample" min="0" max="100" value=(s.sample); "%" }
                        button.soft type="submit" { "Save" }
                    }
                    @if let Some(r) = r {
                        @if let Some(m) = r.mean { p { "Mean " strong { (format!("{m:.1}")) } " / 5" } }
                        @if s.kind != "text" && !r.answers.is_empty() {
                            table.bars { tbody { @for (a, n) in &r.answers {
                                tr { th scope="row" { (a) } td.meter { (meter(*n as f64 / max)) } td.num { (compact(*n)) } }
                            } } }
                        }
                        @if !r.by_role.is_empty() {
                            p.small { "By role: " @for (i, (role, m, n)) in r.by_role.iter().enumerate() { @if i > 0 { " · " } strong { (role) } " " (format!("{m:.1}")) span.muted { " (" (n) ")" } } }
                        }
                        @if !r.latest_text.is_empty() {
                            h3.small { "Latest answers" }
                            ul { @for t in &r.latest_text { li { q { (t) } } } }
                        }
                    }
                }
            }
        },
    )
}

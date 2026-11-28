//! Flags and surveys, pages, errors, replays and the player.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;
use crate::tables::Col;

/// A flag's rollout as a bar and its share (#551): the bar's width is an
/// SVG attribute (the CSP forbids `style`), 100 % fills the track.
fn rollout_bar(rollout: u8) -> Markup {
    let w = f64::from(rollout.min(100)).max(1.5);
    html! {
        div.barcell {
            svg viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true" focusable="false" {
                rect.bg width="100" height="6" rx="3" {}
                rect.fg width=(format!("{w:.1}")) height="6" rx="3" {}
            }
            span.num { (rollout) " %" }
        }
    }
}

/// Feature flags (#551): a table with an on/off tag and the rollout as a
/// bar, each row keeping its two forms (the rollout, the switch).
/// Creating flags is done by agents (API/CLI/MCP) with the code that uses
/// them, or from the editor.
pub fn flags(p: &ProjectInfo<'_>, flags: &[openlens_flags::Flag]) -> Markup {
    layout(
        "Flags",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Flags" }
                    p.muted { "Ship a change to some users first, then compare. Each flag's users show up as their own cohort." }
                }
                a.btn href={ "/p/" (p.id) "/flags-edit" } { (icon("plus")) "New flag" }
            }
            @if flags.is_empty() {
                section.card.empty aria-labelledby="no-flags" {
                    (crate::tables::empty_art())
                    div {
                        h2 #no-flags { "No flags yet" }
                        p { "Your agent creates them together with the code that checks them, for example:" }
                        pre.snippet { code { "openlens-cli flags set new-editor --rollout 10" } }
                        div.actions { a.btn.primary href={ "/p/" (p.id) "/flags-edit" } { (icon("plus")) "New flag" } }
                    }
                }
            } @else {
                (crate::tables::TableCard {
                    label: "Flags",
                    head: html! {},
                    cols: &[
                        Col::Shown("Flag"), Col::Shown("Who"), Col::Shown("Rollout"),
                        Col::Shown("State"), Col::Hidden("Actions"),
                    ],
                    body: html! { @for f in flags {
                        tr {
                            th scope="row" { a href={ "/p/" (p.id) "/flags-edit?key=" (f.key) } title="Edit" { code { (f.key) } } @if !f.description.is_empty() { br; span.muted.small { (f.description) } } }
                            td.small { @if f.roles.is_empty() { "every role" } @else { (f.roles.join(", ")) }
                                @if !f.accounts.is_empty() { br; span.muted { "+ " (f.accounts.len()) " accounts" } }
                                @for r in &f.rules { br; code { (r) } } }
                            td.rollout {
                                (rollout_bar(f.rollout))
                                form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) } {
                                    @if f.enabled { input type="hidden" name="enabled" value="on"; }
                                    input type="number" name="rollout" min="0" max="100" value=(f.rollout) aria-label={ "Rollout percentage of " (f.key) };
                                    " % " button.ghost type="submit" { "Save" }
                                }
                            }
                            td { @if f.enabled { span.tag.tone-good { (icon("task")) "on" } } @else { span.tag { (icon("ban")) "off" } } }
                            td { form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) } {
                                @if !f.enabled { input type="hidden" name="enabled" value="on"; }
                                input type="hidden" name="rollout" value=(f.rollout);
                                button type="submit" { (if f.enabled { "Switch off" } else { "Switch on" }) }
                            } }
                        }
                    } },
                    foot: html! { div.pager { span { (flags.len()) " flags · the rollout is the share of eligible users who get the flag" } } },
                }
                .render())
            }
        },
    )
}

/// Create or edit a flag (#220). `flag` is None for a new one. Delete
/// sits in the danger zone; `confirm` opens its dialog (#545).
pub fn flag_edit(
    p: &ProjectInfo<'_>,
    flag: Option<&openlens_flags::Flag>,
    error: Option<&str>,
    readout: &[openlens_engine::VariantRow],
    confirm: Option<Confirm>,
) -> Markup {
    use crate::detail::{confirm_dialog, confirm_href, danger_item, danger_zone, typed_form};
    let new = flag.is_none_or(|f| f.key.is_empty());
    let f = flag.cloned().unwrap_or(openlens_flags::Flag {
        key: String::new(),
        description: String::new(),
        enabled: false,
        rollout: 0,
        roles: Vec::new(),
        accounts: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        users: Vec::new(),
        holdout: 0,
    });
    let action = format!("/p/{}/flags-edit", p.id);
    let variants = f
        .variants
        .iter()
        .map(|v| format!("{}:{}", v.key, v.weight))
        .collect::<Vec<_>>()
        .join(", ");
    layout(
        "Flags",
        Some(p),
        html! {
            div.heading { div {
                h1 { @if new { "New flag" } @else { "Flag " code { (f.key) } } }
                p.muted { a href={ "/p/" (p.id) "/flags" } { "← Flags" } " · the code checks it with " code { "openlens.isEnabled(\"" (if f.key.is_empty() { "key" } else { &f.key }) "\")" } }
            } }
            @if let Some(e) = error { p.alert role="alert" { (e) } }
            section.card.wide {
                form.settings method="post" action=(action) {
                    input type="hidden" name="original" value=(if new { "" } else { flag.map_or("", |x| x.key.as_str()) });
                    label { "Key " input type="text" name="key" value=(f.key) required maxlength="64" pattern="[a-z0-9_-]+" placeholder="new-editor"; }
                    label { "What it is for " input type="text" name="description" value=(f.description) maxlength="200"; }
                    label { "Rollout % " input type="number" name="rollout" min="0" max="100" value=(f.rollout); }
                    label { "Only roles " input type="text" name="roles" value=(f.roles.join(", ")) placeholder="every role"; }
                    label { "Always on for accounts " input type="text" name="accounts" value=(f.accounts.join(", ")) placeholder="none"; }
                    label.wide { "Variants (experiment) " input type="text" name="variants" value=(variants) placeholder="control:50, short:50"; }
                    label.wide { "Only users whose properties match (one rule per line, all must match) "
                        textarea name="rules" rows="3" spellcheck="false" placeholder="plan in pro, team\ncountry not in FR" { (f.rules.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")) } }
                    label { "Holdout % " input type="number" name="holdout" min="0" max="50" value=(f.holdout); }
                    @if !f.users.is_empty() { p.small.wide { "Only " strong { (f.users.len()) } " listed users (made from the churn page's users at risk); " (f.holdout) "% of them are kept off as a control group." } }
                    label.check { input type="checkbox" name="enabled" checked[f.enabled]; " On" }
                    button type="submit" { (if new { "Create" } else { "Save" }) }
                }
                p.muted.small { "Roles are the feature map's canonical roles; lists are comma-separated. Users in the rollout keep their bucket when you raise it. Variants split the users who get the flag (weights add up to 100); the code reads " code { "openlens.variant(\"key\")" } " and events carry the variant. Rules test " code { "plan" } ", " code { "country" } " (from the address when the app does not send it), " code { "role" } ", " code { "account" } " or any property passed to " code { "identify" } "; a user who did not send the property does not get the flag." }
            }
            @if !readout.is_empty() {
                section.card.wide {
                    h2.violet { (icon("funnel")) "Variants, last 30 days" }
                    div.table-scroll { table.plain {
                        thead { tr { th { "Variant" } (th_num("Users")) (th_num("Reached value")) (th_def("vs baseline")) (th_num("Sessions with errors")) (th_def("vs baseline")) } }
                        tbody { @for r in readout {
                            tr {
                                th scope="row" { code { (r.variant) } @if r.baseline { " " span.chip { "baseline" } } @if r.users < 30 { " " span.chip { "low sample" } } }
                                td.num { (compact(r.users)) }
                                td.num { strong { (pct(r.value_rate)) } }
                                td.small { (versus(r.baseline, r.value_p, r.value_significant())) }
                                td.num { (pct(r.error_rate)) }
                                td.small { (versus(r.baseline, r.error_p, r.error_significant())) }
                            }
                        } }
                    } }
                    p.muted.small { "Reached value: a success event of the feature map (any feature use when none is declared). Each arm is compared with the baseline (" code { "control" } ", else the first variant) with a two-proportion test; a difference counts when p is under " (readout.first().map_or(0.05, |r| r.alpha)) ", 0.05 shared by the arms compared (Bonferroni). Under 30 users or sessions: too few to tell." }
                }
            }
            @if !new {
                @let page = format!("/p/{}/flags-edit?key={}", p.id, url_enc(&f.key));
                (danger_zone("flag-danger", "Danger zone", danger_item("Delete the flag",
                    html! { "The code then gets " code { "false" } " for it. Its past cohort stays in the data." },
                    html! { a.btn.danger-btn href=(confirm_href(&page, Confirm::Delete)) { "Delete…" } })))
                @if confirm == Some(Confirm::Delete) {
                    (confirm_dialog(&format!("Delete the flag {}?", f.key),
                        html! { "The code then gets " code { "false" } " for it. Its past cohort stays in the data." },
                        typed_form(&format!("/p/{}/flags/{}/delete", p.id, f.key), &f.key, "Delete", &page)))
                }
            }
        },
    )
}

/// The answer types of a survey: (form value, label).
pub(crate) const SURVEY_KINDS: [(&str, &str); 4] = [
    ("rating", "Rating 1–5"),
    ("yes_no", "Yes / no"),
    ("choice", "One choice"),
    ("text", "Free text"),
];

/// The answer type checked in the editor: the stored one, or a rating for
/// a value the form does not offer, so exactly one option is checked.
fn survey_kind(kind: &str) -> &'static str {
    SURVEY_KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("rating", |(k, _)| k)
}

/// Create or edit a micro-survey (#220).
pub fn survey_edit(
    p: &ProjectInfo<'_>,
    survey: Option<&openlens_flags::survey::Survey>,
    error: Option<&str>,
) -> Markup {
    let new = survey.is_none_or(|s| s.key.is_empty());
    let s = survey.cloned().unwrap_or(openlens_flags::survey::Survey {
        key: String::new(),
        question: String::new(),
        kind: "rating".into(),
        choices: Vec::new(),
        trigger: String::new(),
        enabled: false,
        sample: 10,
        roles: Vec::new(),
    });
    layout(
        "Surveys",
        Some(p),
        html! {
            div.heading { div {
                h1 { @if new { "New survey" } @else { "Survey " code { (s.key) } } }
                p.muted { a href={ "/p/" (p.id) "/surveys" } { "← Surveys" } " · one question, right after a moment of value, once per user" }
            } }
            @if let Some(e) = error { p.alert role="alert" { (e) } }
            // The form card (#551): the answer type is a segmented control,
            // radio inputs carrying the select's name and values.
            form.card.wide.form-card method="post" action={ "/p/" (p.id) "/surveys-edit" } aria-labelledby="survey-h" {
                input type="hidden" name="original" value=(if new { "" } else { survey.map_or("", |x| x.key.as_str()) });
                div.section-head { h2 #survey-h { (icon("idea")) "Question" } }
                div.form-grid {
                    label.field { "Key" input type="text" name="key" value=(s.key) required maxlength="64" pattern="[a-z0-9_-]+" placeholder="writing-usefulness"; small { "Lowercase letters, digits, - and _." } }
                    label.field.wide { "Question" input type="text" name="question" value=(s.question) required maxlength="200" placeholder="How useful was writing today?"; }
                    fieldset.field.wide {
                        legend { "Answer" }
                        div.seg {
                            @for (k, l) in SURVEY_KINDS {
                                label { input type="radio" name="kind" value=(k) checked[survey_kind(&s.kind) == k]; (l) }
                            }
                        }
                    }
                    label.field { "Choices" input type="text" name="choices" value=(s.choices.join(", ")) placeholder="for One choice: 2 to 6"; small { "One choice only: 2 to 6, comma-separated." } }
                    label.field { "After the event" input type="text" name="trigger" value=(s.trigger) required maxlength="100" placeholder="entry_saved"; small { "Asked right after this event, once per user." } }
                    label.field { "Ask % of users" input type="number" name="sample" min="0" max="100" value=(s.sample); }
                    label.field { "Only roles" input type="text" name="roles" value=(s.roles.join(", ")) placeholder="every role"; small { "Comma-separated. Empty: every role." } }
                }
                label.switch {
                    input type="checkbox" role="switch" name="enabled" checked[s.enabled];
                    span { b { "On" } small { "Off: nobody is asked." } }
                }
                (crate::project_settings::form_foot(if new { "Create" } else { "Save" }, html! {
                    "Answers are " code { "$survey" } " events: " a href={ "/p/" (p.id) "/events?event=%24survey" } { "see them in Events" } ", export them from Settings."
                }))
            }
            @if !new {
                section.card.wide.danger {
                    h2.coral { (icon("alert")) "Delete the survey" }
                    p { "Nobody is asked any more. The answers already given stay (they are events)." }
                    form.inline method="post" action={ "/p/" (p.id) "/surveys/" (s.key) "/delete" } {
                        label { "Type " code { (s.key) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                        button.danger type="submit" { "Delete" }
                    }
                }
            }
        },
    )
}

/// The heatmap of one page.
pub(crate) fn heatmap_href(pid: &str, path: &str) -> String {
    format!("/p/{pid}/heatmap?path={}", url_enc(path))
}

/// Per-page hotspots (#548: one table): views as a bar, scroll depth, the
/// most clicked element, rage clicks, the Web Vitals measured on the page
/// (`vitals`: the vitals report, whose worst pages carry a p75 per page)
/// and a link to the page's heatmap, under the Web Vitals of the whole
/// site (the board's card until #620).
pub fn pages(
    p: &ProjectInfo<'_>,
    b: &Board,
    list: &[openlens_engine::PageStats],
    vitals: &[openlens_engine::VitalReport],
    f: &Filters,
) -> Markup {
    let max = list.iter().map(|x| x.views).max().unwrap_or(1).max(1) as f64;
    let page_vitals = |path: &str| -> Vec<&openlens_engine::PageVital> {
        vitals
            .iter()
            .flat_map(|r| r.worst_pages.iter())
            .filter(|v| v.path == path)
            .collect()
    };
    layout_in(
        "Pages",
        Some(p),
        context_bar(p.id, b, f, "/pages", false),
        html! {
            div.heading { div { h1 { "Pages" } p.muted { "How far people scroll, what they click, where they get frustrated. Last 30 days; Web Vitals: p75 over 7 days, on the slowest pages." } } }
            (crate::vitals::web_vitals(p.id, f, vitals))
            @if list.is_empty() {
                section.card { p.muted { "No page views yet." } }
            } @else {
                (crate::tables::TableCard {
                    label: "Pages",
                    head: html! {},
                    cols: &[
                        Col::Shown("Page"), Col::Shown("Views"), Col::Num("Users"), Col::Num("Reach half"),
                        Col::Num("Reach the end"), Col::Shown("Most clicked"), Col::Num("Rage clicks"),
                        Col::Shown("Web Vitals"), Col::Hidden("Heatmap"),
                    ],
                    body: html! { @for pg in list {
                        @let rage: i64 = pg.hotspots.iter().map(|h| h.rage).sum();
                        tr {
                            (crate::tables::path_cell(Some(&pg.path)))
                            td.views { (crate::board::bar_with(pg.views as f64 / max, &compact(pg.views))) }
                            td.num { (compact(pg.users)) }
                            @if pg.median_scroll >= 0.0 {
                                td.num { (pct(pg.reach_half)) }
                                td.num { (pct(pg.reach_end)) }
                            } @else {
                                td.num.muted { "—" }
                                td.num.muted { "—" }
                            }
                            td { @match pg.hotspots.first() {
                                Some(h) => { (h.label) " " span.muted { "(" (compact(h.clicks)) ")" } },
                                None => span.muted { "—" },
                            } }
                            td.num { @if rage > 0 { span.status.bad { (icon("alert")) (compact(rage)) } } @else { span.muted { "0" } } }
                            td.vitals { @let vs = page_vitals(&pg.path);
                                @if vs.is_empty() { span.muted { "—" } }
                                @for v in vs {
                                    span class={ "vital " (match v.rating { "good" => "good", "poor" => "bad", _ => "warn" }) } title={ (v.samples) " measurements" } {
                                        (v.name) " " (openlens_engine::vitals::show(&v.name, v.p75))
                                    }
                                    " "
                                }
                            }
                            td { a.btn.sm href=(heatmap_href(p.id, &pg.path)) { (icon("spark")) "Heatmap" } }
                        }
                    } },
                    foot: html! {},
                }
                .render())
            }
        },
    )
}

/// Where an error group was raised (#624): a tag.
fn origin_tag(server: bool) -> Markup {
    html! {
        @if server {
            span.tag.tone-accent title="Raised on a server, sent by a server SDK" { (icon("code")) "Server" }
        } @else {
            span.tag.tone-info title="Raised in the browser" { (icon("globe")) "Browser" }
        }
    }
}

/// The browser sessions of a server error group (#624): a replay link
/// when the session was recorded, else its user; or why there are none.
fn linked_sessions(pid: &str, g: &openlens_engine::ErrorGroup) -> Markup {
    html! {
        @if g.linked.is_empty() {
            span.muted.small { "No browser session linked: the requests carried no OpenLens session and no trace id the browser also sent." }
        } @else {
            span.small { "Browser sessions: " }
            @for (i, s) in g.linked.iter().enumerate() {
                @if i > 0 { ", " }
                @if s.replay {
                    a.small href={ "/p/" (pid) "/replays/" (url_enc(&s.session)) "?at=" (url_enc(&s.last_seen)) } title="Replay this session" {
                        (icon("play")) " " (s.user.as_deref().unwrap_or("Replay"))
                    }
                } @else {
                    span.small.mono title="Not recorded" { (s.user.as_deref().unwrap_or(s.session.get(..8).unwrap_or(&s.session))) }
                }
            }
        }
    }
}

/// Error groups, most users first, of one origin or all (#624).
pub fn errors(
    p: &ProjectInfo<'_>,
    b: &Board,
    groups: &[openlens_engine::ErrorGroup],
    original: &[Option<openlens_errors::Original>],
    f: &Filters,
    origin: openlens_engine::Origin,
    backend: ErrorsBackend<'_>,
) -> Markup {
    use openlens_engine::Origin;
    let list = |v: &[(String, i64)]| {
        v.iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let action = match origin {
        Origin::All => "/errors".to_owned(),
        o => format!("/errors?origin={}", o.as_str()),
    };
    let origin_href = |o: Origin| match o {
        Origin::All => crate::explore::filtered_href(p.id, "/errors", f, &[]),
        o => crate::explore::filtered_href(p.id, "/errors", f, &[("origin", o.as_str())]),
    };
    let none = match origin {
        Origin::All => "No errors in the last 30 days. Nice.",
        Origin::Browser => "No browser errors in the last 30 days.",
        Origin::Server => {
            "No server errors in the last 30 days. A server SDK sends them (see sdk/SERVER.md)."
        }
    };
    layout_in(
        "Errors",
        Some(p),
        context_bar(p.id, b, f, &action, false),
        html! {
            div.heading {
                div { h1 { "Errors" } p.muted { "Uncaught errors grouped by cause, most users first, last 30 days. A server error links to the browser sessions that made the request." } }
            }
            nav.tabs aria-label="Origin" {
                @for (o, label) in [(Origin::All, "All"), (Origin::Browser, "Browser"), (Origin::Server, "Server")] {
                    a href=(origin_href(o)) aria-current=[(o == origin).then_some("page")] { (label) }
                }
            }
            @if groups.is_empty() {
                section.card { p { (icon("task")) " " (none) } }
            } @else {
                (crate::tables::TableCard {
                    label: "Error groups",
                    head: html! {},
                    cols: &[
                        Col::Shown("Error"), Col::Num("Users"), Col::Num("Sessions"), Col::Num("Times"),
                        Col::Shown("Roles"), Col::Shown("Releases"), Col::Shown("Where"), Col::Shown("Last seen"),
                        Col::Shown("Last user"), Col::Hidden("Replay"),
                    ],
                    body: html! { @for (i, g) in groups.iter().enumerate() {
                        tr {
                            th scope="row" {
                                span.ev.ev-error title="Error" { (icon("alert")) span.sr-only { "Error: " } (g.sample.get(..160).unwrap_or(&g.sample)) }
                                " " (origin_tag(g.server))
                                br;
                                @if g.server { (linked_sessions(p.id, g)) br; }
                                a.small href={ "/p/" (p.id) "/tickets/new?kind=error&subject=" (url_enc(&g.fingerprint)) } { "Create ticket →" } br;
                                @if let Some(Some(o)) = original.get(i) {
                                    span.small.mono title="Resolved with the release's source map" {
                                        (icon("route")) " " (o.source) ":" (o.line) @if let Some(n) = &o.name { " · " (n) }
                                    }
                                } @else {
                                    @let place = g.fingerprint.rsplit(" @ ").next().unwrap_or("");
                                    @if place != "?" { span.muted.small.mono { (place) } }
                                }
                            }
                            td.num { strong { (compact(g.users)) } }
                            td.num { (compact(g.sessions)) }
                            td.num.muted { (compact(g.count)) }
                            td.small { (list(&g.roles)) }
                            td.small { (list(&g.releases)) }
                            td.path { (list(&g.places)) }
                            td.when.muted { (local_time(&g.last_seen, &f.tz, false)) }
                            td { @if let Some(u) = &g.last_user { (crate::tables::user_cell(p.id, u)) } }
                            td { @if let Some(s) = &g.last_session {
                                a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (g.last_seen) } title="Replay the latest occurrence" aria-label="Replay the latest occurrence" { (icon("play")) }
                            } }
                        }
                    } },
                    foot: html! { div.pager { span { (groups.len()) " groups · most users first · times in " (f.tz) } } },
                }
                .render())
            }
            @if let Some(r) = backend.routes.filter(|r| !r.routes.is_empty()) {
                (server_routes(p.id, r))
            }
            @if let Some(d) = backend.dependencies {
                (crate::dependencies_table::dependencies(d))
            }
        },
    )
}

/// The server routes on the Errors page (#594): one line and a link, the
/// table lives on Quality › Routes (#622).
fn server_routes(pid: &str, r: &openlens_engine::commands::routes::Routes) -> Markup {
    html! {
        section.card { p {
            (icon("route")) " Server: " strong { (pct(r.errors as f64 / r.requests.max(1) as f64)) }
            " of " (compact(r.requests)) " requests answered 5xx over the last " (r.days) " days"
            @if r.worse > 0 {
                ", " strong { (r.worse) } " " (if r.worse == 1 { "route" } else { "routes" }) " worse than the release before"
            }
            ". " a href={ "/p/" (pid) "/routes" } { "Routes →" }
        } }
    }
}

/// Recorded sessions, most recent or most active first, optionally only
/// those with more than some active time (#404).
pub fn replays(
    p: &ProjectInfo<'_>,
    list: &[openlens_replay::Summary],
    who: Option<&str>,
    view: openlens_replay::View,
    tz: &str,
    comments: &[(String, i64)],
    rate: &openlens_engine::ReplayRate,
) -> Markup {
    let min_minutes = view.min_active_secs / 60;
    let cols = [
        Col::Shown("Started"),
        Col::Shown("User"),
        Col::Num("Active time"),
        Col::Num("Size"),
        Col::Num("Comments"),
        Col::Hidden("Play"),
    ];
    // (UTC start as `local_time` reads it, the recording).
    let rows: Vec<(String, &openlens_replay::Summary)> = list
        .iter()
        .map(|r| (r.started.format("%Y-%m-%d %H:%M:%S").to_string(), r))
        .collect();
    let row = |(_, r): &(String, &openlens_replay::Summary), time: &str| {
        let secs = (r.updated - r.started).num_seconds().max(0) as f64;
        html! {
            tr {
                td.when.muted { (time) }
                td { a.mono href={ "/p/" (p.id) "/replays?u=" (url_enc(&r.distinct_id)) } title="Only this user's recordings" { (r.distinct_id) } }
                td.num { @match r.active_secs {
                    Some(a) if (a as f64) + 60.0 < secs => { (duration(a as f64)) " active" br; span.muted.small { "open " (duration(secs)) } }
                    Some(a) => { (duration(a.max(1) as f64)) }
                    None => { (duration(secs)) }
                } }
                td.num.muted { (compact((r.bytes / 1024) as i64)) " kB" }
                td.num { @match comments.iter().find(|(s, _)| *s == r.session_id) {
                    Some((_, n)) => { (n) span.sr-only { " comments" } },
                    None => span.muted { "—" },
                } }
                td { a.btn.sm href={ "/p/" (p.id) "/replays/" (r.session_id) } { (icon("play")) "Play" } }
            }
        }
    };
    // Day groups only when the rows are in time order.
    let body = if view.by_active {
        html! { @for x in &rows { (row(x, &local_time(&x.0, tz, false))) } }
    } else {
        crate::tables::day_rows(&rows, |x| x.0.as_str(), tz, cols.len(), row)
    };
    layout(
        "Replays",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Replays" }
                    p.muted {
                        "Recorded sessions, " (if view.by_active { "most active first" } else { "most recent first" }) ". Inputs are always masked."
                    }
                    p.muted { (rate_reason(rate)) }
                }
            }
            form.card.refine method="get" action={ "/p/" (p.id) "/replays" } aria-label="Sort and filter" {
                @if let Some(w) = who { input type="hidden" name="u" value=(w); }
                label { "Sort"
                    select name="sort" {
                        option value="recent" selected[!view.by_active] { "Most recent first" }
                        option value="active" selected[view.by_active] { "Most active first" }
                    }
                }
                label { "More than (min active)"
                    input type="number" name="active" min="0" max="1440" step="1" value=[(min_minutes > 0).then_some(min_minutes)] placeholder="0";
                }
                button type="submit" { "Show" }
            }
            @if let Some(w) = who {
                p.active-filters { "Only " strong { (w) } a href={ "/p/" (p.id) "/replays" } aria-label="Show everyone" { "Show everyone ×" } }
            }
            @if list.is_empty() {
                section.card.empty aria-labelledby="rep-empty-h" {
                    (crate::tables::empty_art())
                    div {
                        @if min_minutes > 0 {
                            h2 #rep-empty-h { "No recording has more than " (min_minutes) " min of active time" }
                            p { "Lower the active time to see shorter sessions." }
                            div.actions { a.btn.primary href={ "/p/" (p.id) "/replays" } { "Show every recording" } }
                        } @else {
                            h2 #rep-empty-h { "No recordings yet" }
                            p { "The SDK records sessions once a page has loaded; they show up here within a few seconds." }
                            div.actions { a.btn.primary href={ "/p/" (p.id) "/settings/install" } { "Check the install" } }
                        }
                    }
                }
            } @else {
                (crate::tables::TableCard {
                    label: "Recorded sessions",
                    head: html! {},
                    cols: &cols,
                    body,
                    foot: crate::tables::pager(&format!("{} recordings · times in {}", list.len(), tz_label(tz)), &[]),
                }
                .render())
            }
        },
    )
}

/// Why sessions are recorded at the current rate (#327, D-035).
fn rate_reason(r: &openlens_engine::ReplayRate) -> String {
    let budget = r.budget;
    let triggers = "plus every session with an error or a rage click";
    if r.rate < 1.0 {
        let pct = r.rate * 100.0;
        let share = if pct >= 10.0 {
            format!("{pct:.0}")
        } else {
            format!("{pct:.1}")
        };
        format!(
            "{budget} recordings a day budget: {share} % of yesterday's {} active users, {triggers}.",
            crate::compact(r.active_users)
        )
    } else if r.day.is_empty() {
        format!(
            "Every session is recorded until a day of activity is counted. Above {budget} active users a day, a share is recorded, {triggers}."
        )
    } else {
        format!(
            "Every session is recorded: yesterday's {} active users fit the {budget} recordings a day budget. Above it, a share is recorded, {triggers}.",
            crate::compact(r.active_users)
        )
    }
}

/// The replay player page. The player script (rrweb Replayer, MIT) is the
/// only script the dashboard serves (D-011).
pub fn player(
    p: &ProjectInfo<'_>,
    session: &str,
    who: Option<&str>,
    at: Option<&str>,
    t: Option<u64>,
    comments: &[openlens_meta::ReplayComment],
    me: Option<(&str, bool)>,
) -> Markup {
    let base = format!("/p/{}/replays/{}", p.id, session);
    let data: Vec<serde_json::Value> = comments
        .iter()
        .map(|c| serde_json::json!({ "id": c.id, "ms": c.at_ms, "body": c.body, "author": c.author }))
        .collect();
    let data = serde_json::Value::Array(data).to_string();
    layout(
        "Replay",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Replay" }
                    p.muted {
                        "Session " span.mono { (session) }
                        @if let Some(w) = who { " · " a href={ "/p/" (p.id) "/replays?u=" (w) } { (w) } }
                    }
                }
            }
            link rel="stylesheet" href="/assets/player.css";
            section.card.wide {
                div #ol-player data-src={ "/p/" (p.id) "/replays/" (session) "/events.json" } data-timeline={ "/p/" (p.id) "/replays/" (session) "/timeline.json" } data-at=[at] data-t=[t] data-comments=(data) {
                    p.muted { "Loading the recording…" }
                }
            }
            section.card.wide.comments {
                h2.violet { (icon("idea")) "Comments" span.muted.small { " · pinned to a moment" } }
                @if comments.is_empty() { p.muted.small { "None yet. Pause where something happens and say what you see." } }
                @else {
                    ol.comment-list {
                        @for c in comments {
                            li id={ "c" (c.id) } {
                                a.ol-c-at href={ (base) "?t=" (c.at_ms) "#c" (c.id) } { (duration(c.at_ms as f64 / 1000.0)) }
                                " "
                                span.comment-body { (c.body) }
                                span.muted.small { " — " (c.author) " · " (c.created_at.get(..10).unwrap_or(&c.created_at)) }
                                @if me.is_some_and(|(id, admin)| admin || id == c.user_id) {
                                    form.inline method="post" action={ (base) "/comments/" (c.id) "/delete" } {
                                        button.ghost type="submit" aria-label="Delete this comment" { "Delete" }
                                    }
                                }
                            }
                        }
                    }
                }
                form #ol-comment.comment-form method="post" action={ (base) "/comments" } {
                    input type="hidden" name="at" value=(t.unwrap_or(0));
                    label for="ol-comment-body" { "Comment at the current moment" }
                    textarea #ol-comment-body name="body" required maxlength="2000" rows="2" placeholder="The form error shows here" {}
                    button type="submit" { "Add" }
                }
            }
            script src="/assets/player.js" defer {}
        },
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_rate_reason_names_the_budget_in_force() {
        let floor = openlens_engine::replay_rate::DAILY_BUDGET;
        let r = openlens_engine::ReplayRate {
            rate: 0.5,
            active_users: 10_000,
            day: "2026-11-26".into(),
            budget: floor * 3,
            sample_rate: 0.2,
        };
        let why = super::rate_reason(&r);
        assert!(why.contains(&r.budget.to_string()), "{why}");
        assert!(!why.contains(&format!("{floor} recordings")), "{why}");
    }
}

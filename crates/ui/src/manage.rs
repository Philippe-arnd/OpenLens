//! Flags and surveys, pages, errors, replays and the player.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// Feature flags: switch on/off and change the rollout. Creating flags is
/// done by agents (API/CLI/MCP) with the code that uses them.
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
                a.button href={ "/p/" (p.id) "/flags-edit" } { (icon("plus")) "New flag" }
            }
            @if flags.is_empty() {
                section.card { p { "No flags yet. Your agent creates them together with the code that checks them: "
                    code { "openlens-cli flags set new-editor --rollout 10" } "." } }
            } @else {
                section.card.wide {
                    table.plain {
                        thead { tr { th { "Flag" } th { "Who" } th { "Rollout" } th { "On" } th { span.sr-only { "Actions" } } } }
                        tbody { @for f in flags {
                            tr {
                                th scope="row" { a href={ "/p/" (p.id) "/flags-edit?key=" (f.key) } title="Edit" { code { (f.key) } } @if !f.description.is_empty() { br; span.muted.small { (f.description) } } }
                                td.small { @if f.roles.is_empty() { "every role" } @else { (f.roles.join(", ")) }
                                    @if !f.accounts.is_empty() { br; span.muted { "+ " (f.accounts.len()) " accounts" } }
                                    @for r in &f.rules { br; code { (r) } } }
                                td { form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) } {
                                    @if f.enabled { input type="hidden" name="enabled" value="on"; }
                                    input type="number" name="rollout" min="0" max="100" value=(f.rollout) aria-label={ "Rollout percentage of " (f.key) };
                                    " % " button.ghost type="submit" { "Save" }
                                } }
                                td { span class=(if f.enabled { "status good" } else { "status muted" }) {
                                    span aria-hidden="true" { (if f.enabled { "●" } else { "○" }) } " " (if f.enabled { "on" } else { "off" }) } }
                                td { form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) } {
                                    @if !f.enabled { input type="hidden" name="enabled" value="on"; }
                                    input type="hidden" name="rollout" value=(f.rollout);
                                    button type="submit" { (if f.enabled { "Switch off" } else { "Switch on" }) }
                                } }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// Create or edit a flag (#220). `flag` is None for a new one.
pub fn flag_edit(
    p: &ProjectInfo<'_>,
    flag: Option<&openlens_flags::Flag>,
    error: Option<&str>,
    readout: &[openlens_engine::VariantRow],
) -> Markup {
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
                section.card.wide.danger {
                    h2.coral { (icon("alert")) "Delete the flag" }
                    p { "The code then gets " code { "false" } " for it. Its past cohort stays in the data." }
                    form.inline method="post" action={ "/p/" (p.id) "/flags/" (f.key) "/delete" } {
                        label { "Type " code { (f.key) } " to confirm " input type="text" name="confirm" required autocomplete="off"; }
                        button.danger type="submit" { "Delete" }
                    }
                }
            }
        },
    )
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
            section.card.wide {
                form.settings method="post" action={ "/p/" (p.id) "/surveys-edit" } {
                    input type="hidden" name="original" value=(if new { "" } else { survey.map_or("", |x| x.key.as_str()) });
                    label { "Key " input type="text" name="key" value=(s.key) required maxlength="64" pattern="[a-z0-9_-]+" placeholder="writing-usefulness"; }
                    label.wide { "Question " input type="text" name="question" value=(s.question) required maxlength="200" placeholder="How useful was writing today?"; }
                    label { "Answer "
                        select name="kind" {
                            @for (k, l) in [("rating", "Rating 1–5"), ("yes_no", "Yes / no"), ("choice", "One choice"), ("text", "Free text")] {
                                option value=(k) selected[s.kind == k] { (l) }
                            }
                        }
                    }
                    label { "Choices " input type="text" name="choices" value=(s.choices.join(", ")) placeholder="for One choice: 2 to 6"; }
                    label { "After the event " input type="text" name="trigger" value=(s.trigger) required maxlength="100" placeholder="entry_saved"; }
                    label { "Ask % of users " input type="number" name="sample" min="0" max="100" value=(s.sample); }
                    label { "Only roles " input type="text" name="roles" value=(s.roles.join(", ")) placeholder="every role"; }
                    label.check { input type="checkbox" name="enabled" checked[s.enabled]; " On" }
                    button type="submit" { (if new { "Create" } else { "Save" }) }
                }
                p.muted.small { "Answers are " code { "$survey" } " events: " a href={ "/p/" (p.id) "/events?event=%24survey" } { "see them in Events" } ", export them from Settings." }
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

/// Per-page hotspots: scroll depth, most clicked elements, rage clicks.
pub fn pages(
    p: &ProjectInfo<'_>,
    b: &Board,
    list: &[openlens_engine::PageStats],
    f: &Filters,
) -> Markup {
    layout(
        "Pages",
        Some(p),
        html! {
            div.heading { div { h1 { "Pages" } p.muted { "How far people scroll, what they click, where they get frustrated. Last 30 days." } } }
            (filter_bar(p.id, b, f, "/pages"))
            @if list.is_empty() { section.card { p.muted { "No page views yet." } } }
            div.grid {
                @for pg in list {
                    section.card {
                        h2.sky { (icon("layers")) span.mono { (pg.path) } }
                        p.small { a href={ "/p/" (p.id) "/heatmap?path=" (url_enc(&pg.path)) } { (icon("spark")) " Click heatmap" } }
                        p.small.muted { (compact(pg.views)) " views · " (compact(pg.users)) " users" }
                        @if pg.median_scroll >= 0.0 {
                            table.bars { tbody {
                                tr { th scope="row" { "Reach half the page" } td.meter { (meter(pg.reach_half)) } td.num { (pct(pg.reach_half)) } }
                                tr { th scope="row" { "Reach the end" } td.meter { (meter(pg.reach_end)) } td.num { (pct(pg.reach_end)) } }
                            } }
                        }
                        @if !pg.hotspots.is_empty() {
                            h3.small { "Most clicked" }
                            table.plain { tbody { @for h in &pg.hotspots {
                                tr {
                                    th scope="row" { (h.label) }
                                    td.num { (compact(h.clicks)) }
                                    td.num { @if h.rage > 0 { span.status.bad { (icon("alert")) (compact(h.rage)) " rage" } } }
                                }
                            } } }
                        }
                    }
                }
            }
        },
    )
}

/// Error groups, most users first.
pub fn errors(
    p: &ProjectInfo<'_>,
    b: &Board,
    groups: &[openlens_engine::ErrorGroup],
    original: &[Option<openlens_errors::Original>],
    f: &Filters,
) -> Markup {
    let list = |v: &[(String, i64)]| {
        v.iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    layout(
        "Errors",
        Some(p),
        html! {
            div.heading {
                div { h1 { "Errors" } p.muted { "Uncaught errors grouped by cause, most users first, last 30 days." } }
            }
            (filter_bar(p.id, b, f, "/errors"))
            @if groups.is_empty() {
                section.card { p { (icon("task")) " No errors in the last 30 days. Nice." } }
            } @else {
                section.card.wide {
                    table.plain {
                        thead { tr { th { "Error" } (th_num("Users")) (th_num("Sessions")) (th_num("Times")) th { "Roles" } th { "Releases" } th { "Where" } th { "Last seen" } th { span.sr-only { "Replay" } } } }
                        tbody { @for (i, g) in groups.iter().enumerate() {
                            tr {
                                th scope="row" {
                                    span.mono { (g.sample.get(..160).unwrap_or(&g.sample)) }
                                    br;
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
                                td.small { (list(&g.places)) }
                                td.small.muted { (local_time(&g.last_seen, &f.tz, false)) }
                                td.small { @if let Some(u) = &g.last_user { a.mono href={ "/p/" (p.id) "/people/" (url_enc(u)) } title="Who hit it last" { (u) } } }
                                td { @if let Some(s) = &g.last_session {
                                    a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (g.last_seen) } title="Replay the latest occurrence" aria-label="Replay the latest occurrence" { (icon("play")) }
                                } }
                            }
                        } }
                    }
                }
            }
        },
    )
}

/// Recorded sessions, most recent first.
pub fn replays(
    p: &ProjectInfo<'_>,
    list: &[openlens_replay::Summary],
    who: Option<&str>,
    tz: &str,
    comments: &[(String, i64)],
) -> Markup {
    layout(
        "Replays",
        Some(p),
        html! {
            div.heading {
                div {
                    h1 { "Replays" }
                    p.muted {
                        "Recorded sessions, most recent first. Inputs are always masked."
                        @if let Some(w) = who { " Only " span.mono { (w) } ". " a href={ "/p/" (p.id) "/replays" } { "Show everyone" } }
                    }
                }
            }
            @if list.is_empty() {
                section.card { p { (icon("play")) " No recordings yet. The SDK records sessions once a page has loaded; they show up here within a few seconds." } }
            } @else {
                section.card.wide {
                    table.plain {
                        thead { tr { th { "Started (" (tz_label(tz)) ")" } th { "User" } (th_num("Active time")) th.num { "Size" } th.num { "Comments" } th { span.sr-only { "Play" } } } }
                        tbody { @for r in list {
                            @let started = tz.parse::<chrono_tz::Tz>().map_or_else(|_| r.started.format("%Y-%m-%d %H:%M").to_string(), |z| r.started.with_timezone(&z).format("%Y-%m-%d %H:%M").to_string());
                            @let secs = (r.updated - r.started).num_seconds().max(0) as f64;
                            tr {
                                td.mono { (started) }
                                td.mono { a href={ "/p/" (p.id) "/replays?u=" (r.distinct_id) } { (r.distinct_id) } }
                                td.num { @match r.active_secs {
                                    Some(a) if (a as f64) + 60.0 < secs => { (duration(a as f64)) " active" br; span.muted.small { "open " (duration(secs)) } }
                                    Some(a) => { (duration(a.max(1) as f64)) }
                                    None => { (duration(secs)) }
                                } }
                                td.num.muted { (compact((r.bytes / 1024) as i64)) " kB" }
                                td.num { @match comments.iter().find(|(s, _)| *s == r.session_id) { Some((_, n)) => { "💬 " (n) }, None => span.muted { "—" } } }
                                td { a.button href={ "/p/" (p.id) "/replays/" (r.session_id) } { (icon("play")) "Play" } }
                            }
                        } }
                    }
                }
            }
        },
    )
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

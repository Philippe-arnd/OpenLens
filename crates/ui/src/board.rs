//! The Focus board and the events page, and the board's sections.
//!
//! Moved out of `lib.rs` as is (#307).

use super::*;

/// The board page before its content (#301): the layout and a loading
/// line, sent while a cold board is computed. The content follows it in
/// the same `<main>`, and the line hides itself then (`app.css`).
pub fn board_shell(p: &ProjectInfo<'_>) -> Markup {
    layout(
        "Focus board",
        Some(p),
        html! { p.page-loading role="status" { span.spinner aria-hidden="true" {} "Computing the board…" } },
    )
}

/// The Focus board (M1: KPI strip, activity, features, roles, releases).
/// `plain`: optional plain-language `(title, next step)` per insight card
/// (same order), shown instead of the original, which stays as the tooltip.
pub fn board(
    p: &ProjectInfo<'_>,
    b: &Board,
    f: &Filters,
    host: &str,
    plain: &[(String, String)],
    revenue: Option<&openlens_engine::Revenue>,
    marks: &[Mark],
) -> Markup {
    if b.total_events == 0 {
        return layout("Focus board", Some(p), empty(p, host));
    }
    let feats: Vec<_> = b
        .features
        .iter()
        .filter(|x| x.users > 0)
        .map(|x| {
            let extra = if x.declared && x.completed > 0 {
                format!("{} uses · {} done", compact(x.uses), compact(x.completed))
            } else {
                format!("{} uses", compact(x.uses))
            };
            (
                x.name.clone().unwrap_or_else(|| x.key.clone()),
                x.users,
                extra,
            )
        })
        .collect();
    let unused: Vec<_> = b
        .features
        .iter()
        .filter(|x| x.declared && x.users == 0)
        .collect();
    let roles: Vec<_> = b
        .roles
        .iter()
        .map(|r| {
            (
                r.role.clone().unwrap_or_else(|| "not identified".into()),
                r.users,
                String::new(),
            )
        })
        .collect();
    let scope = match (&f.role, &f.release) {
        (Some(r), Some(v)) => format!("{r} · {v}"),
        (Some(r), None) => r.clone(),
        (None, Some(v)) => v.clone(),
        (None, None) => "everyone".into(),
    };
    layout(
        "Focus board",
        Some(p),
        html! {
            div.heading {
                h1 { "Focus board" }
                p.muted { "What " (scope) " did lately in " strong { (f.environment) } ". " @if !shared() { a href={ "/p/" (p.id) "/digest" } { "This week vs last →" } " · " a href={ "/p/" (p.id) "/breakdown?by=role&env=" (url_enc(&f.environment)) } { "Break down →" } } }
            }
            @if let Some(d) = &b.app_down { @if !shared() { p.alert role="alert" { (icon("alert")) " " (d) " — " a href={ "/p/" (p.id) "/uptime" } { "Uptime" } } } }
            @if !b.objectives_at_risk.is_empty() && !shared() {
                p.notice role="status" { (icon("star")) " Objectives at risk: " (b.objectives_at_risk.join(" · ")) " — " a href={ "/p/" (p.id) "/goals" } { "Goals" } }
            }
            (filter_bar(p.id, b, f, ""))
            (insight_cards(p.id, b, plain))
            section.tiles aria-label="Key numbers" {
                (tile("violet", "users", "Daily active users", &{
                    if b.users.known > 0 {
                        format!("{} · {}", compact(b.users.dau), pct(b.users.dau_share()))
                    } else {
                        compact(b.users.dau)
                    }
                }, &{
                    let mut sub = String::new();
                    if b.users.known > 0 {
                        sub.push_str(&format!("of {} known users (90 days) · ", compact(b.users.known)));
                    }
                    sub.push_str(&format!("{} weekly · {} monthly ({})", compact(b.users.wau), compact(b.users.mau), pct(b.users.mau_share())));
                    if b.users.resurrected > 0 {
                        sub.push_str(&format!(" · {} came back", compact(b.users.resurrected)));
                    }
                    if b.users.anonymous > 0 {
                        sub.push_str(&format!(" · +{} not logged in", compact(b.users.anonymous)));
                    }
                    sub
                }))
                (tile("mint", "spark", "Stickiness", &pct(b.users.stickiness), "DAU ÷ MAU"))
                (tile("sky", "clock", "Avg session", &duration(b.sessions.avg_secs), &format!("{} sessions · {} bounce", compact(b.sessions.count), pct(b.sessions.bounce_rate))))
                (tile("sun", "task", "Tasks per user per day", &format!("{:.1}", b.sessions.tasks_per_user_day), "feature uses, last 7 days"))
                (tile("coral", "alert", "Sessions with errors", &pct(b.sessions.error_rate), "last 7 days"))
            }
            div.grid {
                section.card {
                    h2.violet { (icon("pulse")) "Active users" }
                    (sparkline_marked(&b.dau_series, marks))
                    p.muted.small { "Last 14 days. Hover a bar for the day" @if !marks.is_empty() && !shared() { ", a dot for what changed (" a href={ "/p/" (p.id) "/timeline" } { "timeline" } ")" } "." }
                }
                section.card {
                    h2.sky { (icon("bolt")) "Web Vitals (p75)" }
                    @if b.vitals.is_empty() { p.muted { "No measurements yet. They arrive when visitors leave a page." } }
                    @else {
                        table.plain {
                            tbody { @for v in &b.vitals {
                                tr {
                                    th scope="row" { (v.name) }
                                    td.num { @if v.name == "CLS" { (format!("{:.3}", v.p75)) } @else { (format!("{:.0} ms", v.p75)) } }
                                    td { (status(&v.rating)) }
                                    td.num.muted { (compact(v.samples)) " samples" }
                                }
                            } }
                        }
                    }
                }
                @if let Some(r) = revenue {
                    section.card {
                        h2.mint { (icon("star")) "Revenue, last 30 days" }
                        table.plain { tbody { @for m in &r.totals {
                            tr { th scope="row" { (money(m.total, &m.currency)) } td.small.muted { (compact(m.payers)) " payers · " (money(m.per_active_user, &m.currency)) " per active user" } }
                        } } }
                        p.small { a href={ "/p/" (p.id) "/revenue" } { "By feature, role and release →" } }
                    }
                }
                (feature_pulse(b, feats.is_empty()))
                @if !unused.is_empty() {
                    section.card {
                        h2.coral { (icon("ban")) "Never used (last 30 days)" }
                        ul.tags { @for x in &unused { li { (x.name.as_deref().unwrap_or(&x.key)) } } }
                        p.muted.small { "Declared in the feature map, but nobody reached them. Hidden, broken, or not needed?" }
                    }
                }
                @if !b.role_gaps.is_empty() {
                    section.card {
                        h2.sun { (icon("gap")) "Role gaps" }
                        p.small { "These raw role values match no role in the feature map, so they are counted apart:" }
                        ul.tags { @for (raw, n) in &b.role_gaps { li { code { (raw) } " · " (compact(*n)) " users" } } }
                        p.muted.small { "Ask your agent to map them under " code { "roles:" } " in openlens.features.yaml." }
                    }
                }
                section.card {
                    h2.mint { (icon("shield")) "Who uses the app" }
                    (bars(&roles))
                    p.muted.small { "Distinct users per role, last 30 days." }
                }
                (role_matrix(b))
                (retention_grid(b))
                @if !b.funnels.is_empty() {
                    section.card.wide {
                        h2.sky { (icon("funnel")) "Funnels" }
                        @for fun in &b.funnels {
                            h3.small { (fun.feature) }
                            table.bars {
                                tbody { @for (i, st) in fun.steps.iter().enumerate() {
                                    tr {
                                        th scope="row" { span.muted { (i + 1) ". " } (st.event) }
                                        td.meter { (meter(st.conversion)) }
                                        td.num { (compact(st.users)) }
                                        td.num.muted { (pct(st.conversion)) @if i > 0 { " · " (duration(st.median_secs)) } }
                                    }
                                } }
                            }
                        }
                        p.muted.small { "From each feature's declared steps, in order, within a day, last 30 days. Time is the median from the previous step." }
                    }
                }
                @if !b.flag_cohorts.is_empty() {
                    section.card.wide {
                        h2.violet { (icon("flag")) "Flags: with vs without" }
                        table.plain {
                            thead { tr { th { "Flag" } th.num { "Users with / without" } th.num { "Avg session" } th.num { "Tasks / session" } th.num { "Sessions with errors" } th { "Confidence" } } }
                            tbody { @for c in &b.flag_cohorts {
                                tr {
                                    th scope="row" { code { (c.flag) } }
                                    td.num { (compact(c.with.users)) " / " (compact(c.without.users)) }
                                    td.num { (duration(c.with.avg_secs)) " / " (duration(c.without.avg_secs)) }
                                    td.num { (format!("{:.2}", c.with.tasks_per_session)) " / " (format!("{:.2}", c.without.tasks_per_session)) }
                                    td.num { (pct(c.with.error_rate)) " / " (pct(c.without.error_rate)) }
                                    td.small { (confidence(match (c.p_errors, c.p_tasks) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) })) }
                                }
                            } }
                        }
                        p.muted.small { "Sessions where the flag was on, against all other sessions, last 30 days." }
                    }
                }
                @if !b.journeys.is_empty() {
                    section.card.wide {
                        h2.mint { (icon("route")) "Paths to value" }
                        p.muted.small { "What users did just before reaching each success for the first time (last 30 days)." }
                        ul.journeys {
                            @for j in &b.journeys {
                                li {
                                    @for step in &j.path { span.step { (step) } span.arrow aria-hidden="true" { "→" } }
                                    span.step.done { "✓ " (j.success) }
                                    span.muted { " " (compact(j.users)) " users · " (pct(j.share)) " of those who got there" }
                                }
                            }
                        }
                    }
                }
                section.card.wide {
                    h2.coral { (icon("release")) "Releases" }
                    @if b.releases.is_empty() { p.muted { "No release tagged yet. Pass " code { "release" } " to the SDK to compare releases." } }
                    @else {
                        (release_compare(&b.releases))
                        table.plain {
                            thead { tr { th { "Release" } th.num { "Users" } th.num { "Sessions" } th.num { "Avg session" } th.num { "Bounce" } th.num { "Tasks / session" } th.num { "Sessions with errors" } th { "First seen" } } }
                            tbody { @for (i, r) in b.releases.iter().enumerate() {
                                tr {
                                    th scope="row" { (r.release) @if i == 0 { " " span.badge { "latest" } } }
                                    td.num { (compact(r.users)) }
                                    td.num { (compact(r.sessions)) }
                                    td.num { (duration(r.avg_secs)) }
                                    td.num { (pct(r.bounce_rate)) }
                                    td.num { (format!("{:.2}", r.tasks_per_session)) }
                                    td.num { (pct(r.error_rate)) }
                                    td.muted { (r.first_seen.get(..16).unwrap_or(&r.first_seen)) }
                                }
                            } }
                        }
                    }
                }
            }
            p.muted.small.center {
                @match b.map_version {
                    Some(v) => { "Feature map v" (v) " in use. Cards compare users who stayed with users who churned; each shows the cohort sizes behind it." }
                    None => { "No feature map yet: roles are raw and features come only from data-feature. Ask your agent to push openlens.features.yaml for sharper insights." }
                }
            }
        },
    )
}

/// Recent events.
pub fn events(
    p: &ProjectInfo<'_>,
    b: &Board,
    rows: &[EventRow],
    f: &Filters,
    eq: &openlens_engine::EventQuery,
    next: Option<&str>,
) -> Markup {
    let mut envs = b.environments.clone();
    if !envs.iter().any(|e| e == "production") {
        envs.insert(0, "production".into());
    }
    let sort = eq.sort.as_deref().unwrap_or("time");
    let days = eq.days.unwrap_or(30);
    layout(
        "Events",
        Some(p),
        html! {
            div.heading { h1 { "Events" } p.muted {
                @match sort { "event" => "By event name", "person" => "By user", _ => "Newest first" }
                ", last " (days) (if days == 1 { " day" } else { " days" }) ", 100 per page. Times in " (f.tz) "."
            } }
            form.filters method="get" action={ "/p/" (p.id) "/events" } aria-label="Filters" {
                (filter_fields(&envs, b, f))
                label { "Family "
                    select name="family" {
                        option value="" { "All families" }
                        @for (k, label, _) in openlens_engine::FAMILIES {
                            option value=(k) selected[eq.family.as_deref() == Some(*k)] { (label) }
                        }
                    }
                }
                label { "Event " input type="text" name="event" value=[eq.event.as_deref()] placeholder="any" maxlength="200"; }
                label { "User " input type="text" name="person" value=[eq.person.as_deref()] placeholder="any" maxlength="200"; }
                label { "Page " input type="text" name="path" value=[eq.path.as_deref()] placeholder="/settings" maxlength="200"; }
                label { "Period "
                    select name="days" {
                        @for (d, l) in [(1, "24 hours"), (7, "7 days"), (30, "30 days"), (90, "90 days")] {
                            option value=(d) selected[days == d] { (l) }
                        }
                    }
                }
                label { "Sort "
                    select name="sort" {
                        option value="time" selected[sort == "time"] { "Newest first" }
                        option value="event" selected[sort == "event"] { "Event name" }
                        option value="person" selected[sort == "person"] { "User" }
                    }
                }
                button type="submit" { "Apply" }
            }
            section.card.wide {
                @if rows.is_empty() { p.muted { "Nothing matches. Widen the period or clear a filter." } }
                @else {
                    table.plain.events {
                        thead { tr { th { "When (" (tz_label(&f.tz)) ")" } th { "Event" } th { "User" } th { "Role" } th { "Page" } th { "Feature" } th { "Release" } th { span.sr-only { "Replay" } } } }
                        tbody { @for r in rows {
                            @let fam = openlens_engine::family(&r.event);
                            tr {
                                td.muted.mono { (local_time(&r.timestamp, &f.tz, true)) }
                                td { span class={ "chip fam " (fam) } title=(family_label(fam)) { (icon(family_icon(fam))) span.sr-only { (family_label(fam)) ": " } (r.event) } }
                                td.mono { a href={ "/p/" (p.id) "/people/" (url_enc(&r.distinct_id)) } title="Profile" { (r.distinct_id) } }
                                td { (r.role.as_deref().unwrap_or("—")) }
                                td.mono { (r.path.as_deref().unwrap_or("—")) }
                                td { (r.feature.as_deref().unwrap_or("—")) }
                                td { (r.release.as_deref().unwrap_or("—")) }
                                td { @if let Some(s) = &r.session_id {
                                    a.icon-link href={ "/p/" (p.id) "/replays/" (s) "?at=" (r.timestamp) } title="Replay this moment" aria-label="Replay this moment" { (icon("play")) }
                                } }
                            }
                        } }
                    }
                    @if let Some(n) = next { p { a.button href=(n) { "Older / next 100" (icon("right")) } } }
                }
            }
        },
    )
}

pub(crate) fn family_icon(fam: &str) -> &'static str {
    match fam {
        "page" => "route",
        "click" => "bolt",
        "form" => "task",
        "feature" => "star",
        "identify" => "users",
        "error" => "alert",
        "vital" => "pulse",
        "survey" => "idea",
        "custom" => "spark",
        _ => "events",
    }
}

pub(crate) fn family_label(fam: &str) -> &'static str {
    openlens_engine::FAMILIES
        .iter()
        .find(|(k, _, _)| *k == fam)
        .map_or("Other", |(_, l, _)| l)
}

/// Feature pulse: for each used feature, how many of the active users
/// adopt it, how often and on how many days they come back to it, and how
/// many reach its success event.
pub(crate) fn feature_pulse(b: &Board, empty: bool) -> Markup {
    let mau = b.users.mau.max(1) as f64;
    let ratio = |a: i64, n: i64| a as f64 / n.max(1) as f64;
    html! {
        section.card.wide {
            h2.sun { (icon("layers")) "Feature pulse" }
            @if empty {
                p.muted { "No feature has been used yet. Features come from " code { "data-feature" } " attributes and the feature map." }
            } @else {
                table.plain.pulse {
                    thead { tr {
                        th { "Feature" }
                        th { "Adoption" }
                        th.num title="Share of active users who used it" { span.sr-only { "Adoption " } "% users" }
                        th.num title="Average uses per user" { "Uses / user" }
                        th.num title="Average distinct days each user came back to it" { "Days / user" }
                        th.num title="Users who reached its success event" { "Completion" }
                        th.num title="Median time from the first use to the success event (p90 below)" { "Task time" }
                    } }
                    tbody { @for x in b.features.iter().filter(|x| x.users > 0) {
                        @let adoption = (x.users as f64 / mau).min(1.0);
                        tr {
                            th scope="row" { (x.name.as_deref().unwrap_or(&x.key)) }
                            td.meter { (meter(adoption)) }
                            td.num { strong { (pct(adoption)) } br; span.muted.small { (compact(x.users)) " users" } }
                            td.num { (format!("{:.1}", ratio(x.uses, x.users))) }
                            td.num { (format!("{:.1}", ratio(x.user_days, x.users))) }
                            td.num {
                                @if x.declared && x.completed_users > 0 { (pct(ratio(x.completed_users, x.users).min(1.0))) }
                                @else { span.muted title="No success event declared or reached yet" { "—" } }
                            }
                            td.num {
                                @if let Some(t) = b.task_times.iter().find(|t| t.feature == x.key) {
                                    (duration(t.median_secs)) br; span.muted.small { "p90 " (duration(t.p90_secs)) }
                                } @else { span.muted { "—" } }
                            }
                        }
                    } }
                }
                p.muted.small { "Last 30 days. Adoption is the share of active users (MAU) who used the feature; completion is the share of them who reached its success event; task time runs from the first use to that event (within 30 minutes)." }
            }
        }
    }
}

/// A p-value in words: whether the difference can be trusted yet.
pub(crate) fn confidence(p: Option<f64>) -> Markup {
    match p {
        None => {
            html! { span.muted title="Fewer than 30 sessions on one side" { "too few sessions" } }
        }
        Some(p) if p < openlens_engine::stats::ALPHA => {
            html! { strong title=(format!("p = {p:.3}")) { "significant" } }
        }
        Some(p) => {
            html! { span.muted title=(format!("p = {p:.3}")) { "not yet (p = " (format!("{p:.2}")) ")" } }
        }
    }
}

/// Weekly retention cohorts: users first seen each week (rows) and the
/// share of them active again N weeks later (columns).
pub(crate) fn retention_grid(b: &Board) -> Markup {
    let rows: Vec<_> = b.retention.iter().filter(|r| r.users > 0).collect();
    let cols = rows.iter().map(|r| r.weeks.len()).max().unwrap_or(0);
    if rows.len() < 2 || cols == 0 {
        return html! {};
    }
    html! {
        section.card.wide {
            h2.mint { (icon("clock")) "Do they come back?" }
            table.plain.matrix {
                thead { tr {
                    th { "First seen (week of)" }
                    th.num { "Users" }
                    @for k in 1..=cols { th.num scope="col" { "Week " (k) } }
                } }
                tbody { @for r in &rows {
                    tr {
                        th scope="row" { (r.week) }
                        td.num.muted { (compact(r.users)) }
                        @for k in 0..cols {
                            @if let Some(share) = r.weeks.get(k) {
                                @let level = if *share <= 0.0 { 0 } else { ((share * 5.0).ceil() as u8).clamp(1, 5) };
                                td class={ "num heat-" (level) } { (pct(*share)) }
                            } @else {
                                td {}
                            }
                        }
                    }
                } }
            }
            p.muted.small { "Share of each week's new users who were active again N weeks later (weeks start on Monday, " (b.filters.as_ref().map_or("UTC", |f| f.tz.as_str())) ")." }
        }
    }
}

/// Roles × features: the share of each role's users who used each
/// feature in the last 30 days. One hue, darker = more adoption; the number
/// is always printed, so colour is never the only cue.
pub(crate) fn role_matrix(b: &Board) -> Markup {
    let roles: Vec<(&str, i64)> = b
        .roles
        .iter()
        .filter_map(|r| Some((r.role.as_deref()?, r.users)))
        .filter(|(_, n)| *n > 0)
        .take(6)
        .collect();
    let feats: Vec<&openlens_engine::FeatureUse> =
        b.features.iter().filter(|f| f.users > 0).take(10).collect();
    if roles.len() < 2 || feats.is_empty() {
        return html! {};
    }
    let cell = |role: &str, feat: &str| {
        b.role_features
            .iter()
            .find(|(r, f, _)| r == role && f == feat)
            .map_or(0, |x| x.2)
    };
    html! {
        section.card.wide {
            h2.violet { (icon("users")) "Who uses what" }
            table.plain.matrix {
                thead { tr { th { "Feature" } @for (r, n) in &roles { th.num scope="col" { (r) br; span.muted.small { (compact(*n)) " users" } } } } }
                tbody { @for f in &feats {
                    tr {
                        th scope="row" { (f.name.as_deref().unwrap_or(&f.key)) }
                        @for (r, n) in &roles {
                            @let share = cell(r, &f.key) as f64 / (*n).max(1) as f64;
                            @let level = if share <= 0.0 { 0 } else { ((share * 5.0).ceil() as u8).clamp(1, 5) };
                            td class={ "num heat-" (level) } title={ (pct(share)) " of " (r) " users used " (f.name.as_deref().unwrap_or(&f.key)) } {
                                @if share > 0.0 { (pct(share)) } @else { span.muted { "—" } }
                            }
                        }
                    }
                } }
            }
            p.muted.small { "Share of each role's users who used the feature, last 30 days. Empty cells are features a role never touches." }
        }
    }
}

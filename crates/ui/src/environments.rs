//! The *Environments* tab of project settings (#410): hosts per
//! environment, host rules, aliases, hidden environments and the monitors
//! of non-production environments. Every form posts to the same handler,
//! which runs the engine commands the API runs.

use super::*;
use openlens_engine::environments::{
    ACTIVE_DAYS, AllowPreview, AllowReport, AllowStatus, Environments, LOG, MAX_ALLOWED,
    MAX_RELABEL_DAYS, NOTIFY, RELABEL_DAYS, Relabelled,
};

/// A small form posting one action to the tab's handler.
fn action_form(base: &str, action: &str, fields: &[(&str, &str)], label: &str) -> Markup {
    html! {
        form.inline method="post" action={ (base) "/environments" } {
            input type="hidden" name="action" value=(action);
            @for (k, v) in fields { input type="hidden" name=(k) value=(v); }
            button.ghost type="submit" { (label) }
        }
    }
}

/// The names suggested for an environment field.
fn env_names(id: &str, e: &Environments, ignore: bool) -> Markup {
    let mut names: Vec<&str> = vec!["production", "staging", "development", "preview", "test"];
    for h in &e.hosts {
        if !names.contains(&h.environment.as_str()) {
            names.push(&h.environment);
        }
    }
    html! {
        datalist id=(id) {
            @for n in names { option value=(n) {} }
            @if ignore { option value="ignore" {} }
        }
    }
}

/// The tab's content. `tz`: the project's time zone, for the times shown.
pub(crate) fn tab(
    base: &str,
    admin: bool,
    tz: &str,
    e: &Environments,
    relabel: Option<&Result<Relabelled, String>>,
    allow: Option<&AllowReport>,
    allow_preview: Option<&Result<AllowPreview, String>>,
) -> Markup {
    let mut seen: Vec<&str> = Vec::new();
    for h in &e.hosts {
        if !seen.contains(&h.environment.as_str()) {
            seen.push(&h.environment);
        }
    }
    for h in &e.hidden {
        if !seen.contains(&h.as_str()) {
            seen.push(h);
        }
    }
    html! {
        section.card.wide #environments {
            h2.mint { (icon("route")) "Environments" }
            p.small { "Each event's environment comes from its page address. The app's own setting wins, then a host rule below, then the usual hostname patterns (localhost, staging., …). A host that nothing matches is stored as " code { "unknown" } " and never counted as production. " code { "ignore" } " drops a host's events at ingestion. Aliases rename an environment the app sends. Rules and aliases apply to events received from now on." }
            @if !e.unclassified.is_empty() {
                p.notice role="status" { (e.unclassified.len()) " host(s) without a rule, stored as unknown: " @for (i, h) in e.unclassified.iter().enumerate() { @if i > 0 { ", " } span.mono { (h) } } }
            }
            (env_names("env-names", e, true))
            @if admin {
                form.settings method="post" action={ (base) "/environments" } {
                    input type="hidden" name="action" value="rule";
                    label { "Host " input type="text" name="host" required maxlength="253" placeholder="app.example.com or *.example.com" list="env-hosts"; }
                    datalist id="env-hosts" { @for h in &e.unclassified { option value=(h) {} } }
                    label { "Environment " input type="text" name="env" required maxlength="64" list="env-names" placeholder="production"; }
                    button type="submit" { "Save the rule" }
                }
            }
            h3 { "Rules" }
            @if e.rules.is_empty() { p.muted.small { "No rule yet." } } @else {
                div.table-scroll { table.plain {
                    thead { tr { th { "Host" } th { "Environment" } @if admin { th { span.sr-only { "Actions" } } } } }
                    tbody { @for r in &e.rules {
                        tr {
                            td.mono { (r.host) } td { (r.env) }
                            @if admin { td { (action_form(base, "remove", &[("host", r.host.as_str())], "Remove")) } }
                        }
                    } }
                } }
            }
            h3 { "Hosts seen (last " (e.days) " days)" }
            @if e.hosts.is_empty() { p.muted.small { "No events in the window." } } @else {
                div.table-scroll { table.plain {
                    thead { tr { th { "Environment" } th { "Host" } (crate::glossary::th_num("Events")) th { "Last seen" } th { "Rule" } } }
                    tbody { @for h in &e.hosts {
                        tr {
                            td { (h.environment) }
                            td.mono { @if h.host.is_empty() { span.muted { "(no page address)" } } @else { (h.host) } }
                            td.num { (compact(h.events)) }
                            td.small { (local_time(&h.last_seen, tz, false)) }
                            td.small { (h.rule.as_deref().unwrap_or("—")) }
                        }
                    } }
                } }
            }
        }
        @if admin { (relabel_card(base, e, relabel)) }
        @if let Some(a) = allow { (allow_card(base, admin, tz, a, allow_preview)) }
        section.card.wide #env-aliases {
            h2.sky { (icon("route")) "Aliases and visibility" }
            p.small { "An alias stores an environment the app sends under another name (" code { "live-eu" } " → " code { "production" } "), on top of the usual ones (" code { "prod" } ", " code { "stage" } ", " code { "dev" } "); " code { "ignore" } " drops its events. A hidden environment keeps its events and stays readable by name (" code { "?env=" } "), but leaves the environment selector. Production is never aliased or hidden." }
            @if admin {
                form.settings method="post" action={ (base) "/environments" } {
                    input type="hidden" name="action" value="alias";
                    label { "Sent as " input type="text" name="from" required maxlength="64" placeholder="live-eu"; }
                    label { "Store as " input type="text" name="to" required maxlength="64" list="env-names" placeholder="production"; }
                    button type="submit" { "Save the alias" }
                }
            }
            @if e.aliases.is_empty() { p.muted.small { "No alias." } } @else {
                div.table-scroll { table.plain {
                    thead { tr { th { "Sent as" } th { "Stored as" } @if admin { th { span.sr-only { "Actions" } } } } }
                    tbody { @for a in &e.aliases {
                        tr {
                            td.mono { (a.from) } td { (a.to) }
                            @if admin { td { (action_form(base, "unalias", &[("from", a.from.as_str())], "Remove")) } }
                        }
                    } }
                } }
            }
            @if !seen.is_empty() {
                div.table-scroll { table.plain {
                    thead { tr { th { "Environment" } th { "Selector" } @if admin { th { span.sr-only { "Actions" } } } } }
                    tbody { @for env in &seen {
                        @let hidden = e.hidden.iter().any(|h| h == env);
                        tr {
                            td { (env) }
                            td.small { @if hidden { "hidden" } @else { "shown" } }
                            @if admin && *env != "production" { td {
                                @if hidden { (action_form(base, "show", &[("env", *env)], "Show")) }
                                @else { (action_form(base, "hide", &[("env", *env)], "Hide")) }
                            } } @else if admin { td {} }
                        }
                    } }
                } }
            }
        }
        section.card.wide #env-monitors {
            h2.coral { (icon("bolt")) "Checks per environment" }
            p.small { "Production is checked at the app address (General) and gets every alert. Give another environment its own address: it is checked every minute with the same health path, and its certificate daily. At " strong { "notify" } " its incidents are announced like production's; at " strong { "log" } " they are only recorded on Uptime." }
            @if admin {
                form.settings method="post" action={ (base) "/environments" } {
                    input type="hidden" name="action" value="monitor";
                    label { "Environment " input type="text" name="env" required maxlength="64" list="env-names" placeholder="staging"; }
                    label { "Address " input type="url" name="url" required maxlength="200" placeholder="https://staging.example.com"; }
                    label { "Alerts "
                        select name="level" {
                            option value=(LOG) selected { "log only" }
                            option value=(NOTIFY) { "notify" }
                        }
                    }
                    button type="submit" { "Save the check" }
                }
            }
            @if e.monitors.is_empty() { p.muted.small { "No environment besides production is checked." } } @else {
                div.table-scroll { table.plain {
                    thead { tr { th { "Environment" } th { "Address" } th { "Alerts" } @if admin { th { span.sr-only { "Actions" } } } } }
                    tbody { @for m in &e.monitors {
                        tr {
                            td { (m.env) } td.mono { (m.url) }
                            td.small { @if m.level == NOTIFY { "notify" } @else { "log only" } }
                            @if admin { td { (action_form(base, "unmonitor", &[("env", m.env.as_str())], "Remove")) } }
                        }
                    } }
                } }
            }
            p.muted.small { "Agents: " code { "openlens-cli env list" } ", " code { "env rule <host> <env>" } ", " code { "env relabel <host> --dry-run" } ", " code { "env alias <from> <to>" } ", " code { "env hide <env>" } ", " code { "env monitor <env> <url> --level notify" } ", " code { "env allow <hosts…>" } ", or the MCP tool " code { "environments" } "." }
        }
    }
}

/// *Re-label past events* (#484): a dry run first (a plain GET of the
/// page), then a confirm button that posts the rewrite.
fn relabel_card(
    base: &str,
    e: &Environments,
    relabel: Option<&Result<Relabelled, String>>,
) -> Markup {
    let (host, days) = match relabel {
        Some(Ok(r)) => (r.host.clone(), r.days),
        _ => (String::new(), RELABEL_DAYS),
    };
    html! {
        section.card.wide #env-relabel {
            h2.violet { (icon("route")) "Re-label past events" }
            p.small { "Rules apply to new events. After changing a host's rule, move that host's events of the last days (at most " (MAX_RELABEL_DAYS) ") to the environment the rules give now. Bots stay bots, and a host the rules ignore keeps its events. What the app set itself moves too: check the preview first." }
            form.settings method="get" action={ (base) "/environments" } {
                label { "Host " input type="text" name="relabel" required maxlength="253" value=(host) placeholder="app.example.com or *.example.com" list="env-rule-hosts"; }
                datalist id="env-rule-hosts" { @for r in &e.rules { option value=(r.host) {} } }
                label { "Days " input type="number" name="days" min="1" max=(MAX_RELABEL_DAYS) value=(days); }
                button.ghost type="submit" { "Preview" }
            }
            @match relabel {
                Some(Err(msg)) => { p.alert role="alert" { (msg) } }
                Some(Ok(r)) if r.events == 0 => {
                    p.notice role="status" { "Nothing to move: the events of " span.mono { (r.host) } " over the last " (r.days) " days already have the environment the rules give." }
                }
                Some(Ok(r)) => {
                    p.notice role="status" { (r.events) " event(s) of " span.mono { (r.host) } " over the last " (r.days) " days would move:" }
                    div.table-scroll { table.plain {
                        thead { tr { th { "Stored as" } th { "Moves to" } (crate::glossary::th_num("Events")) } }
                        tbody { @for m in &r.moves {
                            tr { td { (m.from) } td { (m.to) } td.num { (m.events) } }
                        } }
                    } }
                    form.inline method="post" action={ (base) "/environments" } {
                        input type="hidden" name="action" value="relabel";
                        input type="hidden" name="host" value=(r.host);
                        input type="hidden" name="days" value=(r.days);
                        button type="submit" { "Re-label " (r.events) " event(s)" }
                    }
                }
                None => {}
            }
        }
    }
}

/// *Allowed hosts* (#485): the list, whether it drops anything now, what
/// it dropped, and for admins a preview of a new list over the last 24
/// hours before it is saved.
fn allow_card(
    base: &str,
    admin: bool,
    tz: &str,
    a: &AllowReport,
    preview: Option<&Result<AllowPreview, String>>,
) -> Markup {
    let since = local_time(&a.dropped_since, tz, false);
    let draft = match preview {
        Some(Ok(p)) => p.hosts.join(" "),
        _ => a.hosts.join(" "),
    };
    html! {
        section.card.wide #env-allow {
            h2.sun { (icon("shield")) "Allowed hosts" }
            p.small { "When set, events whose page address is on another host are dropped at ingestion, so a copied snippet or a scraper cannot write into this project. Events without a page address (server SDKs) always pass. The list only drops events while one of its hosts has sent events in the last " (ACTIVE_DAYS) " days: until then it is pending and drops nothing, so a typo never locks out the app. A list is saved after a preview of what it would have dropped over the last 24 hours. At most " (MAX_ALLOWED) " hosts; " code { "*.example.com" } " covers the sub-domains." }
            @match a.status {
                AllowStatus::Off => { p.muted.small { "Off: every host is accepted." } }
                AllowStatus::Pending => {
                    p.notice role="status" { "Pending: no host on the list has sent events in the last " (ACTIVE_DAYS) " days, so nothing is dropped. Check the list for a typo." }
                }
                AllowStatus::Active => {
                    p.small { "Active: events of other hosts are dropped." @if let Some(t) = &a.last_listed { " A listed host last sent events at " (local_time(t, tz, false)) "." } }
                }
            }
            @if !a.hosts.is_empty() {
                p { @for (i, h) in a.hosts.iter().enumerate() { @if i > 0 { ", " } span.mono { (h) } } }
            }
            @if !a.dropped.is_empty() || a.dropped_other > 0 {
                h3 { "Dropped since " (since) }
                div.table-scroll { table.plain {
                    thead { tr { th { "Host" } (crate::glossary::th_num("Events")) } }
                    tbody {
                        @for d in &a.dropped { tr { td.mono { (d.host) } td.num { (d.events) } } }
                        @if a.dropped_other > 0 { tr { td.muted { "other hosts" } td.num { (a.dropped_other) } } }
                    }
                } }
            } @else if a.status == AllowStatus::Active {
                p.muted.small { "Nothing dropped since " (since) "." }
            }
            @if admin {
                form.settings method="get" action={ (base) "/environments" } {
                    label { "Hosts " input type="text" name="allow" maxlength="4000" value=(draft) placeholder="app.example.com *.example.com"; }
                    button.ghost type="submit" { "Preview" }
                }
                @match preview {
                    Some(Err(msg)) => { p.alert role="alert" { (msg) } }
                    Some(Ok(p)) => { (allow_preview_block(base, p)) }
                    None => {}
                }
                @if !a.hosts.is_empty() {
                    (action_form(base, "allow", &[("hosts", "")], "Turn off"))
                }
            }
        }
    }
}

/// The preview of a new list, and the button that saves it.
fn allow_preview_block(base: &str, p: &AllowPreview) -> Markup {
    let hosts = p.hosts.join(" ");
    html! {
        @if p.hosts.is_empty() {
            p.notice role="status" { "An empty list turns the allow-list off: every host is accepted." }
        } @else {
            p.notice role="status" {
                "Over the last " (p.hours) " hours, this list keeps " (p.kept_events) " event(s) and would have dropped " (p.dropped_events) " from " (p.dropped.len()) " other host(s). "
                @match p.status {
                    AllowStatus::Active => { "A listed host sent events recently: the list takes effect once saved." }
                    _ => { strong { "No host on this list sent events in the last " (ACTIVE_DAYS) " days" } ": it stays pending and drops nothing until one does." }
                }
            }
            @if !p.dropped.is_empty() {
                div.table-scroll { table.plain {
                    thead { tr { th { "Host outside the list" } (crate::glossary::th_num("Events")) } }
                    tbody { @for d in &p.dropped { tr { td.mono { (d.host) } td.num { (d.events) } } } }
                } }
            }
        }
        form.inline method="post" action={ (base) "/environments" } {
            input type="hidden" name="action" value="allow";
            input type="hidden" name="hosts" value=(hosts);
            button type="submit" { @if p.hosts.is_empty() { "Turn off" } @else { "Save the list" } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::environments::HostRow;

    #[test]
    fn the_times_are_read_in_the_project_zone_not_as_iso_strings() {
        // #556: the tab printed `2026-11-26T01:54:22.935195Z` as it came.
        let e = Environments {
            days: 30,
            hosts: vec![HostRow {
                environment: "production".into(),
                host: "app.example.com".into(),
                events: 5,
                last_seen: "2026-11-26T01:54:22.935195Z".into(),
                rule: None,
            }],
            rules: vec![],
            unclassified: vec![],
            aliases: vec![],
            hidden: vec![],
            monitors: vec![],
        };
        let a = AllowReport {
            hosts: vec!["app.example.com".into()],
            status: AllowStatus::Active,
            last_listed: Some("2026-11-26T01:54:22.935195+00:00".into()),
            dropped_since: "2026-11-25T08:00:00+00:00".into(),
            dropped: vec![],
            dropped_other: 0,
        };
        let html = tab(
            "/p/p1/settings",
            true,
            "Europe/Paris",
            &e,
            None,
            Some(&a),
            None,
        )
        .into_string();
        assert!(!crate::tests::has_iso_time(&html), "{html}");
        // Shown in the project's zone (Paris is ahead of UTC in November).
        assert!(html.contains("2026-11-26 02:54"), "{html}");
        assert!(html.contains("2026-11-25 09:00"), "{html}");
    }
}

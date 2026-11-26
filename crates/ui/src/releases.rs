//! The Releases page (#500, D-049): each release per environment, with its
//! state, when it was seen, its users, its time in staging and a link to
//! the board filtered on it. With the deploy hook (#504, D-053): each
//! release's deploy time and last deploy, failed deploys, and the hook's
//! URL, made or rotated here.

use super::*;
use crate::components::detail_url;
use openlens_engine::delivery::Delivery;
use openlens_engine::deploys::Status;
use openlens_engine::releases::{Releases, State};
use openlens_engine::verdict::Verdict;

/// The deploy hook part of the Releases page.
pub struct HookView<'a> {
    /// The viewer may make or rotate the URL.
    pub admin: bool,
    /// A URL just made: shown once.
    pub new_url: Option<&'a str>,
}

/// A duration in seconds, short: `3 d 4 h`, `5 h 10 min`, `12 min`.
fn duration(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{} min", m.max(1))
    }
}

fn state_class(s: State) -> &'static str {
    match s {
        State::Live => "status good",
        State::RollingOut => "status warn",
        State::RolledBack => "status bad",
        State::Superseded => "status muted",
        State::Failed => "status bad",
        State::Deploying => "status warn",
    }
}

fn verdict_class(v: Verdict) -> &'static str {
    match v {
        Verdict::Healthy => "status good",
        Verdict::Regressed => "status bad",
        Verdict::Inconclusive => "status muted",
    }
}

fn deploy_label(s: Status) -> &'static str {
    match s {
        Status::Started => "started",
        Status::Succeeded => "succeeded",
        Status::Failed => "failed",
        Status::RolledBack => "rolled back",
    }
}

/// What the plain form looks like.
const PLAIN: &str =
    r#"{"status": "succeeded", "release": "v1.4", "commit": "…", "environment": "production"}"#;

/// The deploy hook: its URL once made, or how to make or rotate it.
fn hook_card(p: &ProjectInfo<'_>, r: &Releases, h: &HookView<'_>) -> Markup {
    html! {
        section.card.wide {
            h2 { "Deploy hook" }
            p.muted.small { "One URL to paste in the platform's deploy notifications: Coolify, Vercel, Netlify, Render, GitHub Deployments, Argo CD, Flux, or any step that posts "
                code { (PLAIN) }
                ". Deploys then show here with their time, before the first user arrives, and failed ones too. Add "
                code { "?env=staging" } " when the platform does not say the environment. See "
                a href="/agents.md" { "the wiring guide" } "." }
            @if let Some(url) = h.new_url {
                p { strong { "Copy it now: it is not shown again." } }
                pre.snippet { code { (url) } }
            } @else if let Some(since) = &r.deploy_hook {
                p.small { "Set up on " (since) ". The URL is kept secret: rotate it to see a new one." }
            } @else {
                p.small.muted { "No deploy hook yet." }
            }
            @if h.admin {
                form.inline method="post" action={ "/p/" (p.id) "/releases/hook" } {
                    @if r.deploy_hook.is_some() || h.new_url.is_some() {
                        label.check { input type="checkbox" name="confirm" value="yes" required; " The old URL stops working at once" }
                        button.ghost type="submit" { "Rotate the URL" }
                    } @else {
                        button.ghost type="submit" { "Make the URL" }
                    }
                }
            }
        }
    }
}

/// Releases (#500): `releases.list`, as a table, the delivery metrics
/// (`releases.delivery`, #523) when they could be computed, and the
/// deploy hook.
pub fn releases(
    p: &ProjectInfo<'_>,
    r: &Releases,
    delivery: Option<&Delivery>,
    hook: &HookView<'_>,
) -> Markup {
    layout(
        "Releases",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Releases" }
                p.muted { "Last " (r.days) " days, times in " (r.timezone) ". A release's share is its part of the environment's distinct users per day. "
                    strong { "Live" } ": at least 95 % on the latest day. "
                    strong { "Rolling out" } ": below that, not overtaken. "
                    strong { "Superseded" } ": a newer release took the majority. "
                    strong { "Rolled back" } ": it had the majority, then an older release took it back. "
                    strong { "Failed" } " and " strong { "deploying" } ": a deploy the hook reported, with no traffic. "
                    "Time in staging runs from the build's first event in staging to its first in production. "
                    strong { "Verdict" } ": once a release has 30 sessions and 8 users, or is a day old, it is compared with the release before it in the environment (errors, new error groups, bounce, tasks, Web Vitals, features with a success event, guardrails): "
                    strong { "healthy" } " when nothing got worse, " strong { "regressed" } " when something did, " strong { "inconclusive" } " with too little traffic or nothing to compare with. Production is judged unless one environment is picked." }
            } }
            section.card.wide {
                @if r.releases.is_empty() {
                    p.muted { "No release seen yet. Pass a release to the SDK, or let the browser SDK name builds on its own (D-044)." }
                } @else {
                    div.table-scroll { table.plain {
                        thead { tr {
                            th scope="col" { "Release" } th scope="col" { "Environment" } th scope="col" { "State" }
                            th.num scope="col" { "Share" } th scope="col" { "First seen" } th scope="col" { "Last seen" }
                            th.num scope="col" { "Users" } th scope="col" { "Verdict" } th scope="col" { "Deployed" } th scope="col" { "Time in staging" } th scope="col" { "Board" }
                        } }
                        tbody { @for x in &r.releases {
                            tr {
                                td { code { (x.release) } }
                                td { (x.environment) }
                                td { span class=(state_class(x.state)) { (x.state.label()) } }
                                td.num { (pct(x.share)) }
                                td.small.mono { (x.first_seen) }
                                td.small.mono { (x.last_seen) }
                                td.num { (x.users) }
                                td.small {
                                    @match x.verdict {
                                        Some(v) => { span class=(verdict_class(v)) { (v.label()) } }
                                        None if x.reasons.iter().any(|r| r.starts_with("Waiting")) => { span.muted { "waiting" } }
                                        None => { span.muted { "—" } }
                                    }
                                    @for reason in x.reasons.iter().take(3) {
                                        br; span.muted { (reason) }
                                    }
                                }
                                td.small {
                                    @match (&x.deployed_at, x.deploy) {
                                        (Some(t), Some(Status::Succeeded) | None) => { span.mono { (t) } }
                                        (Some(t), Some(s)) => { span.mono { (t) } span.muted { ", then " (deploy_label(s)) } }
                                        (None, Some(s)) => { span.muted { (deploy_label(s)) } }
                                        (None, None) => { span.muted { "—" } }
                                    }
                                }
                                td.small {
                                    @match (x.staging_secs, &x.staged_as) {
                                        (Some(s), Some(n)) if *n != x.release => { (duration(s)) span.muted { " as " code { (n) } } }
                                        (Some(s), _) => { (duration(s)) }
                                        _ => { span.muted { "—" } }
                                    }
                                }
                                td.small {
                                    @if x.users > 0 {
                                        a href=(detail_url(p.id, "", &Filters {
                                            environment: x.environment.clone(),
                                            release: Some(x.release.clone()),
                                            ..Filters::default()
                                        })) { "Board" }
                                    } @else {
                                        span.muted { "—" }
                                    }
                                }
                            }
                        } }
                    } }
                }
            }
            @if let Some(d) = delivery {
                (crate::delivery::delivery_card(d))
            }
            (hook_card(p, r, hook))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlens_engine::releases::Release;

    #[test]
    fn each_release_links_to_the_board_filtered_on_it() {
        let p = ProjectInfo {
            id: "demo",
            name: "Demo",
            key: "k",
        };
        let row = Release {
            release: "v2 <b>".into(),
            environment: "production".into(),
            state: State::RolledBack,
            share: 0.1,
            first_seen: "2026-11-20 10:00".into(),
            last_seen: "2026-11-21 10:00".into(),
            users: 8,
            events: 8,
            staging_secs: Some(90_000),
            staged_as: Some("2026-11-19.1".into()),
            deployed_at: Some("2026-11-20 09:58".into()),
            deploy: Some(Status::Succeeded),
            verdict: Some(Verdict::Regressed),
            reasons: vec!["New error: TypeError <x> (3 users, 5 times)".into()],
        };
        let failed = Release {
            release: "v3".into(),
            state: State::Failed,
            users: 0,
            deployed_at: None,
            deploy: Some(Status::Failed),
            ..row.clone()
        };
        let r = Releases {
            days: 90,
            timezone: "UTC".into(),
            measure: "distinct users per day",
            releases: vec![row, failed],
            deploys: Vec::new(),
            deploy_hook: None,
        };
        let hook = HookView {
            admin: true,
            new_url: None,
        };
        let html = releases(&p, &r, None, &hook).into_string();
        assert!(
            html.contains("/p/demo?env=production&amp;release=v2%20%3Cb%3E"),
            "{html}"
        );
        assert!(html.contains("rolled back"));
        assert!(html.contains("1 d 1 h"));
        assert!(!html.contains("v2 <b>"), "escaped");
        assert!(html.contains("2026-11-20 09:58"), "the deploy time");
        assert!(
            !html.contains("release=v3"),
            "no board link for a deploy with no traffic"
        );
        assert!(html.contains("Make the URL"));
        assert!(html.contains("regressed"), "the verdict");
        assert!(html.contains("TypeError &lt;x&gt;"), "its reason, escaped");
    }
}

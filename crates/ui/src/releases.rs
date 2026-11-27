//! The Releases page (#500, D-049): each release per environment, with its
//! state, when it was seen, its users, its time in staging and a link to
//! the board filtered on it. With the deploy hook (#504, D-053): each
//! release's deploy time and last deploy, failed deploys, and the hook's
//! URL, made or rotated here.

use super::*;
use crate::components::detail_url;
use crate::tables::Col;
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

/// A state as a tag: its tone, and an icon besides the colour when it is
/// bad.
fn state_tag(s: State) -> Markup {
    let (class, glyph) = match s {
        State::Live => ("tag tone-good", None),
        State::RollingOut | State::Deploying => ("tag tone-warn", None),
        State::RolledBack | State::Failed => ("tag tone-bad", Some("alert")),
        State::Superseded => ("tag", None),
    };
    html! { span class=(class) { @if let Some(g) = glyph { (icon(g)) } (s.label()) } }
}

/// A verdict as a tag: the word, a tone, and an icon so that the colour
/// is never the only signal (down for regressed, a tick for healthy).
fn verdict_tag(v: Verdict) -> Markup {
    let (class, glyph) = match v {
        Verdict::Healthy => ("tag tone-good", Some("task")),
        Verdict::Regressed => ("tag tone-bad", Some("down")),
        Verdict::Inconclusive => ("tag", None),
    };
    html! { span class=(class) { @if let Some(g) = glyph { (icon(g)) } (v.label()) } }
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

/// Releases (#500, #549): `releases.list`, one row per release with its
/// users compared with the release before it in the environment (as on
/// the board: arrow, sign and colour) and its verdict as a tag, the
/// delivery metrics (`releases.delivery`, #523) when they could be
/// computed, and the deploy hook.
pub fn releases(
    p: &ProjectInfo<'_>,
    r: &Releases,
    delivery: Option<&Delivery>,
    hook: &HookView<'_>,
) -> Markup {
    // Newest first within an environment: the one before is the next row
    // of the same environment.
    let before = |i: usize| {
        let x = r.releases.get(i)?;
        r.releases
            .iter()
            .skip(i + 1)
            .find(|y| y.environment == x.environment)
    };
    layout(
        "Releases",
        Some(p),
        html! {
            div.heading { div {
                h1 { "Releases" }
                p.muted { "Last " (r.days) " days, times in " (r.timezone) ". Each release is compared with the one before it in its environment; production is judged unless one environment is picked." }
            } }
            @if r.releases.is_empty() {
                section.card.empty aria-labelledby="rel-empty-h" {
                    (crate::tables::empty_art())
                    div {
                        h2 #rel-empty-h { "No release seen yet" }
                        p { "Pass a release to the SDK, or let the browser SDK name builds on its own (D-044). Deploys reported by the hook below show up here too." }
                    }
                }
            } @else {
                (crate::tables::TableCard {
                    label: "Releases",
                    head: html! {},
                    cols: &[
                        Col::Shown("Release"), Col::Shown("Environment"), Col::Shown("State"), Col::Num("Share"),
                        Col::Num("Users"), Col::Shown("Verdict"), Col::Shown("First seen"), Col::Shown("Last seen"),
                        Col::Shown("Deployed"), Col::Shown("Time in staging"), Col::Shown("Board"),
                    ],
                    body: html! { @for (i, x) in r.releases.iter().enumerate() {
                        tr {
                            th scope="row" { code { (x.release) } }
                            td { (x.environment) }
                            td { (state_tag(x.state)) }
                            td.num { (pct(x.share)) }
                            td.num {
                                (x.users)
                                @if let Some(prev) = before(i) {
                                    @if let Some(d) = crate::board::delta(x.users as f64, prev.users as f64, true, false) {
                                        br; (crate::board::delta_mark(&d)) span.muted.small { " vs " (prev.release) }
                                    }
                                }
                            }
                            td.small {
                                @match x.verdict {
                                    Some(v) => (verdict_tag(v)),
                                    None if x.reasons.iter().any(|r| r.starts_with("Waiting")) => span.muted { "waiting" },
                                    None => span.muted { "—" },
                                }
                                @if !x.reasons.is_empty() {
                                    ul.reasons { @for reason in x.reasons.iter().take(3) { li { (reason) } } }
                                }
                            }
                            td.when.muted { (x.first_seen) }
                            td.when.muted { (x.last_seen) }
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
                            td {
                                @if x.users > 0 {
                                    a href=(detail_url(p.id, "", &Filters {
                                        environment: x.environment.clone(),
                                        release: Some(x.release.clone()),
                                        ..Filters::default()
                                    })) title={ "Focus board on " (x.release) } { "Board →" }
                                } @else {
                                    span.muted { "—" }
                                }
                            }
                        }
                    } },
                    foot: html! {},
                }
                .render())
            }
            details.card.disclose {
                summary { h2 { "How releases are judged" } (icon("chev")) }
                div.body {
                    p.muted.small { "A release's share is its part of the environment's distinct users per day. "
                        strong { "Live" } ": at least 95 % on the latest day. "
                        strong { "Rolling out" } ": below that, not overtaken. "
                        strong { "Superseded" } ": a newer release took the majority. "
                        strong { "Rolled back" } ": it had the majority, then an older release took it back. "
                        strong { "Failed" } " and " strong { "deploying" } ": a deploy the hook reported, with no traffic. "
                        "Time in staging runs from the build's first event in staging to its first in production. " }
                    p.muted.small { strong { "Verdict" } ": once a release has 30 sessions and 8 users, or is a day old, it is compared with the release before it in the environment (errors, new error groups, bounce, tasks, Web Vitals, features with a success event, guardrails): "
                        strong { "healthy" } " when nothing got worse, " strong { "regressed" } " when something did, " strong { "inconclusive" } " with too little traffic or nothing to compare with. Production is judged unless one environment is picked. Users are compared with the release before it in the same environment." }
                }
            }
            @if let Some(d) = delivery {
                (crate::delivery::delivery_card(d))
            }
            (crate::release_notes::notes_card(&r.notes))
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
            notes: Default::default(),
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

    #[test]
    fn a_regressed_release_is_marked_beyond_colour_and_compared_with_the_one_before() {
        let p = ProjectInfo {
            id: "demo",
            name: "Demo",
            key: "k",
        };
        let old = Release {
            release: "v1".into(),
            environment: "production".into(),
            state: State::Superseded,
            share: 0.0,
            first_seen: "2026-11-10 10:00".into(),
            last_seen: "2026-11-20 10:00".into(),
            users: 10,
            events: 50,
            staging_secs: None,
            staged_as: None,
            deployed_at: None,
            deploy: None,
            verdict: Some(Verdict::Healthy),
            reasons: Vec::new(),
        };
        let new = Release {
            release: "v2".into(),
            state: State::Live,
            share: 1.0,
            users: 20,
            verdict: Some(Verdict::Regressed),
            reasons: vec!["Bounce went up".into()],
            ..old.clone()
        };
        let r = Releases {
            days: 90,
            timezone: "UTC".into(),
            measure: "distinct users per day",
            releases: vec![new, old],
            deploys: Vec::new(),
            deploy_hook: None,
            notes: Default::default(),
        };
        let hook = HookView {
            admin: false,
            new_url: None,
        };
        let html = releases(&p, &r, None, &hook).into_string();
        let row = |name: &str| {
            html.split("<tr>")
                .find(|r| r.contains(&format!("<code>{name}</code>")))
                .unwrap_or_default()
                .to_owned()
        };
        let (v2, v1) = (row("v2"), row("v1"));
        // Without colour, the regressed verdict still shows: an icon and the word.
        let regressed = verdict_tag(Verdict::Regressed).into_string();
        assert!(v2.contains(&regressed), "{v2}");
        assert!(regressed.contains("<svg") && regressed.contains("regressed"));
        assert!(!v1.contains(&regressed), "{v1}");
        // The newer release is compared with the one before in its
        // environment; the oldest has nothing to compare with.
        assert!(
            v2.contains("class=\"delta ") && v2.contains(" vs v1"),
            "{v2}"
        );
        assert!(!v1.contains("class=\"delta "), "{v1}");
        assert!(!html.contains("style=") && !html.contains("<script"));
    }
}

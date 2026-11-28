//! Tickets as commands (#245), moved out of the server as is (#503): close
//! the DMAIC loop. A finding (insight card, error group, goal, map health
//! finding) becomes a ticket in the app's backlog, prefilled for a person
//! or a coding agent (problem, evidence, links, hypothesis, success metric
//! with its check date, acceptance criteria). Copy it as Markdown, or open
//! a GitHub issue (one per finding: a second click updates it). Once the
//! issue is closed and a release has shipped it, the hourly pass measures
//! the metric (against the flag it shipped behind, else its normal weekly
//! range, D-108) and comments on the issue; every outcome
//! goes into the learning log.
//!
//! Nothing is posted without a click (or an admin API call): the hourly
//! pass only comments on tickets someone already created.
//!
//! The process that runs them (the server) keeps the transport: where the
//! GitHub token comes from, sealing it, and the HTTP calls
//! ([`Host::github`], [`Host::github_call`], [`Host::set_github_token`]).
//! It also keeps the schedule: its hourly job calls [`hourly`].

use chrono::NaiveDate;
use openlens_meta::{Meta, Project};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Access, Caller, Command, Error, Host, short_hash};
use crate::Filters;
use crate::goals;

mod verdict;
pub use self::verdict::*;

/// Days after the release before the effect is measured.
pub(crate) const CHECK_DAYS: i64 = 14;

/// Full weeks before the shipping release that set the normal range of a
/// weekly series (D-108).
const BEFORE_WEEKS: usize = 8;
/// Fewest full weeks before shipping for a control chart.
const MIN_BEFORE: usize = 4;
/// Fewest full weeks after shipping for a control chart.
const MIN_AFTER: usize = 2;
/// Width of the normal range, in standard deviations of the weekly values.
const SIGMAS: f64 = 3.0;
/// The range when the weeks before were flat (σ = 0): ± 5% of their mean.
const FLAT_BAND: f64 = 0.05;

/// The metric moved the right way beyond chance or normal variation
/// (D-108). Outcomes stored before it as `no effect` read as
/// [`INCONCLUSIVE`].
pub const IMPROVED: &str = "improved";
/// The metric moved the wrong way, beyond chance.
pub const WORSE: &str = "worse";
/// No change beyond normal variation or chance, or nothing to judge on.
pub const INCONCLUSIVE: &str = "inconclusive";

/// A ticket made from a finding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ticket {
    /// Stable id of the finding: kind + subject + project, not its numbers.
    pub fp: String,
    /// `insight`, `error`, `goal` or `map`.
    pub kind: String,
    /// What the finding is about, within its kind.
    pub subject: String,
    /// The ticket's title.
    pub title: String,
    /// KPI key the change should move (`error_rate`, `success:<feature>`…).
    #[serde(default)]
    pub metric: Option<String>,
    /// The metric when the ticket was drafted.
    #[serde(default)]
    pub baseline: Option<f64>,
    /// Sessions behind the baseline.
    #[serde(default)]
    pub baseline_n: i64,
    /// The metric the change aims for.
    #[serde(default)]
    pub target: Option<f64>,
    /// Higher is better for the metric.
    #[serde(default)]
    pub up: bool,
    /// Day it was drafted.
    pub created: String,
    /// GitHub issue, when one was opened: (number, URL).
    #[serde(default)]
    pub issue: Option<(u64, String)>,
    /// Day the issue was closed.
    #[serde(default)]
    pub closed: Option<String>,
    /// The first release seen after it was closed.
    #[serde(default)]
    pub shipped_in: Option<String>,
    /// `improved`, `worse` or `inconclusive` (or `no metric`), with the
    /// evidence: the method, the band or the p-value, the sample sizes.
    #[serde(default)]
    pub outcome: Option<(String, String)>,
    /// The flag the change shipped behind (#704): the verdict then
    /// compares the sessions with it against the others.
    #[serde(default)]
    pub flag: Option<String>,
    /// How the outcome was judged (#704): `flag`, `control_chart`,
    /// `before_after` or `none`.
    #[serde(default)]
    pub method: Option<String>,
}

/// A drafted ticket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    /// The ticket, not filed yet.
    pub ticket: Ticket,
    /// Its body, in Markdown.
    pub markdown: String,
}

/// Where a project files its issues, as the host sets it: GitHub's API
/// base, the repository (`owner/name`) and the token.
#[derive(Clone)]
pub struct GitHub {
    /// API base (`https://api.github.com`).
    pub api: String,
    /// `owner/name`.
    pub repo: String,
    /// The token the calls carry.
    pub token: String,
}

pub(crate) fn load(meta: &Meta, project: &str) -> Vec<Ticket> {
    let mut all: Vec<Ticket> = meta
        .project_setting(project, "tickets")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    read_old(&mut all);
    all
}

/// Outcomes stored before D-108 said `no effect`: they read as
/// [`INCONCLUSIVE`].
fn read_old(all: &mut [Ticket]) {
    for t in all {
        if let Some(o) = t.outcome.as_mut()
            && o.0 == "no effect"
        {
            o.0 = INCONCLUSIVE.to_owned();
        }
    }
}

fn save(meta: &Meta, project: &str, t: &[Ticket]) {
    if let Ok(j) = serde_json::to_string(t) {
        let _ = meta.set_project_setting(project, "tickets", &j);
    }
}

fn fmt(v: f64, share: bool) -> String {
    if share {
        format!("{:.1}%", v * 100.0)
    } else {
        format!("{v:.2}")
    }
}

fn filters(p: &Project) -> Filters {
    Filters {
        tz: p.timezone.clone(),
        ..Filters::default()
    }
}

/// The ticket for a finding, from what the board shows now. `host_url`
/// is the app's public address, for the links. `flag` names the flag the
/// change will ship behind; a flag card names its own.
///
/// # Errors
/// Why it cannot be drafted, for the person.
pub async fn draft<H: Host>(
    host: &H,
    p: &Project,
    kind: &str,
    subject: &str,
    host_url: &str,
    flag: Option<&str>,
) -> Result<Draft, String> {
    let flag = match flag.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) if crate::insights::valid_flag(f) => Some(f.to_owned()),
        Some(_) => return Err("A flag key is lowercase letters, digits, - and _.".into()),
        // A flag card (#704): the change ships behind that flag.
        None => match (kind, subject.split_once(':')) {
            ("insight", Some(("flag_regression" | "flag_win", key)))
                if crate::insights::valid_flag(key) =>
            {
                Some(key.to_owned())
            }
            _ => None,
        },
    };
    let f = filters(p);
    let map = host.feature_map(&p.id).await;
    let b = host
        .board(&p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
        .map_err(|e| e.to_string())?;
    let base = format!("{host_url}/p/{}", p.id);
    let release = b
        .releases
        .first()
        .map_or("—".to_owned(), |r| r.release.clone());
    let feature_of = |s: &str| {
        map.as_ref()
            .and_then(|(m, _)| {
                m.features
                    .iter()
                    .find(|x| x.key == s && x.success.is_some())
            })
            .map(|x| x.key.clone())
    };
    let (title, problem, evidence, links, metric, hypothesis): (
        String,
        String,
        Vec<String>,
        Vec<String>,
        Option<String>,
        String,
    ) = match kind {
        "insight" => {
            let (k, s) = subject.split_once(':').unwrap_or((subject, ""));
            let c = b
                .insights
                .iter()
                .find(|c| c.kind == k && c.subject == s)
                .ok_or("This card is not on the board any more.")?;
            let metric = feature_of(&c.subject)
                .map_or_else(|| "error_rate".to_owned(), |f| format!("success:{f}"));
            (
                c.title.clone(),
                c.title.clone(),
                vec![c.evidence.clone()],
                vec![
                    format!("Focus board: {base}"),
                    format!("Replays: {base}/replays"),
                ],
                Some(metric),
                format!(
                    "If we act on it ({}), the metric below improves.",
                    c.next_step
                ),
            )
        }
        // The board's lead when the latest release regressed (#615):
        // `<environment>:<release>`.
        "release" => {
            let (env, rel) = subject
                .split_once(':')
                .ok_or("Name the environment and the release.")?;
            let fe = Filters {
                environment: env.to_owned(),
                ..f.clone()
            };
            let on = host
                .board(&p.id, &fe, map.as_ref().map(|(m, v)| (m, *v)))
                .await
                .map_err(|e| e.to_string())?;
            let l = on
                .release_lead()
                .filter(|l| l.release == rel)
                .ok_or("This release is not judged regressed any more.")?;
            let reasons = on
                .latest_release
                .as_ref()
                .and_then(crate::board_verdict::LatestRelease::report)
                .map(|r| r.reasons.clone())
                .unwrap_or_default();
            (
                format!("Fix the regression of {rel}: {}", l.headline),
                l.headline.clone(),
                std::iter::once(l.evidence.clone()).chain(reasons).collect(),
                vec![
                    format!("Releases: {base}/releases?env={env}"),
                    format!("Focus board: {base}"),
                ],
                l.metric.clone(),
                format!(
                    "If we fix what {rel} broke ({}), the metric below gets back to its level before {rel}.",
                    l.next_step
                ),
            )
        }
        "error" => {
            let src = crate::source(map.as_ref().map(|(m, _)| m));
            let groups = crate::error_groups(host.query(), &p.id, &src, &f)
                .await
                .map_err(|e| e.to_string())?;
            let g = groups
                .iter()
                .find(|g| g.fingerprint == subject)
                .ok_or("This error group has not been seen in 30 days.")?;
            let title: String = format!("Fix: {}", g.sample.chars().take(90).collect::<String>());
            (
                title,
                format!(
                    "`{}` hits users.",
                    g.sample.chars().take(200).collect::<String>()
                ),
                vec![
                    format!(
                        "{} users, {} sessions, {} times in 30 days",
                        g.users, g.sessions, g.count
                    ),
                    format!(
                        "releases: {}",
                        g.releases
                            .iter()
                            .map(|(r, n)| format!("{r} ({n})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    format!("first seen {}, last seen {}", g.first_seen, g.last_seen),
                ],
                vec![
                    format!("Errors: {base}/errors"),
                    g.last_session.as_ref().map_or_else(String::new, |s| {
                        format!("Replay of the latest: {base}/replays/{s}")
                    }),
                ],
                Some("error_rate".into()),
                "If we fix this error, fewer sessions hit an error.".into(),
            )
        }
        "goal" => {
            let goals: Vec<goals::Goal> = host
                .meta()
                .project_setting(&p.id, "goals")
                .ok()
                .flatten()
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default();
            let g = goals
                .iter()
                .find(|g| g.id == subject)
                .ok_or("No such goal.")?;
            let (label, _, share) = goals::kpi_info(&g.kpi);
            (
                format!("Move {label} to {}", fmt(g.target, share)),
                format!("The goal on {label} is not reached yet."),
                vec![format!(
                    "from {} when set ({}), target {} by {}",
                    fmt(g.baseline, share),
                    g.created,
                    fmt(g.target, share),
                    g.deadline
                )],
                vec![format!("Goals: {base}/goals")],
                Some(g.kpi.clone()),
                format!(
                    "If we ship the change, {label} moves toward {}.",
                    fmt(g.target, share)
                ),
            )
        }
        "map" => {
            let (k, feat) = subject.split_once(':').unwrap_or((subject, ""));
            let (m, _) = map.as_ref().ok_or("No feature map.")?;
            let h = crate::map_health(host.query(), &p.id, m, &f)
                .await
                .map_err(|e| e.to_string())?;
            let x = h
                .findings
                .iter()
                .find(|x| x.kind == k && x.feature.as_deref().unwrap_or("") == feat)
                .ok_or("This finding is gone.")?;
            (
                format!("Feature map: {}", x.message),
                x.message.clone(),
                vec![x.evidence.clone()],
                vec![format!("Feature map: {base}/features")],
                None,
                "If we apply the change below, the map's health score goes up.".into(),
            )
        }
        _ => return Err("Unknown kind of finding.".into()),
    };
    let (current, n) = match metric.as_deref() {
        Some(k) => {
            let w = goals::weekly(host.query(), &p.id, map.as_ref().map(|(m, _)| m), &f)
                .await
                .unwrap_or_default();
            (goals::kpi_values_with(&b, &w, k).0, b.sessions.count)
        }
        None => (None, 0),
    };
    let (label, up, share) = metric
        .as_deref()
        .map_or((String::new(), true, false), goals::kpi_info);
    let target = current.map(|c| {
        let step = (c.abs() * 0.1).max(if share { 0.01 } else { 0.0 });
        let t = if up { c + step } else { c - step };
        if share { t.clamp(0.0, 1.0) } else { t.max(0.0) }
    });
    let check = (chrono::Utc::now() + chrono::Duration::days(CHECK_DAYS)).format("%Y-%m-%d");
    let mut md =
        format!("## Problem\n\n{problem}\n\n## Evidence (OpenLens, current release {release})\n\n");
    for e in &evidence {
        md.push_str(&format!("- {e}\n"));
    }
    md.push_str("\n## Links\n\n");
    for l in links.iter().filter(|l| !l.is_empty()) {
        md.push_str(&format!("- {l}\n"));
    }
    md.push_str(&format!("\n## Hypothesis\n\n{hypothesis}\n"));
    if let (Some(c), Some(t)) = (current, target) {
        md.push_str(&format!(
            "\n## Success metric\n\n- **{label}:** {} now → target {} ({} is better)\n- Checked by OpenLens {CHECK_DAYS} days after the release that ships the fix (not before {check}), and reported here.\n",
            fmt(c, share),
            fmt(t, share),
            if up { "higher" } else { "lower" }
        ));
        md.push_str(&match &flag {
            Some(f) => format!(
                "- Judged on the flag `{f}`: sessions with it against the others since the release, with a significance test.\n"
            ),
            None => "- Judged against the metric's normal weekly range before the release (mean ± 3σ of 8 weeks) when it has a weekly series, else before against after with a significance test for shares. A change within normal variation is inconclusive.\n".to_owned(),
        });
    }
    md.push_str("\n## Acceptance criteria\n\n- [ ] The cause is found (replay or error trace) and fixed.\n- [ ] The release that ships it is tagged (`release` in the SDK), so OpenLens can measure it.\n- [ ] No guardrail KPI gets worse.\n\n<sub>Opened from OpenLens. Fingerprint `");
    let fp = short_hash(&format!("{}|{kind}|{subject}", p.id));
    md.push_str(&fp);
    md.push_str("`.</sub>\n");
    Ok(Draft {
        ticket: Ticket {
            fp,
            kind: kind.to_owned(),
            subject: subject.to_owned(),
            title: title.chars().take(200).collect(),
            metric,
            baseline: current,
            baseline_n: n,
            target,
            up,
            created: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            issue: None,
            closed: None,
            shipped_in: None,
            outcome: None,
            flag,
            method: None,
        },
        markdown: md,
    })
}

/// Create the issue, or update the one already open for this finding.
///
/// # Errors
/// No repository set, or what GitHub answered.
pub async fn file<H: Host>(host: &H, project: &str, d: Draft) -> Result<Ticket, String> {
    let GitHub { api, repo, token } = host
        .github(project)
        .ok_or("No GitHub repository set for this project.")?;
    let mut all = load(host.meta(), project);
    let existing = all
        .iter()
        .position(|t| t.fp == d.ticket.fp && t.issue.is_some() && t.closed.is_none());
    let body = d.markdown.clone();
    let t = match existing.and_then(|i| all.get(i).cloned()) {
        Some(found) => {
            let (n, _) = found.issue.clone().unwrap_or_default();
            let url = format!("{api}/repos/{repo}/issues/{n}/comments");
            let c = format!(
                "Fresh numbers from OpenLens ({}):\n\n{body}",
                chrono::Utc::now().format("%Y-%m-%d")
            );
            host.github_call("POST", &url, &token, Some(json!({ "body": c })))
                .await?;
            // A flag named on a later click is kept for the verdict.
            match (existing.and_then(|i| all.get_mut(i)), d.ticket.flag) {
                (Some(t), Some(fl)) => {
                    t.flag = Some(fl);
                    t.clone()
                }
                _ => found,
            }
        }
        None => {
            let url = format!("{api}/repos/{repo}/issues");
            let title = d.ticket.title.clone();
            let v = host
                .github_call(
                    "POST",
                    &url,
                    &token,
                    Some(json!({ "title": title, "body": body, "labels": ["openlens"] })),
                )
                .await?;
            let mut t = d.ticket;
            t.issue = Some((
                v["number"].as_u64().unwrap_or(0),
                v["html_url"].as_str().unwrap_or("").to_owned(),
            ));
            all.push(t.clone());
            t
        }
    };
    save(host.meta(), project, &all);
    Ok(t)
}

/// The flag's cohorts on `metric` since the release, `days` ago.
async fn cohort_on<H: Host>(
    host: &H,
    p: &Project,
    map: Option<&openlens_featuremap::FeatureMap>,
    f: &Filters,
    flag: &str,
    metric: &str,
    days: i64,
) -> Option<Cohort> {
    let c = crate::insights::flag_cohort(host.query(), &p.id, &crate::source(map), f, flag, days)
        .await
        .ok()
        .flatten()?;
    cohort_for(&c, metric)
}

/// Hourly: follow open issues to their release, then measure and report
/// (a schedule, not a command: the server's job calls it).
pub async fn hourly<H: Host>(host: &H, p: &Project) {
    let mut all = load(host.meta(), &p.id);
    if all.iter().all(|t| t.issue.is_none() || t.outcome.is_some()) {
        return;
    }
    let Some(GitHub { api, repo, token }) = host.github(&p.id) else {
        return;
    };
    let f = filters(p);
    let map = host.feature_map(&p.id).await;
    let Ok(b) = host
        .board(&p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
    else {
        return;
    };
    let w = goals::weekly(host.query(), &p.id, map.as_ref().map(|(m, _)| m), &f)
        .await
        .unwrap_or_default();
    let today = chrono::Utc::now().date_naive();
    let mut changed = false;
    for t in all.iter_mut().filter(|t| t.outcome.is_none()) {
        let Some((n, _)) = t.issue.clone() else {
            continue;
        };
        if t.closed.is_none() {
            let url = format!("{api}/repos/{repo}/issues/{n}");
            if let Ok(v) = host.github_call("GET", &url, &token, None).await
                && v["state"] == "closed"
            {
                t.closed = Some(
                    v["closed_at"]
                        .as_str()
                        .unwrap_or("")
                        .get(..10)
                        .unwrap_or("")
                        .to_owned(),
                )
                .filter(|d| !d.is_empty())
                .or_else(|| Some(today.format("%Y-%m-%d").to_string()));
                changed = true;
            }
        }
        let Some(closed) = t.closed.clone() else {
            continue;
        };
        if t.shipped_in.is_none() {
            // The oldest release first seen on or after the close.
            t.shipped_in = b
                .releases
                .iter()
                .filter(|r| r.first_seen.get(..10).unwrap_or("") >= closed.as_str())
                .min_by(|a, c| a.first_seen.cmp(&c.first_seen))
                .map(|r| format!("{}|{}", r.release, r.first_seen.get(..10).unwrap_or("")));
            changed |= t.shipped_in.is_some();
        }
        let Some(shipped) = t.shipped_in.clone() else {
            continue;
        };
        let (rel, shipped_on) = shipped.split_once('|').unwrap_or((&shipped, ""));
        let Ok(shipped_on) = NaiveDate::parse_from_str(shipped_on, "%Y-%m-%d") else {
            continue;
        };
        if shipped_on + chrono::Duration::days(CHECK_DAYS) > today {
            continue;
        }
        let Some(metric) = t.metric.clone() else {
            t.outcome = Some(("no metric".into(), format!("shipped in {rel}")));
            changed = true;
            continue;
        };
        let (_, _, share) = goals::kpi_info(&metric);
        let Some(after) = goals::kpi_values_with(&b, &w, &metric).0 else {
            continue;
        };
        let series = goals::weekly_series(&w, &metric);
        if waiting(&series, shipped_on, today) {
            continue;
        }
        let cohort = match &t.flag {
            Some(fl) => {
                cohort_on(
                    host,
                    p,
                    map.as_ref().map(|(m, _)| m),
                    &f,
                    fl,
                    &metric,
                    (today - shipped_on).num_days(),
                )
                .await
            }
            None => None,
        };
        let v = verdict(
            t,
            &Measures {
                after,
                after_n: b.sessions.count,
                share,
                weekly: &series,
                shipped: Some(shipped_on),
                today,
                cohort,
            },
        );
        let url = format!("{api}/repos/{repo}/issues/{n}/comments");
        let body = format!(
            "**OpenLens: {}.** Shipped in `{rel}`; judged by {}. {}.{}",
            v.word,
            method_label(&v.method),
            v.evidence,
            if v.word == IMPROVED {
                ""
            } else {
                "\n\nNext: look at the replays of the sessions still affected, and try the next hypothesis."
            }
        );
        if host
            .github_call("POST", &url, &token, Some(json!({ "body": body })))
            .await
            .is_ok()
        {
            t.outcome = Some((v.word, v.evidence));
            t.method = Some(v.method);
            changed = true;
        }
    }
    if changed {
        save(host.meta(), &p.id, &all);
    }
}

async fn project<H: Host>(host: &H, id: &str) -> Result<Project, Error> {
    host.project(id).await.ok_or(Error::NotFound)
}

/// A project, for the commands that only name it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInput {
    /// Project id.
    pub project: String,
}

/// How many measured tickets moved their metric the right way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Learning {
    /// Tickets with a verdict (`no metric` is not one).
    pub measured: usize,
    /// Of those, the ones that `improved`.
    pub improved: usize,
    /// The ones that got `worse`.
    pub worse: usize,
    /// The ones judged `inconclusive` (D-108).
    pub inconclusive: usize,
}

impl Learning {
    /// Count the verdicts of `tickets` (pure).
    #[must_use]
    pub fn of(tickets: &[Ticket]) -> Self {
        let count = |w: &str| {
            tickets
                .iter()
                .filter(|t| t.outcome.as_ref().is_some_and(|o| o.0 == w))
                .count()
        };
        let (improved, worse, inconclusive) = (count(IMPROVED), count(WORSE), count(INCONCLUSIVE));
        Self {
            measured: improved + worse + inconclusive,
            improved,
            worse,
            inconclusive,
        }
    }
}

/// A project's tickets and its learning log.
#[derive(Debug, Clone, Serialize)]
pub struct Tickets {
    /// Every ticket, oldest first.
    pub tickets: Vec<Ticket>,
    /// The learning log's counts.
    pub learning: Learning,
    /// The repository set for the project (`owner/name`), or empty.
    pub repo: String,
    /// Whether issues can be filed (a repository and a token are set).
    pub github: bool,
}

/// `tickets.list`: a project's tickets and the learning log.
pub struct List;

impl Command for List {
    const NAME: &'static str = "tickets.list";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Tickets;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProjectInput) -> Result<Tickets, Error> {
        let meta = host.meta();
        let tickets = load(meta, &input.project);
        let learning = Learning::of(&tickets);
        let repo = meta
            .project_setting(&input.project, "tickets_repo")
            .ok()
            .flatten()
            .unwrap_or_default();
        Ok(Tickets {
            learning,
            repo,
            github: host.github(&input.project).is_some(),
            tickets,
        })
    }
}

/// The input of `tickets.draft` and `tickets.file`: a finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingInput {
    /// Project id.
    pub project: String,
    /// `insight`, `error`, `goal` or `map`.
    pub kind: String,
    /// The finding within its kind (`<kind>:<subject>` of an insight
    /// card, an error fingerprint, a goal id, `<kind>:<feature>` of a map
    /// finding).
    pub subject: String,
    /// The app's public address, for the links in the ticket.
    pub host: String,
    /// The flag the change will ship behind (#704): the verdict then
    /// compares its sessions with the others. A flag card names its own.
    #[serde(default)]
    pub flag: Option<String>,
}

/// What `tickets.draft` gives.
#[derive(Debug, Clone, Serialize)]
pub struct Drafted {
    /// The ticket and its Markdown.
    #[serde(flatten)]
    pub draft: Draft,
    /// The GitHub repository issues go to, when one can be filed.
    pub repo: Option<String>,
    /// The open issue already filed for this finding, as its URL.
    pub existing: Option<String>,
}

/// `tickets.draft`: the ticket for a finding, not filed. Errors are
/// [`Error::Invalid`], with the reason for the person.
pub struct DraftTicket;

impl Command for DraftTicket {
    const NAME: &'static str = "tickets.draft";
    const ACCESS: Access = Access::Read;
    type Input = FindingInput;
    type Output = Drafted;

    async fn run<H: Host>(host: &H, _: &Caller, input: FindingInput) -> Result<Drafted, Error> {
        let p = project(host, &input.project).await?;
        let d = draft(
            host,
            &p,
            &input.kind,
            &input.subject,
            &input.host,
            input.flag.as_deref(),
        )
        .await
        .map_err(Error::Invalid)?;
        let existing = load(host.meta(), &p.id)
            .into_iter()
            .find(|t| t.fp == d.ticket.fp && t.closed.is_none())
            .and_then(|t| t.issue)
            .map(|i| i.1);
        Ok(Drafted {
            draft: d,
            repo: host.github(&p.id).map(|g| g.repo),
            existing,
        })
    }
}

/// `tickets.file`: draft the ticket for a finding and open its GitHub
/// issue, or comment on the one already open. A finding that cannot be
/// drafted is [`Error::Invalid`]; a repository not set or GitHub's refusal
/// is [`Error::Unavailable`].
pub struct File;

impl Command for File {
    const NAME: &'static str = "tickets.file";
    const ACCESS: Access = Access::Write;
    type Input = FindingInput;
    type Output = Ticket;

    async fn run<H: Host>(host: &H, _: &Caller, input: FindingInput) -> Result<Ticket, Error> {
        let p = project(host, &input.project).await?;
        let d = draft(
            host,
            &p,
            &input.kind,
            &input.subject,
            &input.host,
            input.flag.as_deref(),
        )
        .await
        .map_err(Error::Invalid)?;
        file(host, &p.id, d).await.map_err(Error::Unavailable)
    }
}

/// The input of `tickets.target`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInput {
    /// Project id.
    pub project: String,
    /// `owner/name` or its `https://github.com/` address; empty unsets it.
    #[serde(default)]
    pub repo: String,
    /// GitHub token, shared by every project, stored sealed; empty keeps
    /// the stored one.
    #[serde(default)]
    pub token: String,
}

/// `tickets.target`: the GitHub repository a project files its issues in
/// (and the token). Gives the repository as stored. A malformed repository
/// is [`Error::Invalid`]; a token that cannot be sealed is
/// [`Error::Storage`].
pub struct SetTarget;

impl Command for SetTarget {
    const NAME: &'static str = "tickets.target";
    const ACCESS: Access = Access::Write;
    type Input = TargetInput;
    type Output = String;

    async fn run<H: Host>(host: &H, _: &Caller, input: TargetInput) -> Result<String, Error> {
        let repo = input
            .repo
            .trim()
            .trim_start_matches("https://github.com/")
            .trim_end_matches('/');
        let ok = repo.is_empty()
            || (repo.split('/').count() == 2
                && repo
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b)));
        if !ok {
            return Err(Error::Invalid("Write owner/name.".into()));
        }
        let _ = host
            .meta()
            .set_project_setting(&input.project, "tickets_repo", repo);
        if !input.token.trim().is_empty() {
            host.set_github_token(input.token.trim())
                .map_err(Error::Storage)?;
        }
        Ok(repo.to_owned())
    }
}

#[cfg(test)]
mod tests;

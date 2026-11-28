//! Tickets as commands (#245), moved out of the server as is (#503): close
//! the DMAIC loop. A finding (insight card, error group, goal, map health
//! finding) becomes a ticket in the app's backlog, prefilled for a person
//! or a coding agent (problem, evidence, links, hypothesis, success metric
//! with its check date, acceptance criteria). Copy it as Markdown, or open
//! a GitHub issue (one per finding: a second click updates it). Once the
//! issue is closed and a release has shipped it, the hourly pass measures
//! the metric (against the flag it shipped behind, else its normal weekly
//! range, D-098) and comments on the issue; every outcome
//! goes into the learning log.
//!
//! Nothing is posted without a click (or an admin API call): the hourly
//! pass only comments on tickets someone already created.
//!
//! The process that runs them (the server) keeps the transport: where the
//! GitHub token comes from, sealing it, and the HTTP calls
//! ([`Host::github`], [`Host::github_call`], [`Host::set_github_token`]).
//! It also keeps the schedule: its hourly job calls [`hourly`].

use chrono::{Datelike, NaiveDate};
use openlens_meta::{Meta, Project};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Access, Caller, Command, Error, Host, short_hash};
use crate::goals;
use crate::{Filters, FlagCohort};

/// Days after the release before the effect is measured.
pub(crate) const CHECK_DAYS: i64 = 14;

/// Full weeks before the shipping release that set the normal range of a
/// weekly series (D-098).
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
/// (D-098). Outcomes stored before it as `no effect` read as
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

/// Outcomes stored before D-098 said `no effect`: they read as
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

/// A flag's sessions against the others on a ticket's metric (#704).
#[derive(Debug, Clone, PartialEq)]
pub struct Cohort {
    /// Flag key.
    pub flag: String,
    /// The metric over the sessions with the flag on.
    pub with: f64,
    /// Those sessions.
    pub with_n: i64,
    /// The metric over the other sessions.
    pub without: f64,
    /// Those sessions.
    pub without_n: i64,
    /// p-value of the difference (`None`: under 30 sessions on a side).
    pub p: Option<f64>,
}

/// The cohorts of a flag on `metric`, for the metrics they measure with a
/// test: sessions with errors and tasks per session (pure).
#[must_use]
pub fn cohort_for(c: &FlagCohort, metric: &str) -> Option<Cohort> {
    let (w, o) = (&c.with, &c.without);
    let (with, without, p) = match metric {
        "error_rate" => (w.error_rate, o.error_rate, c.p_errors),
        "tasks_per_session" => (w.tasks_per_session, o.tasks_per_session, c.p_tasks),
        _ => return None,
    };
    Some(Cohort {
        flag: c.flag.clone(),
        with,
        with_n: w.sessions,
        without,
        without_n: o.sessions,
        p,
    })
}

/// What a ticket's metric is judged on, once its check date has come.
#[derive(Debug, Clone)]
pub struct Measures<'a> {
    /// The metric now.
    pub after: f64,
    /// Sessions behind it.
    pub after_n: i64,
    /// The metric is a share (0–1).
    pub share: bool,
    /// The metric's weekly series (Monday, value), oldest first; empty
    /// when it has none.
    pub weekly: &'a [(String, f64)],
    /// The day the release that shipped the change was first seen.
    pub shipped: Option<NaiveDate>,
    /// Today.
    pub today: NaiveDate,
    /// The ticket's flag on the metric, when it has one and it measures it.
    pub cohort: Option<Cohort>,
}

/// A verdict on a change (D-098).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// [`IMPROVED`], [`WORSE`] or [`INCONCLUSIVE`].
    pub word: String,
    /// `flag`, `control_chart`, `before_after` or `none`.
    pub method: String,
    /// The method, the band or the p-value, and the sample sizes.
    pub evidence: String,
}

impl Verdict {
    fn new(word: &str, method: &str, evidence: String) -> Self {
        Self {
            word: word.to_owned(),
            method: method.to_owned(),
            evidence,
        }
    }
}

/// The word for a change `delta` that is clear (or not) (pure).
fn word(clear: bool, delta: f64, up: bool) -> &'static str {
    if !clear || delta.abs() < 1e-12 {
        INCONCLUSIVE
    } else if (delta > 0.0) == up {
        IMPROVED
    } else {
        WORSE
    }
}

fn day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok()
}

/// The values of the full weeks before shipping (the last
/// [`BEFORE_WEEKS`]) and of the full weeks after it, ended by `today`
/// (pure).
fn split_weeks(
    weekly: &[(String, f64)],
    shipped: NaiveDate,
    today: NaiveDate,
) -> (Vec<f64>, Vec<f64>) {
    let mut weeks: Vec<(NaiveDate, f64)> = weekly
        .iter()
        .filter(|(_, v)| v.is_finite())
        .filter_map(|(d, v)| Some((day(d)?, *v)))
        .collect();
    weeks.sort_by_key(|w| w.0);
    let week = chrono::Duration::days(7);
    let before: Vec<f64> = weeks
        .iter()
        .filter(|(m, _)| *m + week <= shipped)
        .map(|w| w.1)
        .collect();
    let skip = before.len().saturating_sub(BEFORE_WEEKS);
    let before = before.into_iter().skip(skip).collect();
    let after = weeks
        .iter()
        .filter(|(m, _)| *m >= shipped && *m + week <= today)
        .map(|w| w.1)
        .collect();
    (before, after)
}

/// Full weeks (Monday to Sunday) between `shipped` and `today` (pure).
fn full_weeks_after(shipped: NaiveDate, today: NaiveDate) -> usize {
    let to_monday = (7 - i64::from(shipped.weekday().num_days_from_monday())) % 7;
    let first = shipped + chrono::Duration::days(to_monday);
    usize::try_from((today - first).num_days().div_euclid(7)).unwrap_or(0)
}

/// A control chart can be drawn but its second full week after shipping
/// has not ended yet: the check waits for it (pure).
#[must_use]
pub fn waiting(weekly: &[(String, f64)], shipped: NaiveDate, today: NaiveDate) -> bool {
    split_weeks(weekly, shipped, today).0.len() >= MIN_BEFORE
        && full_weeks_after(shipped, today) < MIN_AFTER
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

/// Sample standard deviation.
fn sd(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

fn flag_verdict(t: &Ticket, c: &Cohort, share: bool) -> Verdict {
    let delta = c.with - c.without;
    let clear = c.p.is_some_and(|p| p < crate::stats::ALPHA);
    let ev = format!(
        "Flag cohorts: {} with `{}` ({} sessions) vs {} without it ({} sessions) since the release · {}",
        fmt(c.with, share),
        c.flag,
        c.with_n,
        fmt(c.without, share),
        c.without_n,
        c.p.map_or_else(
            || format!(
                "under {} sessions on a side, no test",
                crate::stats::MIN_SESSIONS
            ),
            |p| format!("p = {p:.3}")
        )
    );
    Verdict::new(word(clear, delta, t.up), "flag", ev)
}

fn chart_verdict(t: &Ticket, before: &[f64], after: &[f64], share: bool, note: &str) -> Verdict {
    let (m, s) = (mean(before), sd(before));
    let flat = s <= 1e-12;
    let band = if flat {
        m.abs() * FLAT_BAND
    } else {
        SIGMAS * s
    };
    let a = mean(after);
    let delta = a - m;
    let ev = format!(
        "{note}Control chart: {} a week over the {} full weeks after the release vs {} over the {} weeks before; normal range {} to {} ({})",
        fmt(a, share),
        after.len(),
        fmt(m, share),
        before.len(),
        fmt(m - band, share),
        fmt(m + band, share),
        if flat {
            "± 5%: the weeks before were flat".to_owned()
        } else {
            format!("mean ± 3σ, σ = {}", fmt(s, share))
        }
    );
    Verdict::new(word(delta.abs() > band, delta, t.up), "control_chart", ev)
}

/// The verdict on a change (pure, D-098): the flag's cohorts when it
/// shipped behind one, else a control chart of the metric's weekly
/// series, else (not enough history) a before/after significance test
/// for shares; anything else is inconclusive.
#[must_use]
pub fn verdict(t: &Ticket, m: &Measures<'_>) -> Verdict {
    let mut note = String::new();
    match (&m.cohort, &t.flag) {
        (Some(c), _) if c.with_n > 0 && c.without_n > 0 => {
            return flag_verdict(t, c, m.share);
        }
        (Some(c), _) => {
            note = format!("Flag `{}` not seen on sessions since the release. ", c.flag);
        }
        (None, Some(f)) => {
            note = format!("Flag `{f}` has no cohort reading of this metric. ");
        }
        (None, None) => {}
    }
    let before = match m.shipped {
        Some(d) => {
            let (before, after) = split_weeks(m.weekly, d, m.today);
            if before.len() >= MIN_BEFORE && after.len() >= MIN_AFTER {
                return chart_verdict(t, &before, &after, m.share, &note);
            }
            before.len()
        }
        None => 0,
    };
    let history = if m.weekly.is_empty() {
        "no weekly series for this metric".to_owned()
    } else {
        format!("{before} full weeks before the release ({MIN_BEFORE} needed)")
    };
    let Some(b) = t.baseline else {
        return Verdict::new(
            INCONCLUSIVE,
            "none",
            format!("{note}No test: no baseline, and {history}"),
        );
    };
    let delta = m.after - b;
    let rel = if b.abs() > 1e-12 {
        delta / b.abs()
    } else {
        0.0
    };
    let moved = format!(
        "{} before ({} sessions) → {} after ({} sessions) ({:+.1}%)",
        fmt(b, m.share),
        t.baseline_n,
        fmt(m.after, m.share),
        m.after_n,
        rel * 100.0
    );
    if !m.share {
        return Verdict::new(
            INCONCLUSIVE,
            "none",
            format!("{note}No test: {moved}; {history}, and only shares get a before/after test"),
        );
    }
    let p = crate::stats::two_proportions(m.after, m.after_n, b, t.baseline_n);
    let ev = format!(
        "{note}Before/after test ({history}): {moved} · {}",
        p.map_or_else(
            || format!(
                "under {} sessions on a side, no test",
                crate::stats::MIN_SESSIONS
            ),
            |p| format!("p = {p:.3}")
        )
    );
    let clear = p.is_some_and(|p| p < crate::stats::ALPHA);
    Verdict::new(word(clear, delta, t.up), "before_after", ev)
}

/// How a method reads in the GitHub comment.
fn method_label(method: &str) -> &'static str {
    match method {
        "flag" => "flag cohorts",
        "control_chart" => "control chart of the weekly series",
        "before_after" => "before/after test",
        _ => "no test",
    }
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
    /// The ones judged `inconclusive` (D-098).
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
mod tests {
    use super::*;

    fn t(baseline: f64, n: i64, up: bool) -> Ticket {
        Ticket {
            fp: "f".into(),
            kind: "error".into(),
            subject: "s".into(),
            title: "t".into(),
            metric: Some("error_rate".into()),
            baseline: Some(baseline),
            baseline_n: n,
            target: None,
            up,
            created: "2026-11-01".into(),
            issue: None,
            closed: None,
            shipped_in: None,
            outcome: None,
            flag: None,
            method: None,
        }
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Shipped on Monday 2026-11-02: `before` are the weeks just before
    /// it (oldest first), `after` the weeks from it.
    fn series(before: &[f64], after: &[f64]) -> Vec<(String, f64)> {
        let ship = d("2026-11-02");
        let n = before.len() as i64;
        before
            .iter()
            .enumerate()
            .map(|(i, v)| (ship - chrono::Duration::weeks(n - i as i64), *v))
            .chain(
                after
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (ship + chrono::Duration::weeks(i as i64), *v)),
            )
            .map(|(m, v)| (m.format("%Y-%m-%d").to_string(), v))
            .collect()
    }

    fn on(weekly: &[(String, f64)], after: f64, n: i64, share: bool) -> Measures<'_> {
        Measures {
            after,
            after_n: n,
            share,
            weekly,
            shipped: Some(d("2026-11-02")),
            today: d("2026-11-16"),
            cohort: None,
        }
    }

    fn cohort(with: f64, with_n: i64, without: f64, without_n: i64) -> Cohort {
        Cohort {
            flag: "new-checkout".into(),
            with,
            with_n,
            without,
            without_n,
            p: crate::stats::two_proportions(with, with_n, without, without_n),
        }
    }

    const NORMAL: [f64; 8] = [100.0, 110.0, 90.0, 105.0, 95.0, 100.0, 108.0, 92.0];

    #[test]
    fn a_change_within_normal_weekly_variation_is_inconclusive() {
        let w = series(&NORMAL, &[104.0, 106.0]);
        let v = verdict(&t(100.0, 500, true), &on(&w, 105.0, 500, false));
        assert_eq!(
            (v.word.as_str(), v.method.as_str()),
            (INCONCLUSIVE, "control_chart")
        );
        assert!(
            v.evidence.contains("3σ") && v.evidence.contains("8 weeks before"),
            "{}",
            v.evidence
        );
    }

    #[test]
    fn a_step_outside_the_band_is_judged_in_the_metrics_direction() {
        let up = series(&NORMAL, &[160.0, 165.0]);
        let down = series(&NORMAL, &[40.0, 45.0]);
        let judge = |w: &[(String, f64)], better_up: bool| {
            verdict(&t(100.0, 500, better_up), &on(w, 0.0, 500, false)).word
        };
        assert_eq!(judge(&up, true), IMPROVED);
        assert_eq!(judge(&up, false), WORSE);
        assert_eq!(judge(&down, true), WORSE);
        assert_eq!(judge(&down, false), IMPROVED);
        // Flat weeks before: a 5% band around their mean.
        let flat = [100.0; 6];
        assert_eq!(judge(&series(&flat, &[103.0, 104.0]), true), INCONCLUSIVE);
        assert_eq!(judge(&series(&flat, &[110.0, 112.0]), true), IMPROVED);
    }

    #[test]
    fn the_chart_needs_four_weeks_before_and_waits_for_two_after() {
        // Three weeks of history: not a chart.
        let short = series(&[100.0, 110.0, 90.0], &[160.0, 165.0]);
        let v = verdict(&t(100.0, 500, true), &on(&short, 160.0, 500, false));
        assert_ne!(v.method, "control_chart");
        // Shipped on a Wednesday, checked 14 days later: one full week
        // after, so the check waits; a few days on, it can run.
        let w = series(&NORMAL, &[]);
        assert!(waiting(&w, d("2026-11-04"), d("2026-11-18")));
        assert!(!waiting(&w, d("2026-11-04"), d("2026-11-23")));
        assert!(
            !waiting(&[], d("2026-11-04"), d("2026-11-18")),
            "no series: no wait"
        );
    }

    #[test]
    fn a_flagged_ticket_is_judged_on_its_cohorts() {
        let mut tk = t(0.2, 500, false);
        tk.flag = Some("new-checkout".into());
        // The weekly series shows nothing, the cohorts a clear drop in errors.
        let w = series(&NORMAL, &[100.0, 101.0]);
        let mut m = on(&w, 0.2, 500, true);
        m.cohort = Some(cohort(0.05, 2000, 0.10, 2000));
        let v = verdict(&tk, &m);
        assert_eq!((v.word.as_str(), v.method.as_str()), (IMPROVED, "flag"));
        assert!(v.evidence.contains("2000 sessions") && v.evidence.contains("p = "));
        // More errors with the flag: worse.
        m.cohort = Some(cohort(0.10, 2000, 0.05, 2000));
        assert_eq!(verdict(&tk, &m).word, WORSE);
        // The same gap on a handful of sessions: no test, inconclusive.
        m.cohort = Some(cohort(0.05, 10, 0.10, 2000));
        assert_eq!(verdict(&tk, &m).word, INCONCLUSIVE);
        // Not seen on any session: judged without the flag.
        m.cohort = Some(cohort(0.0, 0, 0.10, 2000));
        assert_ne!(verdict(&tk, &m).method, "flag");
    }

    #[test]
    fn cohorts_measure_errors_and_tasks_only() {
        let c = FlagCohort {
            flag: "f".into(),
            with: crate::CohortStats {
                sessions: 40,
                error_rate: 0.1,
                ..Default::default()
            },
            without: crate::CohortStats::default(),
            p_errors: Some(0.5),
            p_tasks: None,
        };
        assert!(cohort_for(&c, "error_rate").is_some_and(|x| x.with_n == 40 && x.p == c.p_errors));
        assert!(cohort_for(&c, "avg_secs").is_none());
    }

    #[test]
    fn without_history_shares_keep_the_test_and_other_metrics_are_inconclusive() {
        // Errors 20% → 10% on 500 sessions each: improved, significant.
        let v = verdict(&t(0.2, 500, false), &on(&[], 0.1, 500, true));
        assert_eq!(
            (v.word.as_str(), v.method.as_str()),
            (IMPROVED, "before_after")
        );
        assert!(v.evidence.contains("500 sessions") && v.evidence.contains("p = "));
        // 20% → 19%: within chance.
        assert_eq!(
            verdict(&t(0.2, 500, false), &on(&[], 0.19, 500, true)).word,
            INCONCLUSIVE
        );
        // Errors up: worse.
        assert_eq!(
            verdict(&t(0.1, 500, false), &on(&[], 0.2, 500, true)).word,
            WORSE
        );
        // Too few sessions for a test: no 5% rule any more.
        assert_eq!(
            verdict(&t(0.2, 10, false), &on(&[], 0.1, 10, true)).word,
            INCONCLUSIVE
        );
        // Not a share, no weekly series: inconclusive, even on a large move.
        let v = verdict(&t(100.0, 0, true), &on(&[], 150.0, 0, false));
        assert_eq!((v.word.as_str(), v.method.as_str()), (INCONCLUSIVE, "none"));
    }

    #[test]
    fn the_learning_log_counts_the_new_words_and_reads_the_old_one() {
        let with = |w: &str| Ticket {
            outcome: Some((w.into(), String::new())),
            ..t(0.1, 1, false)
        };
        let mut all = vec![
            with(IMPROVED),
            with(WORSE),
            with("no effect"),
            with("no metric"),
            t(0.1, 1, false),
        ];
        read_old(&mut all);
        let l = Learning::of(&all);
        assert_eq!(l.inconclusive, 1, "no effect reads as inconclusive");
        assert_eq!(l.measured, l.improved + l.worse + l.inconclusive);
        assert!(
            l.measured < all.len(),
            "no metric and waiting are not measured"
        );
    }
}

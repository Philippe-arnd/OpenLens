//! Next actions (#715, D-114): one queue of findings that the PM and the
//! agents share. The findings of the board (insight cards, the regressed
//! latest release), the error groups, the goals at risk or off track and
//! the map health errors are ranked, deduplicated and given a state:
//!
//! `new` → `proposed` → `approved` → `shipped` → `learned`, or `dismissed`.
//!
//! The state comes from the data that already exists: a finding with a
//! filed ticket is `approved`, its issue closed and seen in a release is
//! `shipped`, and its outcome recorded by the hourly pass is `learned`.
//! Measuring and writing the outcome in the learning log are one step of
//! that pass, so `measured` and `learned` are one state, `learned`. What
//! this module stores is what no other data says: an agent's proposal (a
//! ticket drafted, never posted), a person's approval, and a person's
//! dismissal (hidden for [`DISMISS_DAYS`] days). A finding that leaves the
//! board while `new` leaves the list; one that went further stays until
//! it is learned.
//!
//! An agent proposes ([`Propose`]); a person approves ([`Approve`]), which
//! files the ticket as the Tickets page's *Create ticket* does. Proposing
//! twice, or approving twice, does nothing more.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use openlens_meta::{Meta, Project};
use serde::{Deserialize, Serialize};

use super::tickets::{self, Draft, Ticket};
use super::{Access, Caller, Command, Error, Host, short_hash};
use crate::board_verdict::Lead;
use crate::goals::Status;
use crate::{Board, ErrorGroup, Filters, Insight};

/// Actions a list gives at most.
pub const MAX_LISTED: usize = 20;
/// Actions a project keeps at most (oldest learned or dismissed dropped
/// first).
pub const MAX_TRACKED: usize = 200;
/// Days a dismissed action stays hidden.
pub const DISMISS_DAYS: i64 = 30;
/// Error groups taken from the Errors page, the most users first.
const ERROR_GROUPS: usize = 5;
/// Longest note or reason kept, in characters.
const MAX_TEXT: usize = 500;

/// Where an action stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Seen, nobody acted on it yet.
    New,
    /// An agent proposes to work on it: its ticket is drafted, not filed.
    Proposed,
    /// A person filed its ticket (approved a proposal, or created the
    /// ticket on the Tickets page).
    Approved,
    /// Its issue is closed and a release shipped it.
    Shipped,
    /// The hourly pass measured it and wrote the outcome in the learning
    /// log (one step, D-114).
    Learned,
    /// A person hid it for [`DISMISS_DAYS`] days.
    Dismissed,
}

impl State {
    /// The lowercase word (`approved`).
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Shipped => "shipped",
            Self::Learned => "learned",
            Self::Dismissed => "dismissed",
        }
    }

    /// The state named `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::New,
            Self::Proposed,
            Self::Approved,
            Self::Shipped,
            Self::Learned,
            Self::Dismissed,
        ]
        .into_iter()
        .find(|x| x.word() == s)
    }
}

/// A finding as it is now, ranked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// `insight`, `release`, `error`, `goal` or `map` (the kinds of
    /// `tickets.draft`).
    pub kind: String,
    /// The finding within its kind, as `tickets.draft` takes it.
    pub subject: String,
    /// One line.
    pub title: String,
    /// The numbers behind it.
    pub evidence: Vec<String>,
    /// Where to look, as paths of the app (`/p/<id>/errors`).
    pub links: Vec<String>,
    /// How much it matters, 0–1.
    pub impact: f64,
    /// How sure the finding is, 0–1.
    pub confidence: f64,
}

impl Finding {
    /// Impact × confidence.
    #[must_use]
    pub fn rank(&self) -> f64 {
        self.impact * self.confidence
    }
}

/// An agent's proposal: the ticket drafted when it proposed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Day it was proposed.
    pub day: String,
    /// What the agent plans, in its words.
    #[serde(default)]
    pub note: String,
    /// The ticket, drafted then, not filed.
    pub draft: Draft,
}

/// A person's dismissal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dismissal {
    /// Hidden until this day (excluded).
    pub until: String,
    /// Why.
    pub reason: String,
}

/// What a project keeps of an action (`project:<id>:actions`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tracked {
    /// Stable id: the ticket's fingerprint of the finding.
    pub id: String,
    /// The finding as it was last acted on.
    pub finding: Finding,
    /// Day it was first acted on.
    pub since: String,
    /// The agent's proposal.
    #[serde(default)]
    pub proposed: Option<Proposal>,
    /// Day a person approved it.
    #[serde(default)]
    pub approved: Option<String>,
    /// The dismissal, while it lasts or until it is replaced.
    #[serde(default)]
    pub dismissed: Option<Dismissal>,
}

/// The ticket of an action, as the Tickets page shows it.
#[derive(Debug, Clone, Serialize)]
pub struct TicketLink {
    /// The GitHub issue's address, once filed.
    pub url: Option<String>,
    /// The release that shipped it.
    pub shipped_in: Option<String>,
    /// `improved`, `worse` or `inconclusive` (or `no metric`), with its
    /// evidence.
    pub outcome: Option<(String, String)>,
    /// How the outcome was judged.
    pub method: Option<String>,
}

/// One action of the list.
#[derive(Debug, Clone, Serialize)]
pub struct Action {
    /// Stable id (kind + subject, hashed like a ticket's fingerprint).
    pub id: String,
    /// Where it stands.
    pub state: State,
    /// The finding: kind, subject, title, evidence, links, impact and
    /// confidence.
    #[serde(flatten)]
    pub finding: Finding,
    /// Impact × confidence (D-114).
    pub rank: f64,
    /// Still among the findings now (board, errors, goals, map health).
    pub current: bool,
    /// The agent's proposal: day and note.
    pub proposed: Option<(String, String)>,
    /// The drafted ticket of a proposal waiting for approval, in Markdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// The filed ticket.
    pub ticket: Option<TicketLink>,
    /// The dismissal, while it lasts.
    pub dismissed: Option<Dismissal>,
}

/// What `actions.next` gives.
#[derive(Debug, Clone, Serialize)]
pub struct NextActions {
    /// The actions, highest rank first (at most [`MAX_LISTED`]).
    pub actions: Vec<Action>,
    /// Actions left out by the limit.
    pub more: usize,
    /// Actions hidden by a dismissal now.
    pub dismissed: usize,
    /// Sources that could not be read, with why: their findings are
    /// missing from the list.
    pub unavailable: Vec<String>,
}

/// The id of a finding: the ticket's fingerprint, so a ticket filed on
/// the Tickets page belongs to the same action.
#[must_use]
pub fn id_of(project: &str, kind: &str, subject: &str) -> String {
    short_hash(&format!("{project}|{kind}|{subject}"))
}

// Ranking (D-114): rank = impact × confidence, both 0–1, fixed per kind.
//
// | kind    | impact                                      | confidence |
// |---------|---------------------------------------------|------------|
// | release | 1 (everyone on the latest release)          | 0.9        |
// | goal    | 0.8 off track, 0.6 at risk                  | 0.7        |
// | error   | log10(1 + users) / 3, at most 1             | 0.9        |
// | insight | score / (score + 5)                         | 0.6        |
// | map     | log10(1 + affected) / 8, at most 0.5        | 0.8        |

/// The regressed latest release of `env` (pure).
#[must_use]
pub fn from_lead(pid: &str, env: &str, l: &Lead) -> Finding {
    Finding {
        kind: "release".into(),
        subject: format!("{env}:{}", l.release),
        title: format!("{} regressed: {}", l.release, l.headline),
        evidence: vec![l.evidence.clone(), format!("Next step: {}", l.next_step)],
        links: vec![format!("/p/{pid}/releases?env={env}"), format!("/p/{pid}")],
        impact: 1.0,
        confidence: 0.9,
    }
}

/// An insight card (pure).
#[must_use]
pub fn from_card(pid: &str, c: &Insight) -> Finding {
    let score = if c.score.is_finite() {
        c.score.max(0.0)
    } else {
        0.0
    };
    Finding {
        kind: "insight".into(),
        subject: format!("{}:{}", c.kind, c.subject),
        title: c.title.clone(),
        evidence: vec![c.evidence.clone(), format!("Next step: {}", c.next_step)],
        links: vec![format!("/p/{pid}"), format!("/p/{pid}/replays")],
        impact: score / (score + 5.0),
        confidence: 0.6,
    }
}

/// An error group (pure).
#[must_use]
pub fn from_error(pid: &str, g: &ErrorGroup) -> Finding {
    let users = g.users.max(0) as f64;
    Finding {
        kind: "error".into(),
        subject: g.fingerprint.clone(),
        title: format!("Fix: {}", g.sample.chars().take(90).collect::<String>()),
        evidence: vec![format!(
            "{} users, {} sessions, {} times in 30 days; last seen {}",
            g.users, g.sessions, g.count, g.last_seen
        )],
        links: vec![format!("/p/{pid}/errors")],
        impact: ((1.0 + users).log10() / 3.0).min(1.0),
        confidence: 0.9,
    }
}

/// A KPI value as people read it: a share as a percentage.
fn shown(v: f64, share: bool) -> String {
    if share {
        format!("{:.1}%", v * 100.0)
    } else {
        format!("{v:.2}")
    }
}

/// A goal at risk or off track; none otherwise (pure).
#[must_use]
pub fn from_goal(pid: &str, r: &super::goals::GoalRow) -> Option<Finding> {
    let impact = match r.state {
        Status::OffTrack => 0.8,
        Status::AtRisk => 0.6,
        _ => return None,
    };
    Some(Finding {
        kind: "goal".into(),
        subject: r.goal.id.clone(),
        title: format!("The goal on {} is {}", r.label, r.status),
        evidence: vec![format!(
            "{} now, target {} by {} ({} days left)",
            r.current
                .map_or_else(|| "—".to_owned(), |v| shown(v, r.share)),
            shown(r.goal.target, r.share),
            r.goal.deadline,
            r.days_left
        )],
        links: vec![format!("/p/{pid}/goals")],
        impact,
        confidence: 0.7,
    })
}

/// A map health finding at the `error` level; none otherwise (pure).
#[must_use]
pub fn from_map(pid: &str, x: &crate::maphealth::Finding) -> Option<Finding> {
    if x.level != "error" {
        return None;
    }
    let affected = x.impact.max(0) as f64;
    Some(Finding {
        kind: "map".into(),
        subject: format!("{}:{}", x.kind, x.feature.as_deref().unwrap_or("")),
        title: format!("Feature map: {}", x.message),
        evidence: vec![x.evidence.clone()],
        links: vec![format!("/p/{pid}/features")],
        impact: ((1.0 + affected).log10() / 8.0).min(0.5),
        confidence: 0.8,
    })
}

/// The board's findings: the regressed latest release, then the cards
/// (the card of a release that leads as its verdict is left out) (pure).
#[must_use]
pub fn from_board(pid: &str, env: &str, b: &Board) -> Vec<Finding> {
    b.release_lead()
        .map(|l| from_lead(pid, env, &l))
        .into_iter()
        .chain(
            b.ranked_insights()
                .into_iter()
                .map(|(_, c)| from_card(pid, c)),
        )
        .collect()
}

pub(crate) fn load(meta: &Meta, project: &str) -> Vec<Tracked> {
    meta.project_setting(project, "actions")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save(meta: &Meta, project: &str, all: &[Tracked]) -> Result<(), Error> {
    let j = serde_json::to_string(all).map_err(|e| Error::Storage(e.to_string()))?;
    meta.set_project_setting(project, "actions", &j)
        .map_err(|e| Error::Storage(e.to_string()))
}

fn day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok()
}

fn hidden(t: &Tracked, today: NaiveDate) -> bool {
    t.dismissed
        .as_ref()
        .and_then(|d| day(&d.until))
        .is_some_and(|until| until > today)
}

/// The newest ticket of each fingerprint.
fn tickets_by_fp(all: &[Ticket]) -> BTreeMap<&str, &Ticket> {
    all.iter().map(|t| (t.fp.as_str(), t)).collect()
}

/// Where an action stands, from its record and its ticket (pure).
#[must_use]
pub fn state_of(t: Option<&Tracked>, ticket: Option<&Ticket>, today: NaiveDate) -> State {
    if t.is_some_and(|t| hidden(t, today)) {
        return State::Dismissed;
    }
    match ticket {
        Some(k) if k.outcome.is_some() => State::Learned,
        Some(k) if k.shipped_in.is_some() => State::Shipped,
        Some(_) => State::Approved,
        None if t.is_some_and(|t| t.approved.is_some()) => State::Approved,
        None if t.is_some_and(|t| t.proposed.is_some()) => State::Proposed,
        None => State::New,
    }
}

fn action(
    id: String,
    finding: Finding,
    current: bool,
    t: Option<&Tracked>,
    ticket: Option<&Ticket>,
    today: NaiveDate,
) -> Action {
    let state = state_of(t, ticket, today);
    let proposal = t.and_then(|t| t.proposed.as_ref());
    Action {
        rank: finding.rank(),
        id,
        state,
        current,
        proposed: proposal.map(|p| (p.day.clone(), p.note.clone())),
        draft: proposal
            .filter(|_| state == State::Proposed)
            .map(|p| p.draft.markdown.clone()),
        ticket: ticket.map(|k| TicketLink {
            url: k.issue.as_ref().map(|i| i.1.clone()),
            shipped_in: k
                .shipped_in
                .as_ref()
                .map(|s| s.split('|').next().unwrap_or("").to_owned()),
            outcome: k.outcome.clone(),
            method: k.method.clone(),
        }),
        dismissed: t
            .and_then(|t| t.dismissed.clone())
            .filter(|_| state == State::Dismissed),
        finding,
    }
}

/// Every action of a project, deduplicated by id and highest rank first,
/// the dismissed ones included (pure): the findings now (a finding seen
/// twice counts once, at its higher rank), the actions acted on that left
/// them, and the filed tickets still being followed. A finding that left
/// while `new` is not there; a learned ticket that never went through
/// this list is in the learning log only.
#[must_use]
pub fn assemble(
    project: &str,
    findings: Vec<Finding>,
    tracked: &[Tracked],
    tickets: &[Ticket],
    today: NaiveDate,
) -> Vec<Action> {
    let by_fp = tickets_by_fp(tickets);
    let by_id: BTreeMap<&str, &Tracked> = tracked.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut now: BTreeMap<String, Finding> = BTreeMap::new();
    for f in findings {
        let id = id_of(project, &f.kind, &f.subject);
        if now.get(&id).is_none_or(|seen| seen.rank() < f.rank()) {
            now.insert(id, f);
        }
    }
    let mut out: Vec<Action> = Vec::new();
    for (id, f) in now {
        let t = by_id.get(id.as_str()).copied();
        let k = by_fp.get(id.as_str()).copied();
        out.push(action(id, f, true, t, k, today));
    }
    for t in tracked {
        if out.iter().any(|a| a.id == t.id) {
            continue;
        }
        let k = by_fp.get(t.id.as_str()).copied();
        out.push(action(
            t.id.clone(),
            t.finding.clone(),
            false,
            Some(t),
            k,
            today,
        ));
    }
    for k in by_fp.values() {
        if k.outcome.is_some() || k.issue.is_none() || out.iter().any(|a| a.id == k.fp) {
            continue;
        }
        let f = Finding {
            kind: k.kind.clone(),
            subject: k.subject.clone(),
            title: k.title.clone(),
            evidence: Vec::new(),
            links: vec![format!("/p/{project}/tickets")],
            impact: 0.0,
            confidence: 0.0,
        };
        out.push(action(k.fp.clone(), f, false, None, Some(*k), today));
    }
    out.sort_by(|a, b| b.rank.total_cmp(&a.rank).then_with(|| a.id.cmp(&b.id)));
    out
}

/// The list: the dismissed actions left out, or only those in `state`
/// when it is given, then the first [`MAX_LISTED`] (pure).
#[must_use]
pub fn list(all: Vec<Action>, state: Option<State>, unavailable: Vec<String>) -> NextActions {
    let dismissed = all.iter().filter(|a| a.state == State::Dismissed).count();
    let mut shown: Vec<Action> = all
        .into_iter()
        .filter(|a| match state {
            Some(s) => a.state == s,
            None => a.state != State::Dismissed,
        })
        .collect();
    let more = shown.len().saturating_sub(MAX_LISTED);
    shown.truncate(MAX_LISTED);
    NextActions {
        actions: shown,
        more,
        dismissed,
        unavailable,
    }
}

/// Keep at most [`MAX_TRACKED`] records: the oldest of those `done`
/// (learned, or whose dismissal ended) go first, then the oldest of the
/// others (pure).
pub fn prune(all: &mut Vec<Tracked>, done: impl Fn(&Tracked) -> bool) {
    while all.len() > MAX_TRACKED {
        let oldest = |pick: &dyn Fn(&Tracked) -> bool| {
            all.iter()
                .enumerate()
                .filter(|(_, t)| pick(t))
                .min_by(|a, b| a.1.since.cmp(&b.1.since))
                .map(|(i, _)| i)
        };
        let Some(i) = oldest(&done).or_else(|| oldest(&|_: &Tracked| true)) else {
            return;
        };
        all.remove(i);
    }
}

fn filters(p: &Project) -> Filters {
    Filters {
        tz: p.timezone.clone(),
        ..Filters::default()
    }
}

/// The findings now, and the sources that could not be read.
async fn findings<H: Host>(host: &H, p: &Project) -> Result<(Vec<Finding>, Vec<String>), Error> {
    let f = filters(p);
    let map = host.feature_map(&p.id).await;
    let b = host
        .board(&p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    let mut out = from_board(&p.id, &f.environment, &b);
    let mut unavailable = Vec::new();
    let src = crate::source(map.as_ref().map(|(m, _)| m));
    let (errors, goals, health) = futures::join!(
        crate::error_groups(host.query(), &p.id, &src, &f),
        Box::pin(super::goals::evaluate(host, p, &p.timezone)),
        async {
            match &map {
                Some((m, _)) => Some(crate::map_health(host.query(), &p.id, m, &f).await),
                None => None,
            }
        }
    );
    match errors {
        Ok(mut g) => {
            g.sort_by(|a, b| b.users.cmp(&a.users).then(b.sessions.cmp(&a.sessions)));
            out.extend(
                g.iter()
                    .filter(|g| g.users > 0 || g.sessions > 0)
                    .take(ERROR_GROUPS)
                    .map(|g| from_error(&p.id, g)),
            );
        }
        Err(e) => unavailable.push(format!("errors: {e}")),
    }
    match goals {
        Ok(ev) => out.extend(ev.rows.iter().filter_map(|r| from_goal(&p.id, r))),
        Err(e) => unavailable.push(format!("goals: {e}")),
    }
    match health {
        Some(Ok(h)) => out.extend(h.findings.iter().filter_map(|x| from_map(&p.id, x))),
        Some(Err(e)) => unavailable.push(format!("map health: {e}")),
        None => {}
    }
    Ok((out, unavailable))
}

async fn project<H: Host>(host: &H, id: &str) -> Result<Project, Error> {
    host.project(id).await.ok_or(Error::NotFound)
}

fn today() -> NaiveDate {
    chrono::Utc::now().date_naive()
}

fn ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Every action of the project now, with the stored records.
async fn everything<H: Host>(
    host: &H,
    p: &Project,
) -> Result<(Vec<Action>, Vec<Tracked>, Vec<String>), Error> {
    let (found, unavailable) = findings(host, p).await?;
    let tracked = load(host.meta(), &p.id);
    let tickets = tickets::load(host.meta(), &p.id);
    let all = assemble(&p.id, found, &tracked, &tickets, today());
    Ok((all, tracked, unavailable))
}

/// The action `id`, or why there is none.
fn find(all: Vec<Action>, id: &str) -> Result<Action, Error> {
    all.into_iter().find(|a| a.id == id).ok_or_else(|| {
        Error::Invalid("No such action: it is not on the list now (see next_actions).".into())
    })
}

/// Store `t` in place of the record with its id, then prune.
fn store<H: Host>(host: &H, p: &str, mut all: Vec<Tracked>, t: Tracked) -> Result<(), Error> {
    match all.iter_mut().find(|x| x.id == t.id) {
        Some(x) => *x = t,
        None => all.push(t),
    }
    let tickets = tickets::load(host.meta(), p);
    let by_fp = tickets_by_fp(&tickets);
    let today = today();
    prune(&mut all, |t| {
        (t.dismissed.is_some() && !hidden(t, today))
            || by_fp
                .get(t.id.as_str())
                .is_some_and(|k| k.outcome.is_some())
    });
    save(host.meta(), p, &all)
}

/// The stored record of `a`, or a new one.
fn record(a: &Action, tracked: &[Tracked]) -> Tracked {
    tracked
        .iter()
        .find(|t| t.id == a.id)
        .cloned()
        .unwrap_or_else(|| Tracked {
            id: a.id.clone(),
            finding: a.finding.clone(),
            since: ymd(today()),
            proposed: None,
            approved: None,
            dismissed: None,
        })
}

fn bounded(s: &str) -> String {
    s.trim().chars().take(MAX_TEXT).collect()
}

/// The input of `actions.next`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextInput {
    /// Project id.
    pub project: String,
    /// Only the actions in this state (`approved`…); without it, every
    /// action but the dismissed ones.
    #[serde(default)]
    pub state: Option<String>,
}

/// `actions.next`: the ranked, deduplicated actions of a project, each
/// with its state. An unknown state is [`Error::Invalid`]; a board that
/// cannot be computed is [`Error::Unavailable`]; another source that
/// cannot be read is named in `unavailable`.
pub struct Next;

impl Command for Next {
    const NAME: &'static str = "actions.next";
    const ACCESS: Access = Access::Read;
    type Input = NextInput;
    type Output = NextActions;

    async fn run<H: Host>(host: &H, _: &Caller, input: NextInput) -> Result<NextActions, Error> {
        let state = match input
            .state
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(s) => Some(State::parse(s).ok_or_else(|| {
                Error::Invalid(
                    "state is new, proposed, approved, shipped, learned or dismissed.".into(),
                )
            })?),
            None => None,
        };
        let p = project(host, &input.project).await?;
        let (all, _, unavailable) = everything(host, &p).await?;
        Ok(list(all, state, unavailable))
    }
}

/// The input of `actions.propose`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposeInput {
    /// Project id.
    pub project: String,
    /// The action's id.
    pub id: String,
    /// What the agent plans to do.
    #[serde(default)]
    pub note: Option<String>,
    /// The app's public address, for the links of the drafted ticket.
    #[serde(default)]
    pub host: String,
}

/// `actions.propose`: an agent proposes to work on an action. Its ticket
/// is drafted and kept with the action, never posted. An action already
/// proposed or further is given back unchanged; a dismissed one, or one
/// not on the list, is [`Error::Invalid`].
pub struct Propose;

impl Command for Propose {
    const NAME: &'static str = "actions.propose";
    const ACCESS: Access = Access::Read;
    type Input = ProposeInput;
    type Output = Action;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProposeInput) -> Result<Action, Error> {
        let p = project(host, &input.project).await?;
        let (all, tracked, _) = everything(host, &p).await?;
        let a = find(all, &input.id)?;
        match a.state {
            State::New => {}
            State::Dismissed => {
                return Err(Error::Invalid(
                    "A person dismissed this action; it comes back when the dismissal ends.".into(),
                ));
            }
            _ => return Ok(a),
        }
        let draft = tickets::draft(
            host,
            &p,
            &a.finding.kind,
            &a.finding.subject,
            &input.host,
            None,
        )
        .await
        .map_err(Error::Invalid)?;
        let mut t = record(&a, &tracked);
        t.finding = a.finding.clone();
        t.proposed = Some(Proposal {
            day: ymd(today()),
            note: bounded(input.note.as_deref().unwrap_or("")),
            draft,
        });
        let out = action(
            a.id.clone(),
            a.finding.clone(),
            a.current,
            Some(&t),
            None,
            today(),
        );
        store(host, &p.id, tracked, t)?;
        Ok(out)
    }
}

/// The input of `actions.approve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveInput {
    /// Project id.
    pub project: String,
    /// The action's id.
    pub id: String,
    /// The app's public address, for the links of the ticket.
    #[serde(default)]
    pub host: String,
}

/// `actions.approve`: a person approves an action, and its ticket is
/// filed as *Create ticket* files it (with fresh numbers while the
/// finding is there, else as proposed). An action already approved or
/// further is given back unchanged, so the ticket is filed once. No
/// repository set or GitHub's refusal is [`Error::Unavailable`], and the
/// action stays as it was.
pub struct Approve;

impl Command for Approve {
    const NAME: &'static str = "actions.approve";
    const ACCESS: Access = Access::Write;
    type Input = ApproveInput;
    type Output = Action;

    async fn run<H: Host>(host: &H, _: &Caller, input: ApproveInput) -> Result<Action, Error> {
        let p = project(host, &input.project).await?;
        let (all, tracked, _) = everything(host, &p).await?;
        let a = find(all, &input.id)?;
        if !matches!(a.state, State::New | State::Proposed | State::Dismissed) {
            return Ok(a);
        }
        let mut t = record(&a, &tracked);
        // A ticket filed already (a dismissed action): approving files
        // nothing more.
        let k = tickets::load(host.meta(), &p.id);
        if let Some(filed) = tickets_by_fp(&k).get(a.id.as_str()).copied() {
            t.approved = Some(ymd(today()));
            t.dismissed = None;
            let out = action(
                a.id.clone(),
                a.finding.clone(),
                a.current,
                Some(&t),
                Some(filed),
                today(),
            );
            store(host, &p.id, tracked, t)?;
            return Ok(out);
        }
        let stored = t.proposed.as_ref().map(|x| x.draft.clone());
        let flag = stored.as_ref().and_then(|d| d.ticket.flag.clone());
        let fresh = tickets::draft(
            host,
            &p,
            &a.finding.kind,
            &a.finding.subject,
            &input.host,
            flag.as_deref(),
        )
        .await;
        let d = match (fresh, stored) {
            (Ok(d), _) | (Err(_), Some(d)) => d,
            (Err(e), None) => return Err(Error::Invalid(e)),
        };
        let filed = tickets::file(host, &p.id, d)
            .await
            .map_err(Error::Unavailable)?;
        t.approved = Some(ymd(today()));
        t.dismissed = None;
        let out = action(
            a.id.clone(),
            a.finding.clone(),
            a.current,
            Some(&t),
            Some(&filed),
            today(),
        );
        store(host, &p.id, tracked, t)?;
        Ok(out)
    }
}

/// The input of `actions.dismiss`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DismissInput {
    /// Project id.
    pub project: String,
    /// The action's id.
    pub id: String,
    /// Why it is dismissed (required).
    pub reason: String,
}

/// `actions.dismiss`: a person hides an action for [`DISMISS_DAYS`] days.
/// No reason, or no such action, is [`Error::Invalid`].
pub struct Dismiss;

impl Command for Dismiss {
    const NAME: &'static str = "actions.dismiss";
    const ACCESS: Access = Access::Write;
    type Input = DismissInput;
    type Output = Action;

    async fn run<H: Host>(host: &H, _: &Caller, input: DismissInput) -> Result<Action, Error> {
        let reason = bounded(&input.reason);
        if reason.is_empty() {
            return Err(Error::Invalid("Say why it is dismissed.".into()));
        }
        let p = project(host, &input.project).await?;
        let (all, tracked, _) = everything(host, &p).await?;
        let a = find(all, &input.id)?;
        let mut t = record(&a, &tracked);
        let today = today();
        t.dismissed = Some(Dismissal {
            until: ymd(today + chrono::Duration::days(DISMISS_DAYS)),
            reason,
        });
        let k = tickets::load(host.meta(), &p.id);
        let ticket = tickets_by_fp(&k).get(a.id.as_str()).copied().cloned();
        let out = action(
            a.id.clone(),
            a.finding.clone(),
            a.current,
            Some(&t),
            ticket.as_ref(),
            today,
        );
        store(host, &p.id, tracked, t)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests;

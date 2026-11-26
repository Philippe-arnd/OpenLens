//! Goals and objectives as commands (#235, #272), moved out of the server
//! as is (#494): listing them with where they stand, setting and deleting
//! them, and the hourly pass. Stored per project as `project:<id>:goals`
//! and `project:<id>:objectives` (JSON); admins change them.

use openlens_meta::{Meta, Project};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

use super::{Access, Caller, Change, Command, Error, Host};
use crate::Filters;
use crate::goals::{self, Goal, Move, Objective, Status};

fn load(meta: &Meta, project: &str) -> Vec<Goal> {
    meta.project_setting(project, "goals")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save(meta: &Meta, project: &str, g: &[Goal]) -> Result<(), Error> {
    let j = serde_json::to_string(g).map_err(|e| Error::Storage(e.to_string()))?;
    meta.set_project_setting(project, "goals", &j)
        .map_err(|e| Error::Storage(e.to_string()))
}

fn load_objectives(meta: &Meta, project: &str) -> Vec<Objective> {
    meta.project_setting(project, "objectives")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save_objectives(meta: &Meta, project: &str, o: &[Objective]) -> Result<(), Error> {
    let j = serde_json::to_string(o).map_err(|e| Error::Storage(e.to_string()))?;
    meta.set_project_setting(project, "objectives", &j)
        .map_err(|e| Error::Storage(e.to_string()))
}

/// A 12-character id from `s` (the first bytes of its SHA-256, in hex).
fn short_hash(s: &str) -> String {
    use sha2::Digest;
    let h = hex::encode(sha2::Sha256::digest(s.as_bytes()));
    h.get(..12).unwrap_or(&h).to_owned()
}

async fn project<H: Host>(host: &H, id: &str) -> Result<Project, Error> {
    host.project(id).await.ok_or(Error::NotFound)
}

/// A goal with where it stands (#235).
pub struct GoalRow {
    /// The stored goal.
    pub goal: Goal,
    /// KPI label.
    pub label: String,
    /// A share (shown as %).
    pub share: bool,
    /// Value now.
    pub current: Option<f64>,
    /// Projected value at the deadline.
    pub projected: Option<f64>,
    /// Per release (oldest first).
    pub series: Vec<(String, f64)>,
    /// Status label.
    pub status: String,
    /// Reached or on track.
    pub good: bool,
    /// Off track.
    pub bad: bool,
    /// Days to the deadline.
    pub days_left: i64,
    /// The status itself.
    pub state: Status,
    /// Its guardrails now (#272).
    pub guards: Vec<GuardRow>,
    /// Releases that moved the KPI most since the goal was set.
    pub moves: Vec<Move>,
    /// Changes in the period (flags, surveys, map, notes, alerts).
    pub marks: Vec<Change>,
}

/// A guardrail as shown.
pub struct GuardRow {
    /// KPI label.
    pub label: String,
    /// A share.
    pub share: bool,
    /// Value now.
    pub current: Option<f64>,
    /// The limit.
    pub limit: f64,
    /// Worse than the limit.
    pub breached: bool,
}

/// An objective as shown (#272).
pub struct ObjectiveRow {
    /// As stored.
    pub objective: Objective,
    /// Its status (the worst key result).
    pub status: String,
    /// Reached or on track.
    pub good: bool,
    /// At risk or off track.
    pub bad: bool,
    /// Key results: (label, now, target, share, status).
    pub results: Vec<(String, Option<f64>, f64, bool, String)>,
}

/// A KPI a goal can be set on, with its suggested target.
pub struct GoalChoice {
    /// Key.
    pub kpi: String,
    /// Label.
    pub label: String,
    /// Higher is better.
    pub up: bool,
    /// A share.
    pub share: bool,
    /// Value now.
    pub current: f64,
    /// Suggested target.
    pub suggested: f64,
}

/// Goals, objectives and suggestions, evaluated now. Serialised as the
/// REST API returns them.
pub struct Evaluated {
    /// Every goal.
    pub rows: Vec<GoalRow>,
    /// A suggestion per KPI with a value.
    pub choices: Vec<GoalChoice>,
    /// Every objective.
    pub objectives: Vec<ObjectiveRow>,
    /// First-seen days of the releases (for deadlines in releases).
    release_days: Vec<String>,
}

impl Serialize for Evaluated {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        json!({
            "goals": self.rows.iter().map(|r| json!({
                "goal": r.goal, "label": r.label, "current": r.current,
                "projected": r.projected, "status": r.status, "days_left": r.days_left,
                "guardrails": r.guards.iter().map(|g| json!({
                    "label": g.label, "current": g.current, "limit": g.limit, "breached": g.breached,
                })).collect::<Vec<_>>(),
                "moved_by": { "releases": r.moves, "changes": r.marks.iter().map(|m| json!({
                    "day": m.day, "kind": m.kind, "text": m.text,
                })).collect::<Vec<_>>() },
            })).collect::<Vec<_>>(),
            "objectives": self.objectives.iter().map(|o| json!({
                "objective": o.objective, "status": o.status,
            })).collect::<Vec<_>>(),
            "suggestions": self.choices.iter().map(|c| json!({
                "kpi": c.kpi, "label": c.label, "current": c.current, "suggested_target": c.suggested,
                "higher_is_better": c.up, "share": c.share,
            })).collect::<Vec<_>>(),
        })
        .serialize(s)
    }
}

/// Every goal with where it stands, its guardrails and what moved it, the
/// objectives, and a suggestion per KPI, with days counted in `tz`.
///
/// # Errors
/// [`Error::Unavailable`] when the board cannot be computed.
pub async fn evaluate<H: Host>(host: &H, p: &Project, tz: &str) -> Result<Evaluated, Error> {
    let map = host.feature_map(&p.id).await;
    let f = Filters {
        tz: tz.to_owned(),
        ..Filters::default()
    };
    let b = host
        .board(&p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    let w = goals::weekly(host.query(), &p.id, map.as_ref().map(|(m, _)| m), &f)
        .await
        .unwrap_or_default();
    let today = chrono::Utc::now().date_naive();
    let releases: Vec<(String, String)> = b
        .releases
        .iter()
        .map(|r| (r.release.clone(), r.first_seen.clone()))
        .collect();
    let marks: Vec<Change> = host
        .changes(&p.id, &releases, 365)
        .into_iter()
        .filter(|m| m.kind != "release")
        .collect();
    let rows: Vec<GoalRow> = load(host.meta(), &p.id)
        .into_iter()
        .map(|g| {
            let (current, series) = goals::kpi_values_with(&b, &w, &g.kpi);
            let (st, projected) = goals::status(&g, current, &series, today);
            let (label, _, share) = goals::kpi_info(&g.kpi);
            let guards = g
                .guardrails
                .iter()
                .map(|gr| {
                    let (cur, _) = goals::kpi_values_with(&b, &w, &gr.kpi);
                    let (label, _, share) = goals::kpi_info(&gr.kpi);
                    GuardRow {
                        label,
                        share,
                        current: cur,
                        limit: gr.limit,
                        breached: cur.is_some_and(|c| gr.breached(c)),
                    }
                })
                .collect();
            let moves = goals::movers(&b, &g);
            let during: Vec<Change> = marks
                .iter()
                .filter(|m| m.day >= g.created)
                .take(5)
                .cloned()
                .collect();
            GoalRow {
                status: st.label().to_owned(),
                state: st,
                good: matches!(st, Status::Reached | Status::OnTrack),
                bad: matches!(st, Status::OffTrack),
                label,
                share,
                current,
                projected,
                series,
                days_left: chrono::NaiveDate::parse_from_str(&g.deadline, "%Y-%m-%d")
                    .map_or(0, |d| (d - today).num_days()),
                guards,
                moves,
                marks: during,
                goal: g,
            }
        })
        .collect();
    let mut keys: Vec<String> = goals::KPIS.iter().map(|k| k.0.to_owned()).collect();
    if let Some((m, _)) = &map {
        keys.extend(
            m.features
                .iter()
                .filter(|x| x.success.is_some())
                .map(|x| format!("success:{}", x.key)),
        );
    }
    let choices = keys
        .into_iter()
        .filter_map(|k| {
            let (current, series) = goals::kpi_values_with(&b, &w, &k);
            let current = current?;
            let (label, up, share) = goals::kpi_info(&k);
            let (_, target) = goals::suggest(current, &series, up, share);
            Some(GoalChoice {
                kpi: k,
                label,
                up,
                share,
                current,
                suggested: target,
            })
        })
        .collect();
    let objectives = load_objectives(host.meta(), &p.id)
        .into_iter()
        .map(|o| {
            let krs: Vec<&GoalRow> = rows
                .iter()
                .filter(|r| o.goals.contains(&r.goal.id))
                .collect();
            let st = goals::objective_status(&krs.iter().map(|r| r.state).collect::<Vec<_>>());
            ObjectiveRow {
                status: st.label().to_owned(),
                good: matches!(st, Status::Reached | Status::OnTrack),
                bad: matches!(st, Status::OffTrack | Status::AtRisk),
                results: krs
                    .iter()
                    .map(|r| {
                        (
                            r.label.clone(),
                            r.current,
                            r.goal.target,
                            r.share,
                            r.status.clone(),
                        )
                    })
                    .collect(),
                objective: o,
            }
        })
        .collect();
    Ok(Evaluated {
        rows,
        choices,
        objectives,
        release_days: b.releases.iter().map(|r| r.first_seen.clone()).collect(),
    })
}

/// What a new goal is made of.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewGoal {
    /// KPI key (`bounce_rate`, `success:<feature>`).
    pub kpi: String,
    /// In KPI units: a share as 0–1 (as `goals.list` returns them).
    pub target: f64,
    /// Deadline (YYYY-MM-DD), or ...
    #[serde(default)]
    pub deadline: Option<String>,
    /// ... a number of releases (#272).
    #[serde(default)]
    pub releases: Option<u32>,
    /// Who owns it.
    #[serde(default)]
    pub owner: String,
    /// Up to three guardrails.
    #[serde(default)]
    pub guardrails: Vec<NewGuardrail>,
}

/// A guardrail of a new goal: `{ "kpi": "bounce_rate", "limit": 0.4 }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewGuardrail {
    /// KPI key.
    pub kpi: String,
    /// In KPI units; none: the suggested 5% worse than now.
    #[serde(default)]
    pub limit: Option<f64>,
}

/// Validate and build a goal against what the board shows now.
fn build_goal(ev: &Evaluated, existing: usize, i: NewGoal) -> Result<Goal, String> {
    let Some(c) = ev.choices.iter().find(|c| c.kpi == i.kpi) else {
        return Err("This KPI has no value yet.".into());
    };
    if !i.target.is_finite() || i.target < 0.0 || (c.share && i.target > 1.0) {
        return Err("The target is out of range.".into());
    }
    let today = chrono::Utc::now().date_naive();
    let deadline = match (
        i.releases.filter(|n| *n > 0),
        i.deadline.as_deref().filter(|d| !d.is_empty()),
    ) {
        (Some(n), _) => goals::deadline_in_releases(&ev.release_days, n.min(50), today)
            .ok_or("A deadline in releases needs at least two releases seen.")?,
        (None, Some(d)) => chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map_err(|_| "The deadline is a date (YYYY-MM-DD).")?,
        (None, None) => return Err("Give a deadline, or a number of releases.".into()),
    };
    if deadline <= today || deadline > today + chrono::Duration::days(3 * 366) {
        return Err("The deadline is a date in the next three years.".into());
    }
    if existing >= 50 {
        return Err("At most 50 goals per project.".into());
    }
    let mut guardrails = Vec::new();
    for NewGuardrail { kpi: k, limit } in i.guardrails.into_iter().take(3) {
        if k.is_empty() || k == i.kpi {
            continue;
        }
        let Some(gc) = ev.choices.iter().find(|x| x.kpi == k) else {
            return Err(format!("The guardrail `{k}` has no value yet."));
        };
        let limit =
            limit.unwrap_or_else(|| goals::Guardrail::suggested(gc.current, gc.up, gc.share));
        if !limit.is_finite() || limit < 0.0 || (gc.share && limit > 1.0) {
            return Err(format!("The limit of `{k}` is out of range."));
        }
        guardrails.push(goals::Guardrail {
            kpi: k,
            limit,
            up: gc.up,
        });
    }
    let deadline = deadline.format("%Y-%m-%d").to_string();
    Ok(Goal {
        id: short_hash(&format!(
            "{}{}{}",
            i.kpi,
            deadline,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )),
        kpi: i.kpi,
        target: i.target,
        up: c.up,
        deadline,
        owner: i.owner.trim().chars().take(80).collect(),
        baseline: c.current,
        created: today.format("%Y-%m-%d").to_string(),
        guardrails,
        releases: i.releases.filter(|n| *n > 0),
    })
}

/// A project, for the commands that only name it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInput {
    /// Project id.
    pub project: String,
}

/// `goals.list`: every goal with its current value, projection and status,
/// the objectives, and the suggested targets.
pub struct List;

impl Command for List {
    const NAME: &'static str = "goals.list";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Evaluated;

    async fn run<H: Host>(host: &H, caller: &Caller, input: ProjectInput) -> Result<Evaluated, Error> {
        let p = project(host, &input.project).await?;
        let tz = caller.tz.clone().unwrap_or_else(|| p.timezone.clone());
        evaluate(host, &p, &tz).await
    }
}

/// The input of `goals.set`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetGoalInput {
    /// Project id.
    pub project: String,
    /// The goal.
    #[serde(flatten)]
    pub goal: NewGoal,
}

/// `goals.set`: set a goal, checked against what the board shows now.
pub struct Set;

impl Command for Set {
    const NAME: &'static str = "goals.set";
    const ACCESS: Access = Access::Write;
    type Input = SetGoalInput;
    type Output = Goal;

    async fn run<H: Host>(host: &H, caller: &Caller, input: SetGoalInput) -> Result<Goal, Error> {
        let p = project(host, &input.project).await?;
        let tz = caller.tz.clone().unwrap_or_else(|| p.timezone.clone());
        let ev = Box::pin(evaluate(host, &p, &tz)).await?;
        set_evaluated(host.meta(), &p.id, &ev, input.goal)
    }
}

/// `goals.set` once the caller has evaluated the goals (the page, which
/// reads the KPIs' units from them first).
///
/// # Errors
/// [`Error::Invalid`] when the goal is refused, [`Error::Storage`] when it
/// cannot be stored.
pub fn set_evaluated(
    meta: &Meta,
    project: &str,
    ev: &Evaluated,
    goal: NewGoal,
) -> Result<Goal, Error> {
    let mut all = load(meta, project);
    let goal = build_goal(ev, all.len(), goal).map_err(Error::Invalid)?;
    all.push(goal.clone());
    save(meta, project, &all)?;
    Ok(goal)
}

/// The input of `goals.delete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteGoalInput {
    /// Project id.
    pub project: String,
    /// Goal id.
    pub goal: String,
}

/// `goals.delete`: remove a goal (nothing happens if it is not there).
pub struct Delete;

impl Command for Delete {
    const NAME: &'static str = "goals.delete";
    const ACCESS: Access = Access::Write;
    type Input = DeleteGoalInput;
    type Output = ();

    async fn run<H: Host>(host: &H, _: &Caller, input: DeleteGoalInput) -> Result<(), Error> {
        let mut all = load(host.meta(), &input.project);
        all.retain(|g| g.id != input.goal);
        save(host.meta(), &input.project, &all)
    }
}

/// The input of `goals.set_objective`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetObjectiveInput {
    /// Project id.
    pub project: String,
    /// The objective, in a sentence.
    pub title: String,
    /// One to three existing goal ids, its key results (empty ones are
    /// ignored).
    pub goals: Vec<String>,
    /// Last day of the period (YYYY-MM-DD), within a year.
    pub end: String,
}

/// `goals.set_objective` (#272): an objective with 1–3 goals as key
/// results, from today to its end.
pub struct SetObjective;

impl Command for SetObjective {
    const NAME: &'static str = "goals.set_objective";
    const ACCESS: Access = Access::Write;
    type Input = SetObjectiveInput;
    type Output = Objective;

    async fn run<H: Host>(host: &H, _: &Caller, input: SetObjectiveInput) -> Result<Objective, Error> {
        let bad = |m: &str| Error::Invalid(m.to_owned());
        let p = project(host, &input.project).await?;
        let title: String = input.title.trim().chars().take(140).collect();
        if title.is_empty() {
            return Err(bad("Say what the objective is, in a sentence."));
        }
        let known = load(host.meta(), &p.id);
        let mut ids: Vec<String> = input.goals.into_iter().filter(|g| !g.is_empty()).collect();
        ids.dedup();
        if ids.is_empty() || ids.len() > 3 || ids.iter().any(|g| !known.iter().any(|k| &k.id == g))
        {
            return Err(bad("Pick one to three existing goals as key results."));
        }
        let today = chrono::Utc::now().date_naive();
        match chrono::NaiveDate::parse_from_str(&input.end, "%Y-%m-%d") {
            Ok(d) if d > today && d <= today + chrono::Duration::days(366) => {}
            _ => return Err(bad("The period ends on a date within a year.")),
        }
        let mut all = load_objectives(host.meta(), &p.id);
        if all.iter().filter(|o| o.archived.is_none()).count() >= 10 {
            return Err(bad("At most 10 open objectives per project."));
        }
        let objective = Objective {
            id: short_hash(&format!(
                "{title}{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            )),
            title,
            goals: ids,
            start: today.format("%Y-%m-%d").to_string(),
            end: input.end,
            archived: None,
        };
        all.push(objective.clone());
        save_objectives(host.meta(), &p.id, &all)?;
        Ok(objective)
    }
}

/// The input of `goals.delete_objective`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteObjectiveInput {
    /// Project id.
    pub project: String,
    /// Objective id.
    pub objective: String,
}

/// `goals.delete_objective`: remove an objective (nothing happens if it
/// is not there).
pub struct DeleteObjective;

impl Command for DeleteObjective {
    const NAME: &'static str = "goals.delete_objective";
    const ACCESS: Access = Access::Write;
    type Input = DeleteObjectiveInput;
    type Output = ();

    async fn run<H: Host>(host: &H, _: &Caller, input: DeleteObjectiveInput) -> Result<(), Error> {
        let mut all = load_objectives(host.meta(), &input.project);
        all.retain(|o| o.id != input.objective);
        save_objectives(host.meta(), &input.project, &all)
    }
}

/// A guardrail as the hourly pass checks it (#272).
pub struct GuardCheck {
    /// Incident rule: `guardrail:<goal>:<kpi>`.
    pub rule: String,
    /// Worse than its limit now.
    pub breached: bool,
    /// What the incident says.
    pub text: String,
}

/// Every guardrail with a value now, for incidents that open when it is
/// breached and close when it recovers.
#[must_use]
pub fn guard_checks(ev: &Evaluated) -> Vec<GuardCheck> {
    let mut out = Vec::new();
    for r in &ev.rows {
        for (gr, row) in r.goal.guardrails.iter().zip(&r.guards) {
            let Some(cur) = row.current else { continue };
            out.push(GuardCheck {
                rule: format!("guardrail:{}:{}", r.goal.id, gr.kpi),
                breached: row.breached,
                text: format!(
                    "While pursuing \"{}\", {} is {} (limit {})",
                    r.label,
                    row.label,
                    fmt(cur, row.share),
                    fmt(gr.limit, row.share)
                ),
            });
        }
    }
    out
}

/// The rest of the hourly pass (#272): objectives past their period are
/// archived with the final values, and a snapshot feeds the board banner
/// and the digest.
pub fn close_periods(meta: &Meta, project: &str, ev: &Evaluated) {
    let today = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let mut objectives = load_objectives(meta, project);
    let mut changed = false;
    for o in objectives
        .iter_mut()
        .filter(|o| o.archived.is_none() && o.end < today)
    {
        let results: Vec<(String, String, Option<f64>, f64, bool)> = ev
            .rows
            .iter()
            .filter(|r| o.goals.contains(&r.goal.id))
            .map(|r| {
                let reached = r.current.is_some_and(|c| {
                    if r.goal.up {
                        c >= r.goal.target
                    } else {
                        c <= r.goal.target
                    }
                });
                (
                    r.goal.id.clone(),
                    r.label.clone(),
                    r.current,
                    r.goal.target,
                    reached,
                )
            })
            .collect();
        o.archived = Some(goals::Archived {
            at: today.clone(),
            achieved: !results.is_empty() && results.iter().all(|x| x.4),
            results,
        });
        changed = true;
    }
    if changed {
        let _ = save_objectives(meta, project, &objectives);
    }
    let at_risk: Vec<String> = ev
        .objectives
        .iter()
        .filter(|o| o.objective.archived.is_none() && o.objective.end >= today && o.bad)
        .map(|o| format!("{} ({})", o.objective.title, o.status))
        .collect();
    let snapshot: Vec<(String, String)> = ev
        .rows
        .iter()
        .map(|r| {
            (
                format!(
                    "{}: {} now, target {} by {}",
                    r.label,
                    r.current.map_or_else(|| "—".into(), |c| fmt(c, r.share)),
                    fmt(r.goal.target, r.share),
                    r.goal.deadline
                ),
                r.status.clone(),
            )
        })
        .collect();
    if let Ok(j) = serde_json::to_string(&json!({ "at_risk": at_risk, "goals": snapshot })) {
        let _ = meta.set_project_setting(project, "goals_snapshot", &j);
    }
}

/// The last hourly snapshot: (objectives at risk, goal lines with status).
#[must_use]
pub fn snapshot(meta: &Meta, project: &str) -> (Vec<String>, Vec<(String, String)>) {
    let v: serde_json::Value = meta
        .project_setting(project, "goals_snapshot")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    (
        serde_json::from_value(v["at_risk"].clone()).unwrap_or_default(),
        serde_json::from_value(v["goals"].clone()).unwrap_or_default(),
    )
}

fn fmt(v: f64, share: bool) -> String {
    if share {
        format!("{:.1}%", v * 100.0)
    } else if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

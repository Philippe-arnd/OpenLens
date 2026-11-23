//! Goals on KPIs (#235): the Goals page, setting a goal from a suggested
//! target, deleting one, and the API for agents. Stored per project as
//! `project:<id>:goals` (JSON); admins change them.

use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::goals::{self, Goal, Objective, Status};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

fn load(meta: &openlens_meta::Meta, project: &str) -> Vec<Goal> {
    meta.project_setting(project, "goals")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save(meta: &openlens_meta::Meta, project: &str, g: &[Goal]) -> Result<(), String> {
    let j = serde_json::to_string(g).map_err(|e| e.to_string())?;
    meta.set_project_setting(project, "goals", &j)
        .map_err(|e| e.to_string())
}

async fn project(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

fn load_objectives(meta: &openlens_meta::Meta, project: &str) -> Vec<Objective> {
    meta.project_setting(project, "objectives")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save_objectives(
    meta: &openlens_meta::Meta,
    project: &str,
    o: &[Objective],
) -> Result<(), String> {
    let j = serde_json::to_string(o).map_err(|e| e.to_string())?;
    meta.set_project_setting(project, "objectives", &j)
        .map_err(|e| e.to_string())
}

/// Goals, objectives and suggestions, evaluated now.
pub(crate) struct Evaluated {
    rows: Vec<openlens_ui::GoalRow>,
    choices: Vec<openlens_ui::GoalChoice>,
    objectives: Vec<openlens_ui::ObjectiveRow>,
    /// First-seen days of the releases (for deadlines in releases).
    release_days: Vec<String>,
}

/// Every goal with where it stands, its guardrails and what moved it, the
/// objectives, and a suggestion per KPI.
async fn evaluate(
    state: &AppState,
    p: &openlens_meta::Project,
    tz: &str,
) -> Result<Evaluated, String> {
    let map = crate::load_map(state, &p.id).await;
    let f = openlens_engine::Filters {
        tz: tz.to_owned(),
        ..openlens_engine::Filters::default()
    };
    let b = crate::boards::board(state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
        .map_err(|e| e.to_string())?;
    let w = goals::weekly(&state.query, &p.id, map.as_ref().map(|(m, _)| m), &f)
        .await
        .unwrap_or_default();
    let today = chrono::Utc::now().date_naive();
    let releases: Vec<(String, String)> = b
        .releases
        .iter()
        .map(|r| (r.release.clone(), r.first_seen.clone()))
        .collect();
    let marks: Vec<openlens_ui::Mark> = crate::timeline::collect(state, &p.id, &releases, 365)
        .into_iter()
        .filter(|m| m.kind != "release")
        .collect();
    let rows: Vec<openlens_ui::GoalRow> = load(&state.meta, &p.id)
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
                    openlens_ui::GuardRow {
                        label,
                        share,
                        current: cur,
                        limit: gr.limit,
                        breached: cur.is_some_and(|c| gr.breached(c)),
                    }
                })
                .collect();
            let moves = goals::movers(&b, &g);
            let during: Vec<openlens_ui::Mark> = marks
                .iter()
                .filter(|m| m.day >= g.created)
                .take(5)
                .cloned()
                .collect();
            openlens_ui::GoalRow {
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
            Some(openlens_ui::GoalChoice {
                kpi: k,
                label,
                up,
                share,
                current,
                suggested: target,
            })
        })
        .collect();
    let objectives = load_objectives(&state.meta, &p.id)
        .into_iter()
        .map(|o| {
            let krs: Vec<&openlens_ui::GoalRow> = rows
                .iter()
                .filter(|r| o.goals.contains(&r.goal.id))
                .collect();
            let st = goals::objective_status(&krs.iter().map(|r| r.state).collect::<Vec<_>>());
            openlens_ui::ObjectiveRow {
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

/// What a new goal is made of (form or API).
struct GoalInput {
    kpi: String,
    /// In KPI units (a share as 0–1).
    target: f64,
    deadline: Option<String>,
    releases: Option<u32>,
    owner: String,
    /// (KPI, limit in KPI units; None: the suggested 5% worse).
    guards: Vec<(String, Option<f64>)>,
}

/// Validate and build a goal against what the board shows now.
fn build_goal(ev: &Evaluated, existing: usize, i: GoalInput) -> Result<Goal, String> {
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
    for (k, limit) in i.guards.into_iter().take(3) {
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
        id: crate::short_hash(&format!(
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

/// Hourly (#272): breached guardrails become incidents (and close when
/// they recover), objectives past their period are archived with the
/// final values, and a snapshot feeds the board banner and the digest.
pub(crate) async fn hourly(state: &AppState, p: &openlens_meta::Project, alert: bool) {
    let Ok(ev) = Box::pin(evaluate(state, p, &p.timezone)).await else {
        return;
    };
    if alert {
        for r in &ev.rows {
            for (gr, row) in r.goal.guardrails.iter().zip(&r.guards) {
                let Some(cur) = row.current else { continue };
                let rule = format!("guardrail:{}:{}", r.goal.id, gr.kpi);
                Box::pin(crate::alerts::track(
                    state,
                    &p.id,
                    &p.name,
                    &rule,
                    row.breached,
                    &format!(
                        "While pursuing \"{}\", {} is {} (limit {})",
                        r.label,
                        row.label,
                        fmt(cur, row.share),
                        fmt(gr.limit, row.share)
                    ),
                    0,
                ))
                .await;
            }
        }
    }
    let today = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let mut objectives = load_objectives(&state.meta, &p.id);
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
        let _ = save_objectives(&state.meta, &p.id, &objectives);
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
        let _ = state.meta.set_project_setting(&p.id, "goals_snapshot", &j);
    }
}

/// The last hourly snapshot: (objectives at risk, goal lines with status).
pub(crate) fn snapshot(state: &AppState, project: &str) -> (Vec<String>, Vec<(String, String)>) {
    let v: serde_json::Value = state
        .meta
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

/// `GET /p/{id}/goals`.
pub(crate) async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(v) = auth::viewer(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };
    let Some(p) = project(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let tz = auth::tz_for(&state, &headers, &p.timezone);
    match evaluate(&state, &p, &tz).await {
        Ok(ev) => {
            let info = openlens_ui::ProjectInfo {
                id: &p.id,
                name: &p.name,
                key: &p.api_key,
            };
            let default_deadline = (chrono::Utc::now() + chrono::Duration::days(90))
                .format("%Y-%m-%d")
                .to_string();
            html_page(
                StatusCode::OK,
                openlens_ui::goals(
                    &info,
                    &ev.rows,
                    &ev.choices,
                    &ev.objectives,
                    v.admin,
                    &default_deadline,
                ),
            )
        }
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e),
        ),
    }
}

#[derive(Deserialize)]
pub(crate) struct NewGoal {
    kpi: String,
    /// In display units: a percentage for shares.
    target: f64,
    #[serde(default)]
    deadline: String,
    /// Or a number of releases (#272).
    #[serde(default)]
    releases: String,
    #[serde(default)]
    owner: String,
    /// Guardrails (#272): KPI keys, and limits in display units (empty:
    /// 5% worse than now).
    #[serde(default)]
    guard1: String,
    #[serde(default)]
    guard1_limit: String,
    #[serde(default)]
    guard2: String,
    #[serde(default)]
    guard2_limit: String,
}

/// `POST /p/{id}/goals` (admins).
pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<NewGoal>,
) -> Response {
    let oops = |m: &str| {
        html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't set that goal", m),
        )
    };
    if !crate::flags::same_origin(&headers) {
        return oops("Cross-site form.");
    }
    let Some(p) = project(&state, &id).await else {
        return oops("No such project.");
    };
    let tz = auth::tz_for(&state, &headers, &p.timezone);
    let Ok(ev) = evaluate(&state, &p, &tz).await else {
        return oops("The board is unavailable.");
    };
    let share_of = |k: &str| {
        ev.choices
            .iter()
            .find(|c| c.kpi == k)
            .is_some_and(|c| c.share)
    };
    let raw = |k: &str, v: f64| if share_of(k) { v / 100.0 } else { v };
    let guard = |k: &str, l: &str| {
        (
            k.to_owned(),
            l.trim().parse::<f64>().ok().map(|v| raw(k, v)),
        )
    };
    let mut all = load(&state.meta, &p.id);
    let input = GoalInput {
        target: raw(&f.kpi, f.target),
        kpi: f.kpi.clone(),
        deadline: Some(f.deadline.clone()),
        releases: f.releases.trim().parse().ok(),
        owner: f.owner.clone(),
        guards: vec![
            guard(&f.guard1, &f.guard1_limit),
            guard(&f.guard2, &f.guard2_limit),
        ],
    };
    let g = match build_goal(&ev, all.len(), input) {
        Ok(g) => g,
        Err(e) => return oops(&e),
    };
    all.push(g);
    match save(&state.meta, &p.id, &all) {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "goal.set", "");
            Redirect::to(&format!("/p/{}/goals", p.id)).into_response()
        }
        Err(e) => oops(&e),
    }
}

#[derive(Deserialize)]
pub(crate) struct NewObjective {
    title: String,
    #[serde(default)]
    goal1: String,
    #[serde(default)]
    goal2: String,
    #[serde(default)]
    goal3: String,
    end: String,
}

/// `POST /p/{id}/objectives` (admins, #272): an objective with 1–3 goals
/// as key results, from today to `end`.
pub(crate) async fn create_objective(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<NewObjective>,
) -> Response {
    let oops = |m: &str| {
        html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't set that objective", m),
        )
    };
    if !crate::flags::same_origin(&headers) {
        return oops("Cross-site form.");
    }
    let Some(p) = project(&state, &id).await else {
        return oops("No such project.");
    };
    let title: String = f.title.trim().chars().take(140).collect();
    if title.is_empty() {
        return oops("Say what the objective is, in a sentence.");
    }
    let known = load(&state.meta, &p.id);
    let mut ids: Vec<String> = [f.goal1, f.goal2, f.goal3]
        .into_iter()
        .filter(|g| !g.is_empty())
        .collect();
    ids.dedup();
    if ids.is_empty() || ids.iter().any(|g| !known.iter().any(|k| &k.id == g)) {
        return oops("Pick one to three existing goals as key results.");
    }
    let today = chrono::Utc::now().date_naive();
    match chrono::NaiveDate::parse_from_str(&f.end, "%Y-%m-%d") {
        Ok(d) if d > today && d <= today + chrono::Duration::days(366) => {}
        _ => return oops("The period ends on a date within a year."),
    }
    let mut all = load_objectives(&state.meta, &p.id);
    if all.iter().filter(|o| o.archived.is_none()).count() >= 10 {
        return oops("At most 10 open objectives per project.");
    }
    all.push(Objective {
        id: crate::short_hash(&format!(
            "{title}{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )),
        title,
        goals: ids,
        start: today.format("%Y-%m-%d").to_string(),
        end: f.end,
        archived: None,
    });
    match save_objectives(&state.meta, &p.id, &all) {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "objective.set", "");
            Redirect::to(&format!("/p/{}/goals", p.id)).into_response()
        }
        Err(e) => oops(&e),
    }
}

/// `POST /p/{id}/objectives/{oid}/delete` (admins).
pub(crate) async fn delete_objective(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, oid)): Path<(String, String)>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    let mut all = load_objectives(&state.meta, &id);
    all.retain(|o| o.id != oid);
    let _ = save_objectives(&state.meta, &id, &all);
    crate::privacy::audit(&state, &headers, Some(&id), "objective.remove", &oid);
    Redirect::to(&format!("/p/{id}/goals")).into_response()
}

/// `POST /p/{id}/goals/{gid}/delete` (admins).
pub(crate) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, gid)): Path<(String, String)>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    let mut all = load(&state.meta, &id);
    all.retain(|g| g.id != gid);
    let _ = save(&state.meta, &id, &all);
    crate::privacy::audit(&state, &headers, Some(&id), "goal.remove", &gid);
    Redirect::to(&format!("/p/{id}/goals")).into_response()
}

/// `GET /api/projects/{id}/goals` (bearer): every goal with its current
/// value, projection and status, and the suggested targets.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "admin bearer token required"})),
        )
            .into_response();
    }
    let Some(p) = project(&state, &id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown project"})),
        )
            .into_response();
    };
    match evaluate(&state, &p, &p.timezone).await {
        Ok(ev) => Json(json!({
            "goals": ev.rows.iter().map(|r| json!({
                "goal": r.goal, "label": r.label, "current": r.current,
                "projected": r.projected, "status": r.status, "days_left": r.days_left,
                "guardrails": r.guards.iter().map(|g| json!({
                    "label": g.label, "current": g.current, "limit": g.limit, "breached": g.breached,
                })).collect::<Vec<_>>(),
                "moved_by": { "releases": r.moves, "changes": r.marks.iter().map(|m| json!({
                    "day": m.day, "kind": m.kind, "text": m.text,
                })).collect::<Vec<_>>() },
            })).collect::<Vec<_>>(),
            "objectives": ev.objectives.iter().map(|o| json!({
                "objective": o.objective, "status": o.status,
            })).collect::<Vec<_>>(),
            "suggestions": ev.choices.iter().map(|c| json!({
                "kpi": c.kpi, "label": c.label, "current": c.current, "suggested_target": c.suggested,
                "higher_is_better": c.up, "share": c.share,
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct ApiGoal {
    kpi: String,
    /// In KPI units: a share as 0–1 (as `GET` returns them).
    target: f64,
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    releases: Option<u32>,
    #[serde(default)]
    owner: String,
    /// `[{ "kpi": "bounce_rate", "limit": 0.4 }]`; limit optional.
    #[serde(default)]
    guardrails: Vec<ApiGuard>,
}

#[derive(Deserialize)]
pub(crate) struct ApiGuard {
    kpi: String,
    #[serde(default)]
    limit: Option<f64>,
}

/// Set a goal from the Ask chat once a person confirmed it (#244): the
/// same checks as the page and the API.
pub(crate) async fn set_confirmed(
    state: &AppState,
    p: &openlens_meta::Project,
    kpi: &str,
    target: f64,
    deadline: &str,
) -> Result<Goal, String> {
    let ev = evaluate(state, p, &p.timezone).await?;
    let mut all = load(&state.meta, &p.id);
    let goal = build_goal(
        &ev,
        all.len(),
        GoalInput {
            kpi: kpi.to_owned(),
            target,
            deadline: Some(deadline.to_owned()),
            releases: None,
            owner: String::new(),
            guards: Vec::new(),
        },
    )?;
    all.push(goal.clone());
    save(&state.meta, &p.id, &all)?;
    Ok(goal)
}

/// `POST /api/projects/{id}/goals` (bearer, #272): set a goal, with the
/// same checks as the page.
pub(crate) async fn api_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(g): Json<ApiGoal>,
) -> Response {
    let bad = |s: StatusCode, m: &str| (s, Json(json!({ "error": m }))).into_response();
    if !auth::has_bearer(&state, &headers) {
        return bad(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let Some(p) = project(&state, &id).await else {
        return bad(StatusCode::NOT_FOUND, "unknown project");
    };
    let ev = match evaluate(&state, &p, &p.timezone).await {
        Ok(ev) => ev,
        Err(e) => return bad(StatusCode::BAD_REQUEST, &e),
    };
    let mut all = load(&state.meta, &p.id);
    let input = GoalInput {
        kpi: g.kpi,
        target: g.target,
        deadline: g.deadline,
        releases: g.releases,
        owner: g.owner,
        guards: g.guardrails.into_iter().map(|x| (x.kpi, x.limit)).collect(),
    };
    match build_goal(&ev, all.len(), input) {
        Ok(goal) => {
            all.push(goal.clone());
            if let Err(e) = save(&state.meta, &p.id, &all) {
                return bad(StatusCode::INTERNAL_SERVER_ERROR, &e);
            }
            crate::privacy::audit(&state, &headers, Some(&p.id), "goal.set", "api");
            Json(json!({ "stored": true, "goal": goal })).into_response()
        }
        Err(e) => bad(StatusCode::UNPROCESSABLE_ENTITY, &e),
    }
}

//! Goals on KPIs (#235): the Goals page, setting a goal from a suggested
//! target, deleting one, and the API for agents. Stored per project as
//! `project:<id>:goals` (JSON); admins change them.

use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::goals::{self, Goal, Status};
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

/// Every goal with where it stands, and a suggestion per KPI.
async fn evaluate(
    state: &AppState,
    p: &openlens_meta::Project,
    tz: &str,
) -> Result<(Vec<openlens_ui::GoalRow>, Vec<openlens_ui::GoalChoice>), String> {
    let map = crate::load_map(state, &p.id).await;
    let f = openlens_engine::Filters {
        tz: tz.to_owned(),
        ..openlens_engine::Filters::default()
    };
    let b = openlens_engine::board(&state.query, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
        .map_err(|e| e.to_string())?;
    let today = chrono::Utc::now().date_naive();
    let rows = load(&state.meta, &p.id)
        .into_iter()
        .map(|g| {
            let (current, series) = goals::kpi_values(&b, &g.kpi);
            let (st, projected) = goals::status(&g, current, &series, today);
            let (label, _, share) = goals::kpi_info(&g.kpi);
            openlens_ui::GoalRow {
                status: st.label().to_owned(),
                good: matches!(st, Status::Reached | Status::OnTrack),
                bad: matches!(st, Status::OffTrack),
                label,
                share,
                current,
                projected,
                series,
                days_left: chrono::NaiveDate::parse_from_str(&g.deadline, "%Y-%m-%d")
                    .map_or(0, |d| (d - today).num_days()),
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
            let (current, series) = goals::kpi_values(&b, &k);
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
    Ok((rows, choices))
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
        Ok((rows, choices)) => {
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
                openlens_ui::goals(&info, &rows, &choices, v.admin, &default_deadline),
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
    deadline: String,
    #[serde(default)]
    owner: String,
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
    let Ok((_, choices)) = evaluate(&state, &p, &tz).await else {
        return oops("The board is unavailable.");
    };
    let Some(c) = choices.iter().find(|c| c.kpi == f.kpi) else {
        return oops("This KPI has no value yet.");
    };
    let target = if c.share { f.target / 100.0 } else { f.target };
    if !target.is_finite() || target < 0.0 || (c.share && target > 1.0) {
        return oops("The target is out of range.");
    }
    let today = chrono::Utc::now().date_naive();
    match chrono::NaiveDate::parse_from_str(&f.deadline, "%Y-%m-%d") {
        Ok(d) if d > today && d <= today + chrono::Duration::days(3 * 366) => {}
        _ => return oops("The deadline is a date in the next three years."),
    }
    let mut all = load(&state.meta, &p.id);
    if all.len() >= 50 {
        return oops("At most 50 goals per project.");
    }
    all.push(Goal {
        id: crate::short_hash(&format!(
            "{}{}{}",
            f.kpi,
            f.deadline,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )),
        kpi: f.kpi,
        target,
        up: c.up,
        deadline: f.deadline,
        owner: f.owner.trim().chars().take(80).collect(),
        baseline: c.current,
        created: today.format("%Y-%m-%d").to_string(),
    });
    match save(&state.meta, &p.id, &all) {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "goal.set", "");
            Redirect::to(&format!("/p/{}/goals", p.id)).into_response()
        }
        Err(e) => oops(&e),
    }
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
        Ok((rows, choices)) => Json(json!({
            "goals": rows.iter().map(|r| json!({
                "goal": r.goal, "label": r.label, "current": r.current,
                "projected": r.projected, "status": r.status, "days_left": r.days_left,
            })).collect::<Vec<_>>(),
            "suggestions": choices.iter().map(|c| json!({
                "kpi": c.kpi, "label": c.label, "current": c.current, "suggested_target": c.suggested,
                "higher_is_better": c.up,
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    }
}

//! Close the DMAIC loop (#245): a finding (insight card, error group, goal,
//! map health finding) becomes a ticket in the app's backlog, prefilled for
//! a person or a coding agent (problem, evidence, links, hypothesis,
//! success metric with its check date, acceptance criteria). Copy it as
//! Markdown, or open a GitHub issue (one per finding: a second click
//! updates it). Once the issue is closed and a release has shipped it,
//! the hourly pass measures the metric before and after and comments on
//! the issue; every outcome goes into the learning log.
//!
//! Nothing is posted without a click (or an admin API call): the hourly
//! pass only comments on tickets someone already created.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::auth;
use crate::web::html_page;

/// Days after the release before the effect is measured.
const CHECK_DAYS: i64 = 14;

/// A ticket made from a finding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Ticket {
    /// Stable id of the finding: kind + subject + project, not its numbers.
    pub fp: String,
    pub kind: String,
    pub subject: String,
    pub title: String,
    /// KPI key the change should move (`error_rate`, `success:<feature>`…).
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub baseline: Option<f64>,
    #[serde(default)]
    pub baseline_n: i64,
    #[serde(default)]
    pub target: Option<f64>,
    /// Higher is better for the metric.
    #[serde(default)]
    pub up: bool,
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
    /// `improved`, `no effect` or `worse`, with the evidence.
    #[serde(default)]
    pub outcome: Option<(String, String)>,
}

/// A drafted ticket.
pub(crate) struct Draft {
    pub ticket: Ticket,
    pub markdown: String,
}

fn load(state: &AppState, project: &str) -> Vec<Ticket> {
    state
        .meta
        .project_setting(project, "tickets")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save(state: &AppState, project: &str, t: &[Ticket]) {
    if let Ok(j) = serde_json::to_string(t) {
        let _ = state.meta.set_project_setting(project, "tickets", &j);
    }
}

fn fmt(v: f64, share: bool) -> String {
    if share {
        format!("{:.1}%", v * 100.0)
    } else {
        format!("{v:.2}")
    }
}

fn filters(p: &openlens_meta::Project) -> openlens_engine::Filters {
    openlens_engine::Filters {
        tz: p.timezone.clone(),
        ..openlens_engine::Filters::default()
    }
}

/// The ticket for a finding, from what the board shows now.
pub(crate) async fn draft(
    state: &AppState,
    p: &openlens_meta::Project,
    kind: &str,
    subject: &str,
    host: &str,
) -> Result<Draft, String> {
    let f = filters(p);
    let map = crate::load_map(state, &p.id).await;
    let b = crate::boards::board(state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
        .map_err(|e| e.to_string())?;
    let base = format!("{host}/p/{}", p.id);
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
        "error" => {
            let src = openlens_engine::source(map.as_ref().map(|(m, _)| m));
            let groups = openlens_engine::error_groups(&state.query, &p.id, &src, &f)
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
            let goals: Vec<openlens_engine::goals::Goal> = state
                .meta
                .project_setting(&p.id, "goals")
                .ok()
                .flatten()
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default();
            let g = goals
                .iter()
                .find(|g| g.id == subject)
                .ok_or("No such goal.")?;
            let (label, _, share) = openlens_engine::goals::kpi_info(&g.kpi);
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
            let h = openlens_engine::map_health(&state.query, &p.id, m, &f)
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
            let w = openlens_engine::goals::weekly(
                &state.query,
                &p.id,
                map.as_ref().map(|(m, _)| m),
                &f,
            )
            .await
            .unwrap_or_default();
            (
                openlens_engine::goals::kpi_values_with(&b, &w, k).0,
                b.sessions.count,
            )
        }
        None => (None, 0),
    };
    let (label, up, share) = metric.as_deref().map_or(
        (String::new(), true, false),
        openlens_engine::goals::kpi_info,
    );
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
    }
    md.push_str("\n## Acceptance criteria\n\n- [ ] The cause is found (replay or error trace) and fixed.\n- [ ] The release that ships it is tagged (`release` in the SDK), so OpenLens can measure it.\n- [ ] No guardrail KPI gets worse.\n\n<sub>Opened from OpenLens. Fingerprint `");
    let fp = crate::short_hash(&format!("{}|{kind}|{subject}", p.id));
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
        },
        markdown: md,
    })
}

/// The GitHub target: (API base, owner/repo, token), when configured.
fn github(state: &AppState, project: &str) -> Option<(String, String, String)> {
    let repo = state
        .meta
        .project_setting(project, "tickets_repo")
        .ok()
        .flatten()
        .filter(|r| r.split('/').count() == 2)?;
    let token = std::env::var("OPENLENS_GITHUB_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            state
                .meta
                .setting("github_token")
                .ok()
                .flatten()
                .and_then(|s| crate::secrets::open(&state.data_dir, &s).ok())
        })?;
    let api = state
        .meta
        .setting("github_api")
        .ok()
        .flatten()
        .unwrap_or_else(|| "https://api.github.com".into());
    Some((api, repo, token))
}

fn gh(
    method: &str,
    url: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let auth = format!("Bearer {token}");
    let mut resp = match (method, body) {
        ("GET", _) => agent
            .get(url)
            .header("authorization", &auth)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "OpenLens")
            .call(),
        (m, b) => {
            let r = if m == "PATCH" {
                agent.patch(url)
            } else {
                agent.post(url)
            };
            r.header("authorization", &auth)
                .header("accept", "application/vnd.github+json")
                .header("user-agent", "OpenLens")
                .send_json(b.unwrap_or_default())
        }
    }
    .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let v: serde_json::Value = resp.body_mut().read_json().unwrap_or_default();
    if status >= 300 {
        return Err(format!(
            "GitHub answered {status}: {}",
            v["message"].as_str().unwrap_or("")
        ));
    }
    Ok(v)
}

/// Create the issue, or update the one already open for this finding.
pub(crate) async fn file(state: &AppState, project: &str, d: Draft) -> Result<Ticket, String> {
    let (api, repo, token) =
        github(state, project).ok_or("No GitHub repository set for this project.")?;
    let mut all = load(state, project);
    let existing = all
        .iter()
        .position(|t| t.fp == d.ticket.fp && t.issue.is_some() && t.closed.is_none());
    let body = d.markdown.clone();
    let t = match existing {
        Some(i) => {
            let (n, _) = all[i].issue.clone().unwrap_or_default();
            let url = format!("{api}/repos/{repo}/issues/{n}/comments");
            let token2 = token.clone();
            let c = format!(
                "Fresh numbers from OpenLens ({}):\n\n{body}",
                chrono::Utc::now().format("%Y-%m-%d")
            );
            tokio::task::spawn_blocking(move || {
                gh("POST", &url, &token2, Some(json!({ "body": c })))
            })
            .await
            .map_err(|e| e.to_string())??;
            all[i].clone()
        }
        None => {
            let url = format!("{api}/repos/{repo}/issues");
            let title = d.ticket.title.clone();
            let v = tokio::task::spawn_blocking(move || {
                gh(
                    "POST",
                    &url,
                    &token,
                    Some(json!({ "title": title, "body": body, "labels": ["openlens"] })),
                )
            })
            .await
            .map_err(|e| e.to_string())??;
            let mut t = d.ticket;
            t.issue = Some((
                v["number"].as_u64().unwrap_or(0),
                v["html_url"].as_str().unwrap_or("").to_owned(),
            ));
            all.push(t.clone());
            t
        }
    };
    save(state, project, &all);
    Ok(t)
}

/// The verdict on a change: before vs after, with a significance test for
/// shares when both sides are large enough (pure).
pub(crate) fn verdict(t: &Ticket, after: f64, after_n: i64, share: bool) -> (String, String) {
    let Some(before) = t.baseline else {
        return ("no effect".into(), "no baseline".into());
    };
    let delta = after - before;
    let p = if share {
        openlens_engine::stats::two_proportions(after, after_n, before, t.baseline_n)
    } else {
        None
    };
    let rel = if before.abs() > 1e-12 {
        delta / before.abs()
    } else {
        0.0
    };
    let better = (delta > 0.0) == t.up;
    let clear = match p {
        Some(p) => p < openlens_engine::stats::ALPHA,
        None => rel.abs() >= 0.05,
    };
    let word = if !clear || delta.abs() < 1e-12 {
        "no effect"
    } else if better {
        "improved"
    } else {
        "worse"
    };
    let ev = format!(
        "{} before → {} after ({:+.1}%){}",
        fmt(before, share),
        fmt(after, share),
        rel * 100.0,
        p.map_or_else(
            || " · sample too small for a significance test, judged on a 5% change".to_owned(),
            |p| format!(" · p = {p:.3}")
        )
    );
    (word.into(), ev)
}

/// Hourly: follow open issues to their release, then measure and report.
pub(crate) async fn hourly(state: &AppState, p: &openlens_meta::Project) {
    let mut all = load(state, &p.id);
    if all.iter().all(|t| t.issue.is_none() || t.outcome.is_some()) {
        return;
    }
    let Some((api, repo, token)) = github(state, &p.id) else {
        return;
    };
    let f = filters(p);
    let map = crate::load_map(state, &p.id).await;
    let Ok(b) = crate::boards::board(state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))).await
    else {
        return;
    };
    let w = openlens_engine::goals::weekly(&state.query, &p.id, map.as_ref().map(|(m, _)| m), &f)
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
            let tok = token.clone();
            if let Ok(Ok(v)) =
                tokio::task::spawn_blocking(move || gh("GET", &url, &tok, None)).await
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
        let (rel, day) = shipped.split_once('|').unwrap_or((&shipped, ""));
        let due = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
            .map(|d| d + chrono::Duration::days(CHECK_DAYS) <= today)
            .unwrap_or(false);
        if !due {
            continue;
        }
        let Some(metric) = t.metric.clone() else {
            t.outcome = Some(("no metric".into(), format!("shipped in {rel}")));
            changed = true;
            continue;
        };
        let (_, _, share) = openlens_engine::goals::kpi_info(&metric);
        let Some(after) = openlens_engine::goals::kpi_values_with(&b, &w, &metric).0 else {
            continue;
        };
        let (word, ev) = verdict(t, after, b.sessions.count, share);
        let url = format!("{api}/repos/{repo}/issues/{n}/comments");
        let tok = token.clone();
        let body = format!(
            "**OpenLens: {word}.** Shipped in `{rel}`; {ev}.{}",
            if word == "improved" {
                ""
            } else {
                "\n\nNext: look at the replays of the sessions still affected, and try the next hypothesis."
            }
        );
        if tokio::task::spawn_blocking(move || {
            gh("POST", &url, &tok, Some(json!({ "body": body })))
        })
        .await
        .is_ok_and(|r| r.is_ok())
        {
            t.outcome = Some((word, ev));
            changed = true;
        }
    }
    if changed {
        save(state, &p.id, &all);
    }
}

async fn project(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

fn host(headers: &HeaderMap) -> String {
    crate::base_url(headers)
}

#[derive(Deserialize)]
pub(crate) struct Finding {
    kind: String,
    subject: String,
}

fn info(p: &openlens_meta::Project) -> openlens_ui::ProjectInfo<'_> {
    openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    }
}

/// `GET /p/{id}/tickets/new?kind=&subject=`: the drafted ticket.
pub(crate) async fn new(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Finding>,
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
    match draft(&state, &p, &q.kind, &q.subject, &host(&headers)).await {
        Ok(d) => {
            let open = load(&state, &p.id)
                .into_iter()
                .find(|t| t.fp == d.ticket.fp && t.closed.is_none())
                .and_then(|t| t.issue);
            html_page(
                StatusCode::OK,
                openlens_ui::ticket_draft(&openlens_ui::TicketDraft {
                    p: &info(&p),
                    title: &d.ticket.title,
                    markdown: &d.markdown,
                    kind: &q.kind,
                    subject: &q.subject,
                    repo: github(&state, &p.id).map(|g| g.1),
                    existing: open.map(|i| i.1),
                    admin: v.admin,
                }),
            )
        }
        Err(e) => html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Can't draft that ticket", &e),
        ),
    }
}

/// `POST /p/{id}/tickets` (admins): open or update the GitHub issue.
pub(crate) async fn create(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<Finding>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let res = match draft(&state, &p, &f.kind, &f.subject, &host(&headers)).await {
        Ok(d) => file(&state, &p.id, d).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(t) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "ticket.file", &t.title);
            Redirect::to(&format!("/p/{}/tickets", p.id)).into_response()
        }
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't open the issue", &e),
        ),
    }
}

/// `GET /p/{id}/tickets`: tickets and the learning log.
pub(crate) async fn list(
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
    let rows: Vec<openlens_ui::TicketRow> = load(&state, &p.id)
        .into_iter()
        .rev()
        .map(|t| openlens_ui::TicketRow {
            title: t.title,
            kind: t.kind,
            created: t.created,
            url: t.issue.map(|i| i.1),
            metric: t.metric.map(|m| openlens_engine::goals::kpi_info(&m).0),
            shipped_in: t
                .shipped_in
                .map(|s| s.split('|').next().unwrap_or("").to_owned()),
            outcome: t.outcome,
        })
        .collect();
    let repo = state
        .meta
        .project_setting(&p.id, "tickets_repo")
        .ok()
        .flatten()
        .unwrap_or_default();
    html_page(
        StatusCode::OK,
        openlens_ui::tickets(
            &info(&p),
            &rows,
            &repo,
            github(&state, &p.id).is_some(),
            v.admin,
        ),
    )
}

#[derive(Deserialize)]
pub(crate) struct TargetForm {
    #[serde(default)]
    repo: String,
    /// Empty keeps the stored token.
    #[serde(default)]
    token: String,
}

/// `POST /p/{id}/tickets/target` (admins): the GitHub repository (and the
/// token, shared by every project, stored sealed).
pub(crate) async fn target(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<TargetForm>,
) -> Response {
    let repo = f
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
        return html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Not a repository", "Write owner/name."),
        );
    }
    let _ = state.meta.set_project_setting(&id, "tickets_repo", repo);
    if !f.token.trim().is_empty() {
        match crate::secrets::seal(&state.data_dir, f.token.trim()) {
            Ok(s) => {
                let _ = state.meta.set_setting("github_token", &s);
            }
            Err(e) => {
                return html_page(
                    StatusCode::BAD_REQUEST,
                    openlens_ui::message("Can't store it", &e),
                );
            }
        }
    }
    crate::privacy::audit(&state, &headers, Some(&id), "tickets.target", repo);
    Redirect::to(&format!("/p/{id}/tickets")).into_response()
}

/// `GET /api/projects/{id}/tickets` (bearer): tickets and the learning log.
pub(crate) async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let all = load(&state, &id);
    let measured: Vec<&Ticket> = all.iter().filter(|t| t.outcome.is_some()).collect();
    let improved = measured
        .iter()
        .filter(|t| t.outcome.as_ref().is_some_and(|o| o.0 == "improved"))
        .count();
    axum::Json(json!({
        "tickets": all,
        "learning": { "measured": measured.len(), "improved": improved },
    }))
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct ApiTicket {
    kind: String,
    subject: String,
    /// `markdown` (default: only draft it) or `github` (open or update).
    #[serde(default)]
    to: String,
}

/// `POST /api/projects/{id}/tickets` (bearer): draft a ticket, or file it.
pub(crate) async fn api_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::Json(b): axum::Json<ApiTicket>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(p) = project(&state, &id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let d = match draft(&state, &p, &b.kind, &b.subject, &host(&headers)).await {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(json!({ "error": e })),
            )
                .into_response();
        }
    };
    if b.to != "github" {
        return axum::Json(
            json!({ "title": d.ticket.title, "markdown": d.markdown, "fingerprint": d.ticket.fp }),
        )
        .into_response();
    }
    match file(&state, &p.id, d).await {
        Ok(t) => {
            crate::privacy::audit(&state, &headers, Some(&p.id), "ticket.file", &t.title);
            axum::Json(json!({ "ticket": t })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": e }))).into_response(),
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
        }
    }

    #[test]
    fn verdicts_need_a_clear_change_in_the_right_direction() {
        // Errors 20% → 10% on 500 sessions each: improved, significant.
        assert_eq!(verdict(&t(0.2, 500, false), 0.1, 500, true).0, "improved");
        // 20% → 19% on 500: not significant.
        assert_eq!(verdict(&t(0.2, 500, false), 0.19, 500, true).0, "no effect");
        // Errors up: worse.
        assert_eq!(verdict(&t(0.1, 500, false), 0.2, 500, true).0, "worse");
        // Not a share, small: judged on 5%.
        let (w, ev) = verdict(&t(100.0, 0, true), 110.0, 0, false);
        assert_eq!(w, "improved");
        assert!(ev.contains("judged on a 5% change"));
    }
}

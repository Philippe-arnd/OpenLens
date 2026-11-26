//! The project settings page (#219): every action of `openlens-cli init`,
//! `timezone`, `delete-project`, `verify` and `export`, from the browser.
//! Changes are admin-only (the access layer refuses other POSTs under /p/).

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use openlens_engine::commands::alerts::{self, ProjectInput};
use openlens_engine::commands::uptime;
use openlens_engine::commands::{Caller, Command, Error};

use crate::AppState;
use crate::web::html_page;

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
}

fn project_input(id: &str) -> ProjectInput {
    ProjectInput {
        project: id.to_owned(),
    }
}

async fn load(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

#[derive(Deserialize, Default)]
pub(crate) struct Notice {
    #[serde(default)]
    done: Option<String>,
    /// *Environments*: the host whose past events a dry run counts (#484).
    #[serde(default)]
    relabel: Option<String>,
    #[serde(default)]
    days: Option<String>,
    /// *Environments*: the allow-list a preview counts (#485).
    #[serde(default)]
    allow: Option<String>,
}

/// The days of a re-labelling form: empty is the default, anything not a
/// number is refused by the engine's check.
fn form_days(raw: Option<&str>) -> Option<i64> {
    match raw.map(str::trim) {
        None | Some("") => None,
        Some(d) => Some(d.parse().unwrap_or(0)),
    }
}

/// `GET /p/{id}/settings`: the *General* tab.
pub(crate) async fn page(
    viewer: crate::access::Signed,
    state: State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    nq: Query<Notice>,
) -> Response {
    page_at(
        viewer,
        state,
        headers,
        id,
        nq,
        openlens_ui::SettingsTab::General,
    )
    .await
}

/// `GET /p/{id}/settings/{install|alerts|sharing|data}` (#438).
pub(crate) async fn tab_page(
    viewer: crate::access::Signed,
    state: State<AppState>,
    headers: HeaderMap,
    Path((id, slug)): Path<(String, String)>,
    nq: Query<Notice>,
) -> Response {
    let Some(tab) = openlens_ui::SettingsTab::from_slug(&slug) else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such settings tab."),
        );
    };
    page_at(viewer, state, headers, id, nq, tab).await
}

/// `GET /p/{id}/settings/alerts`.
pub(crate) async fn alerts_page(
    viewer: crate::access::Signed,
    state: State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    nq: Query<Notice>,
) -> Response {
    page_at(
        viewer,
        state,
        headers,
        id,
        nq,
        openlens_ui::SettingsTab::Alerts,
    )
    .await
}

/// `GET /p/{id}/settings/environments` (#410).
pub(crate) async fn environments_page(
    viewer: crate::access::Signed,
    state: State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    nq: Query<Notice>,
) -> Response {
    page_at(
        viewer,
        state,
        headers,
        id,
        nq,
        openlens_ui::SettingsTab::Environments,
    )
    .await
}

/// The form of `POST /p/{id}/settings/environments`: `action` is `rule`
/// (the default: set a host's rule, or remove it when `remove` is sent),
/// `remove`, `alias`, `unalias`, `hide`, `show`, `monitor`, `unmonitor`, or
/// `relabel` (re-label a host's past events, after the dry run of the
/// page's preview, #484), or `allow` (save the host allow-list, #485).
#[derive(Debug, Deserialize)]
pub(crate) struct EnvironmentForm {
    #[serde(default)]
    action: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    env: String,
    #[serde(default)]
    remove: Option<String>,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    days: String,
    #[serde(default)]
    hosts: String,
}

impl EnvironmentForm {
    /// The change this form asks for, and what the audit log records.
    fn change(&self) -> Option<(crate::environments::RuleChange, String)> {
        use crate::environments::{MonitorChange, RuleChange};
        let t = |s: &str| s.trim().to_owned();
        let c = match (self.action.as_str(), self.remove.is_some()) {
            ("" | "rule", true) | ("remove", _) => RuleChange {
                remove_environment_rule: Some(t(&self.host)),
                ..Default::default()
            },
            ("" | "rule", false) => RuleChange {
                environment_rule: Some(openlens_engine::environments::Rule {
                    host: t(&self.host),
                    env: t(&self.env),
                }),
                ..Default::default()
            },
            ("alias", _) => RuleChange {
                environment_alias: Some(openlens_engine::environments::Alias {
                    from: t(&self.from),
                    to: t(&self.to),
                }),
                ..Default::default()
            },
            ("unalias", _) => RuleChange {
                remove_environment_alias: Some(t(&self.from)),
                ..Default::default()
            },
            ("hide", _) => RuleChange {
                hide_environment: Some(t(&self.env)),
                ..Default::default()
            },
            ("show", _) => RuleChange {
                show_environment: Some(t(&self.env)),
                ..Default::default()
            },
            ("monitor", _) => RuleChange {
                environment_monitor: Some(MonitorChange {
                    env: t(&self.env),
                    url: t(&self.url),
                    level: t(&self.level),
                }),
                ..Default::default()
            },
            ("unmonitor", _) => RuleChange {
                remove_environment_monitor: Some(t(&self.env)),
                ..Default::default()
            },
            _ => return None,
        };
        let subject = [&self.host, &self.from, &self.env]
            .into_iter()
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let action = if self.action.is_empty() {
            "rule"
        } else {
            &self.action
        };
        Some((c, format!("{action} {subject}").chars().take(200).collect()))
    }
}

/// `POST /p/{id}/settings/environments` (#410): the engine commands of
/// the environment settings (rules, aliases, hidden, monitors), as the
/// API's `PATCH /api/projects/{id}` runs them.
pub(crate) async fn environments(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<EnvironmentForm>,
) -> Response {
    if load(&state, &id).await.is_none() {
        return oops("No such project.");
    }
    if f.action == "relabel" {
        let req = crate::environments::RelabelRequest {
            host: f.host.clone(),
            days: form_days(Some(&f.days)),
            dry_run: false,
        };
        let (st, pid) = (state.clone(), id.clone());
        let done =
            tokio::task::spawn_blocking(move || crate::environments::relabel(&st, &pid, &req))
                .await;
        return match done {
            Ok(Ok(r)) => {
                crate::environments::audit_relabel(&state, &headers, &id, &r);
                Redirect::to(&format!("/p/{id}/settings/environments?done=relabelled"))
                    .into_response()
            }
            Ok(Err((_, msg))) => oops(&msg),
            Err(_) => oops("The events could not be re-labelled."),
        };
    }
    if f.action == "allow" {
        let hosts = crate::allowlist::split_hosts(&f.hosts);
        return match crate::allowlist::preview_and_save(&state, &id, &hosts, true).await {
            Ok((p, _)) => {
                crate::allowlist::audit(&state, &headers, &id, &p);
                Redirect::to(&format!("/p/{id}/settings/environments?done=allow")).into_response()
            }
            Err((_, msg)) => oops(&msg),
        };
    }
    let Some((change, detail)) = f.change() else {
        return oops("Unknown action.");
    };
    let (st, pid) = (state.clone(), id.clone());
    let done =
        tokio::task::spawn_blocking(move || crate::environments::change(&st, &pid, &change)).await;
    match done {
        Ok(Ok(_)) => {
            crate::privacy::audit(&state, &headers, Some(&id), "environments.update", &detail);
            Redirect::to(&format!("/p/{id}/settings/environments?done=environments"))
                .into_response()
        }
        Ok(Err((_, msg))) => oops(&msg),
        Err(_) => oops("The rules could not be saved."),
    }
}

async fn page_at(
    crate::access::Signed(viewer): crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    id: String,
    Query(nq): Query<Notice>,
    tab: openlens_ui::SettingsTab,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let app_url = state.meta.project_setting(&p.id, "app_url").ok().flatten();
    let detected = crate::icon::detected_origin(&state, &p.id).await;
    let counts = openlens_engine::reads::day_counts(&state.query, &p.id).await;
    let checks = [
        (
            "Events",
            counts.events,
            "add the script tag or an SDK to the app",
        ),
        (
            "Logins (identify)",
            counts.identified,
            "call identify(id, { role }) after sign-in",
        ),
        (
            "Events with a role",
            counts.with_role,
            "pass the user's role to identify",
        ),
        (
            "Releases",
            counts.releases,
            "set data-release (or release) to the app's version",
        ),
    ];
    let map_version = state.meta.feature_map(&p.id).ok().flatten().map(|(v, _)| v);
    // The latest releases, suggested on the share-link form (#403).
    let releases: Vec<String> = openlens_engine::filter_options(
        &state.query,
        &p.id,
        &crate::api::FilterQuery::default().filters(&p.timezone),
        None,
    )
    .await
    .map(|b| b.releases.into_iter().map(|r| r.release).collect())
    .unwrap_or_default();
    let yesterday = {
        let now = chrono::Utc::now() - chrono::Duration::days(1);
        now.format("%Y-%m-%d").to_string()
    };
    let notice = match nq.done.as_deref() {
        Some("saved") => Some("Saved."),
        Some("rotated") => Some("New capture key: update every app that sends events."),
        Some("created") => Some("Project created. Add the snippet below to the app."),
        Some("alerts") => Some("Alert settings saved."),
        Some("environments") => {
            Some("Saved. Rules and aliases apply to events received from now on.")
        }
        Some("relabelled") => Some(
            "Past events re-labelled. Boards follow at once; closed days are rolled up again within minutes.",
        ),
        Some("allow") => Some("Allowed hosts saved."),
        Some("status") => Some("Status page saved."),
        Some("test") => Some("Test alert sent: check every channel."),
        Some("test-none") => {
            Some("No channel is set: add an email (SMTP), a webhook or an ntfy topic.")
        }
        _ => None,
    };
    let env_tab = tab == openlens_ui::SettingsTab::Environments;
    let (environments, allow) = if env_tab {
        let pe = crate::environments::current(&state, &p.id);
        if !pe.seeded {
            crate::environments::spawn_seed(&state, &p.id);
        }
        let e = openlens_engine::environments::environments(&state.query, &p.id, &pe.rules);
        let allow = crate::allowlist::report(&state, &p.id, &pe.rules.allow);
        (e.await.ok(), Some(allow))
    } else {
        (None, None)
    };
    let allow_preview = match nq.allow.as_deref() {
        Some(hosts) if viewer.admin && env_tab => {
            crate::allowlist::tab_preview(&state, &p.id, Some(hosts)).await
        }
        _ => None,
    };
    // The dry run of a re-labelling (#484), for admins: the page shows the
    // moves, then a confirm button posts the rewrite.
    let relabel = match nq.relabel.as_deref() {
        Some(host) if viewer.admin && tab == openlens_ui::SettingsTab::Environments => {
            let req = crate::environments::RelabelRequest {
                host: host.to_owned(),
                days: form_days(nq.days.as_deref()),
                dry_run: true,
            };
            let (st, pid) = (state.clone(), p.id.clone());
            Some(
                match tokio::task::spawn_blocking(move || {
                    crate::environments::relabel(&st, &pid, &req)
                })
                .await
                {
                    Ok(Ok(r)) => Ok(r),
                    Ok(Err((_, msg))) => Err(msg),
                    Err(_) => Err("The events could not be counted.".to_owned()),
                },
            )
        }
        _ => None,
    };
    let host = crate::public_url(&headers);
    let status_page = if tab == openlens_ui::SettingsTab::Sharing {
        crate::status_page::card(&state, &p, &host, viewer.admin).await
    } else {
        None
    };
    let a = alerts::GetSettings::run(&state, &Caller::default(), project_input(&p.id))
        .await
        .unwrap_or_default();
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html_page(
        StatusCode::OK,
        openlens_ui::project_settings(
            &info,
            &openlens_ui::SettingsView {
                tab,
                timezone: &p.timezone,
                app_url: app_url.as_deref(),
                detected_url: detected.as_deref(),
                host: &host,
                checks: &checks,
                map_version,
                admin: viewer.admin,
                notice,
                yesterday: &yesterday,
                shares: state.meta.share_links(&p.id).unwrap_or_default(),
                releases: &releases,
                environments,
                relabel,
                allow,
                allow_preview,
                status_page,
                setup: setup_steps(
                    &state,
                    &p.id,
                    counts.events > 0,
                    counts.releases > 0,
                    detected.is_some(),
                ),
                alerts: {
                    openlens_ui::AlertsView {
                        down: a.down,
                        down_wait: a.down_wait,
                        traffic: a.traffic,
                        regressions: a.regressions,
                        cert: a.cert,
                        failing: a.failing,
                        budget_spent: a.budget_spent,
                        budget_burn: a.budget_burn,
                        quiet: a.quiet,
                        health_path: a.health_path,
                        email: a.email,
                        webhook: a.webhook,
                        ntfy: a.ntfy,
                        smtp: crate::digest::login_mail().is_some(),
                    }
                },
            },
        ),
    )
}

/// The setup steps of a project (#403). Required: events arrive, the app
/// address is known (set, or seen in pageviews), one alert channel. The
/// rest is shown as optional: releases, source maps, S3, SMTP, the AI key.
pub(crate) fn setup_steps(
    state: &AppState,
    project: &str,
    events: bool,
    releases: bool,
    detected_url: bool,
) -> Vec<openlens_ui::SetupStep> {
    let base = format!("/p/{project}/settings");
    let set = |k: &str| {
        state
            .meta
            .project_setting(project, k)
            .ok()
            .flatten()
            .is_some_and(|v| !v.is_empty())
    };
    let a = alerts::Settings::load(&state.meta, project);
    let smtp = crate::digest::login_mail().is_some();
    let has_events = events
        || std::fs::read_dir(
            state
                .data_dir
                .join("events")
                .join(format!("project={project}")),
        )
        .is_ok_and(|mut d| d.next().is_some());
    let maps = std::fs::read_dir(
        state
            .data_dir
            .join("sourcemaps")
            .join(format!("project={project}")),
    )
    .is_ok_and(|mut d| d.next().is_some());
    let step = |label, done, required, href: String| openlens_ui::SetupStep {
        label,
        done,
        required,
        href,
    };
    vec![
        step(
            "First event received",
            has_events,
            true,
            format!("{base}/install"),
        ),
        step(
            "App address known (for uptime)",
            set("app_url") || detected_url,
            true,
            base.clone(),
        ),
        step(
            "An alert channel (email, webhook or ntfy)",
            (a.email && smtp) || !a.webhook.is_empty() || !a.ntfy.is_empty(),
            true,
            format!("{base}/alerts"),
        ),
        step(
            "Releases detected",
            releases,
            false,
            format!("{base}/install"),
        ),
        step(
            "Source maps uploaded",
            maps,
            false,
            "/settings/tokens".into(),
        ),
        step(
            "S3 copy configured",
            crate::s3::configured(&state.meta, &state.data_dir).is_some(),
            false,
            "/settings/storage".into(),
        ),
        step("SMTP for emails", smtp, false, "/settings/storage".into()),
        step(
            "AI key for Ask",
            crate::llm::config(state).is_some(),
            false,
            "/settings/ai".into(),
        ),
    ]
}

/// Required setup steps (done, total), when some are left (#403).
pub(crate) fn setup_left(state: &AppState, project: &str) -> Option<(usize, usize)> {
    let steps = setup_steps(state, project, false, false, false);
    let req: Vec<_> = steps.iter().filter(|s| s.required).collect();
    let done = req.iter().filter(|s| s.done).count();
    (done < req.len()).then_some((done, req.len()))
}

#[derive(Deserialize)]
pub(crate) struct SaveForm {
    name: String,
    #[serde(default)]
    app_url: String,
    timezone: String,
}

/// `POST /p/{id}/settings`: name, app address, time zone.
pub(crate) async fn save(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<SaveForm>,
) -> Response {
    let Ok(tz) = crate::api::check_timezone(&f.timezone) else {
        return oops("The time zone must be an IANA name like Europe/Paris.");
    };
    let url = f.app_url.trim().to_owned();
    let app_url = if url.is_empty() {
        String::new()
    } else {
        match crate::icon::origin(&url) {
            Some(o) => o,
            None => return oops("The app address must start with https:// (or http://)."),
        }
    };
    let st = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        let m = &st.meta;
        if !m.rename_project(&id, &f.name)? {
            return Ok(None);
        }
        m.set_project_timezone(&id, &tz)?;
        let before = m.project_setting(&id, "app_url")?.unwrap_or_default();
        if before != app_url {
            m.set_project_setting(&id, "app_url", &app_url)?;
            crate::icon::forget(&st, &id);
        }
        Ok::<_, openlens_meta::MetaError>(Some(id))
    })
    .await;
    match res {
        Ok(Ok(Some(id))) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&id),
                "project.update",
                "name, app address, time zone",
            );
            Redirect::to(&format!("/p/{id}/settings?done=saved")).into_response()
        }
        Ok(Ok(None)) => html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        ),
        Ok(Err(e)) => oops(&e.to_string()),
        Err(_) => oops("Try again."),
    }
}

#[derive(Deserialize)]
pub(crate) struct ConfirmForm {
    #[serde(default)]
    confirm: String,
}

/// `POST /p/{id}/settings/rotate-key`.
pub(crate) async fn rotate(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    if f.confirm != "yes" {
        return oops("Tick the box: the old key stops working at once.");
    }
    let meta = state.meta.clone();
    let pid = id.clone();
    match tokio::task::spawn_blocking(move || meta.rotate_project_key(&pid)).await {
        Ok(Ok(Some(_))) => {
            crate::privacy::audit(&state, &headers, Some(&id), "project.rotate_key", "");
            Redirect::to(&format!("/p/{id}/settings/install?done=rotated")).into_response()
        }
        _ => oops("No such project."),
    }
}

/// `POST /p/{id}/settings/delete`: the id must be typed again.
pub(crate) async fn delete(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<ConfirmForm>,
) -> Response {
    // The name, as the page asks (the id, as `delete-project --confirm`, works too).
    let typed = f.confirm.trim();
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    if typed.is_empty() || (typed != p.name && typed != p.id) {
        return oops("Type the project's name exactly to delete it.");
    }
    let st = state.clone();
    let pid = id.clone();
    match tokio::task::spawn_blocking(move || {
        let r = crate::api::purge_project(&st, &pid);
        crate::icon::forget(&st, &pid);
        r
    })
    .await
    {
        Ok(Ok(_)) => {
            crate::privacy::audit(&state, &headers, Some(&id), "project.delete", "");
            Redirect::to("/projects").into_response()
        }
        Ok(Err(e)) => oops(&e),
        Err(_) => oops("Try again."),
    }
}

#[derive(Deserialize)]
pub(crate) struct ExportQuery {
    date: String,
}

/// `GET /p/{id}/export?date=YYYY-MM-DD`: one day as an NDJSON download
/// (admins: the export holds every event property).
pub(crate) async fn export(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    let mut r = crate::api::export_day(&state, &p.id, &q.date).await;
    if r.status().is_success()
        && let Ok(v) = axum::http::HeaderValue::from_str(&format!(
            "attachment; filename=\"{}-{}.ndjson\"",
            p.id, q.date
        ))
    {
        r.headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    r
}

#[derive(Deserialize)]
pub(crate) struct NewForm {
    name: String,
    #[serde(default)]
    timezone: String,
}

/// `POST /projects`: a new project (admins), then its settings page.
pub(crate) async fn create(
    _: crate::access::Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<NewForm>,
) -> Response {
    let tz = if f.timezone.trim().is_empty() {
        None
    } else {
        match crate::api::check_timezone(&f.timezone) {
            Ok(t) => Some(t),
            Err(e) => return oops(e),
        }
    };
    let name = f.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 {
        return oops("The name must be 1-80 characters.");
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || {
        let p = meta.create_project(&name, None)?;
        if let Some(tz) = tz {
            meta.set_project_timezone(&p.id, &tz)?;
        }
        Ok::<_, openlens_meta::MetaError>(p.id)
    })
    .await
    {
        Ok(Ok(id)) => {
            crate::privacy::audit(&state, &headers, Some(&id), "project.create", "");
            Redirect::to(&format!("/p/{id}/settings/install?done=created")).into_response()
        }
        Ok(Err(_)) => oops("A project with that name already exists."),
        Err(_) => oops("Try again."),
    }
}

#[derive(Deserialize)]
pub(crate) struct AlertsForm {
    #[serde(default)]
    down: Option<String>,
    #[serde(default)]
    down_wait: Option<u32>,
    #[serde(default)]
    traffic: Option<String>,
    #[serde(default)]
    regressions: Option<String>,
    #[serde(default)]
    cert: Option<String>,
    #[serde(default)]
    failing: Option<String>,
    #[serde(default)]
    budget_spent: Option<String>,
    #[serde(default)]
    budget_burn: Option<String>,
    #[serde(default)]
    quiet_start: String,
    #[serde(default)]
    quiet_end: String,
    #[serde(default)]
    health_path: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    webhook: String,
    #[serde(default)]
    ntfy: String,
}

/// `POST /p/{id}/settings/alerts` (#256).
pub(crate) async fn alerts(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<AlertsForm>,
) -> Response {
    let path = f.health_path.trim();
    let a = alerts::Settings {
        down: f.down.as_deref() == Some("on"),
        down_wait: f.down_wait.unwrap_or(2),
        traffic: f.traffic.as_deref() == Some("on"),
        regressions: f.regressions.as_deref() == Some("on"),
        cert: f.cert.as_deref() == Some("on"),
        failing: f.failing.as_deref() == Some("on"),
        budget_spent: f.budget_spent.as_deref() == Some("on"),
        budget_burn: f.budget_burn.as_deref() == Some("on"),
        quiet: match (
            f.quiet_start.trim().parse::<u8>(),
            f.quiet_end.trim().parse::<u8>(),
        ) {
            (Ok(a), Ok(b)) => Some((a, b)),
            _ => None,
        },
        health_path: if path.is_empty() {
            "/".into()
        } else {
            path.to_owned()
        },
        email: f.email.as_deref() == Some("on"),
        webhook: f.webhook.trim().to_owned(),
        ntfy: f.ntfy.trim().to_owned(),
    };
    let input = alerts::SetSettingsInput {
        project: id.clone(),
        settings: a,
    };
    match alerts::SetSettings::run(&state, &Caller::default(), input).await {
        Ok(()) => {
            crate::privacy::audit(&state, &headers, Some(&id), "alerts.update", "");
            Redirect::to(&format!("/p/{id}/settings/alerts?done=alerts")).into_response()
        }
        Err(Error::NotFound) => oops("No such project."),
        Err(e) => oops(&e.to_string()),
    }
}

/// `POST /p/{id}/settings/alerts/test`: one message on every channel.
pub(crate) async fn alerts_test(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let sent = match alerts::Test::run(&state, &Caller::default(), project_input(&id)).await {
        Ok(sent) => sent,
        Err(Error::NotFound) => return oops("No such project."),
        Err(e) => return oops(&e.to_string()),
    };
    if sent.is_empty() {
        return Redirect::to(&format!("/p/{id}/settings/alerts?done=test-none")).into_response();
    }
    let failed: Vec<String> = sent
        .iter()
        .filter_map(|d| d.error.as_ref().map(|e| format!("{}: {e}", d.channel)))
        .collect();
    if failed.is_empty() {
        Redirect::to(&format!("/p/{id}/settings/alerts?done=test")).into_response()
    } else {
        oops(&format!("Not delivered: {}", failed.join("; ")))
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn when(t: i64, tz: &str) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| {
        openlens_ui::local_time(&d.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)
    })
}

/// The query of the Uptime page: the page of incidents (#559).
#[derive(Debug, Deserialize)]
pub(crate) struct UptimeQuery {
    before: Option<i64>,
}

/// `GET /p/{id}/uptime` (#256): with the last 90 days and every incident,
/// page by page (#559).
pub(crate) async fn uptime(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<UptimeQuery>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let tz = crate::auth::tz_for(&state, &headers, &p.timezone);
    let u = match alerts::GetUptime::run(&state, &Caller::default(), project_input(&p.id)).await {
        Ok(u) => u,
        Err(e) => return oops(&e.to_string()),
    };
    let days_input = uptime::DaysInput {
        project: p.id.clone(),
        from: None,
        to: None,
        environment: None,
    };
    let days = match uptime::GetDays::run(&state, &Caller::default(), days_input).await {
        Ok(d) => d,
        Err(e) => return oops(&e.to_string()),
    };
    let page_input = uptime::IncidentsInput {
        project: p.id.clone(),
        before: q.before,
        limit: None,
    };
    let page = match uptime::GetIncidents::run(&state, &Caller::default(), page_input).await {
        Ok(i) => i,
        Err(e) => return oops(&e.to_string()),
    };
    let left = |s: i64| (s > 0).then(|| openlens_engine::maintenance::length(s));
    let maintenance = openlens_ui::MaintenanceView {
        rows: openlens_engine::commands::maintenance::list(&state, &p.id, &tz, now())
            .into_iter()
            .map(|w| openlens_ui::MaintenanceRow {
                id: w.id,
                when: w.when,
                length: w.length,
                state: w.state.to_owned(),
                components: w.components.join(", "),
                text: w.text,
            })
            .collect(),
        admin: crate::auth::viewer(&state, &headers).is_some_and(|v| v.admin),
        tz: tz.clone(),
    };
    let view = openlens_ui::UptimeView {
        state: u.status.state,
        since: u.status.since.map(|t| when(t, &tz)),
        url: u.url,
        avail: u.avail,
        left_out: (
            left(u.maintenance_left_out.0),
            left(u.maintenance_left_out.1),
        ),
        maintenance,
        ms: u.ms,
        incidents: page
            .incidents
            .into_iter()
            .map(|i| {
                let end = i.end.map(|e| when(e, &tz));
                let cause = if i.planned {
                    format!("{} · during planned maintenance, not announced", i.cause)
                } else {
                    i.cause
                };
                (i.rule, when(i.start, &tz), end, i.minutes, cause)
            })
            .collect(),
        older: page.next,
        paged: q.before.is_some(),
        last: u
            .last
            .into_iter()
            .map(|c| (when(c.0, &tz), c.1, c.2, c.3, c.4))
            .collect(),
        days_avail: days.availability,
        days: days.days,
        notices: crate::announcements::section(&state, &p, &tz).await,
        availability: crate::slo::section(&state, &headers, &p).await,
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let mut r = html_page(StatusCode::OK, openlens_ui::uptime(&info, &view));
    if let Ok(v) = axum::http::HeaderValue::from_str(&format!("60;url=/p/{}/uptime", p.id)) {
        r.headers_mut().insert(axum::http::header::REFRESH, v);
    }
    r
}

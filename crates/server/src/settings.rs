//! The project settings page (#219): every action of `openlens-cli init`,
//! `timezone`, `delete-project`, `verify` and `export`, from the browser.
//! Changes are admin-only (the access layer refuses other POSTs under /p/).

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

fn oops(msg: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't do that", msg),
    )
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
}

/// `GET /p/{id}/settings`.
pub(crate) async fn page(
    crate::access::Signed(viewer): crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(nq): Query<Notice>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let app_url = state.meta.project_setting(&p.id, "app_url").ok().flatten();
    let detected = crate::icon::detected_origin(&state, &p.id).await;
    let sql = "SELECT count(*) AS events, \
               count(CASE WHEN event = '$identify' THEN 1 END) AS identified, \
               count(role) AS with_role, count(DISTINCT release) AS releases \
               FROM events WHERE timestamp >= now() - INTERVAL '1 day'";
    let row = state
        .query
        .sql(&p.id, sql)
        .await
        .ok()
        .and_then(|r| r.rows.into_iter().next());
    let n = |k: &str| {
        row.as_ref()
            .and_then(|r| r.get(k))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    let checks = [
        (
            "Events",
            n("events"),
            "add the script tag or an SDK to the app",
        ),
        (
            "Logins (identify)",
            n("identified"),
            "call identify(id, { role }) after sign-in",
        ),
        (
            "Events with a role",
            n("with_role"),
            "pass the user's role to identify",
        ),
        (
            "Releases",
            n("releases"),
            "set data-release (or release) to the app's version",
        ),
    ];
    let map_version = state.meta.feature_map(&p.id).ok().flatten().map(|(v, _)| v);
    let yesterday = {
        let now = chrono::Utc::now() - chrono::Duration::days(1);
        now.format("%Y-%m-%d").to_string()
    };
    let notice = match nq.done.as_deref() {
        Some("saved") => Some("Saved."),
        Some("rotated") => Some("New capture key: update every app that sends events."),
        Some("created") => Some("Project created. Add the snippet below to the app."),
        Some("alerts") => Some("Alert settings saved."),
        Some("test") => Some("Test alert sent: check every channel."),
        Some("test-none") => {
            Some("No channel is set: add an email (SMTP), a webhook or an ntfy topic.")
        }
        _ => None,
    };
    let host = crate::public_url(&headers);
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
                setup: setup_steps(
                    &state,
                    &p.id,
                    n("events") > 0,
                    n("releases") > 0,
                    detected.is_some(),
                ),
                alerts: {
                    let a = crate::alerts::Settings::load(&state.meta, &p.id);
                    openlens_ui::AlertsView {
                        down: a.down,
                        down_wait: a.down_wait,
                        traffic: a.traffic,
                        regressions: a.regressions,
                        cert: a.cert,
                        failing: a.failing,
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
    let a = crate::alerts::Settings::load(&state.meta, project);
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
            format!("{base}#install"),
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
            format!("{base}#alerts"),
        ),
        step(
            "Releases detected",
            releases,
            false,
            format!("{base}#install"),
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
            Redirect::to(&format!("/p/{id}/settings?done=rotated")).into_response()
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
            Redirect::to(&format!("/p/{id}/settings?done=created")).into_response()
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
    if load(&state, &id).await.is_none() {
        return oops("No such project.");
    }
    let path = f.health_path.trim();
    let a = crate::alerts::Settings {
        down: f.down.as_deref() == Some("on"),
        down_wait: f.down_wait.unwrap_or(2),
        traffic: f.traffic.as_deref() == Some("on"),
        regressions: f.regressions.as_deref() == Some("on"),
        cert: f.cert.as_deref() == Some("on"),
        failing: f.failing.as_deref() == Some("on"),
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
    if let Err(e) = a.validate() {
        return oops(&e);
    }
    match serde_json::to_string(&a) {
        Ok(j) => {
            let _ = state.meta.set_project_setting(&id, "alerts", &j);
            crate::privacy::audit(&state, &headers, Some(&id), "alerts.update", "");
            Redirect::to(&format!("/p/{id}/settings?done=alerts#alerts")).into_response()
        }
        Err(e) => oops(&e.to_string()),
    }
}

/// `POST /p/{id}/settings/alerts/test`: one message on every channel.
pub(crate) async fn alerts_test(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return oops("No such project.");
    };
    let sent = crate::alerts::deliver(
        &state,
        &p.id,
        &format!("🔔 Test alert for {}", p.name),
        "This is a test from OpenLens: alerts reach you here.",
        "test",
        false,
    )
    .await;
    if sent.is_empty() {
        return Redirect::to(&format!("/p/{id}/settings?done=test-none#alerts")).into_response();
    }
    let failed: Vec<String> = sent
        .iter()
        .filter_map(|(ch, r)| r.as_ref().err().map(|e| format!("{ch}: {e}")))
        .collect();
    if failed.is_empty() {
        Redirect::to(&format!("/p/{id}/settings?done=test#alerts")).into_response()
    } else {
        oops(&format!("Not delivered: {}", failed.join("; ")))
    }
}

fn when(t: i64, tz: &str) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(String::new, |d| {
        openlens_ui::local_time(&d.format("%Y-%m-%d %H:%M:%S").to_string(), tz, false)
    })
}

/// `GET /p/{id}/uptime` (#256).
pub(crate) async fn uptime(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = load(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let tz = crate::auth::tz_for(&state, &headers, &p.timezone);
    let now = chrono::Utc::now().timestamp();
    let st = crate::alerts::status(&state, &p.id);
    let settings = crate::alerts::Settings::load(&state.meta, &p.id);
    let url = state
        .meta
        .project_setting(&p.id, "app_url")
        .ok()
        .flatten()
        .filter(|_| settings.down)
        .map(|o| format!("{o}{}", settings.health_path));
    let week = state
        .meta
        .checks(&p.id, now - 7 * 86400)
        .unwrap_or_default();
    let day: Vec<_> = week.iter().filter(|c| c.0 >= now - 86400).collect();
    let share = |v: &[&openlens_meta::Check]| {
        (!v.is_empty()).then(|| v.iter().filter(|c| c.1).count() as f64 / v.len() as f64)
    };
    let mut ms: Vec<i64> = day.iter().filter_map(|c| c.3).collect();
    ms.sort_unstable();
    let pct = |q: f64| (!ms.is_empty()).then(|| ms[((ms.len() - 1) as f64 * q).round() as usize]);
    let week_refs: Vec<_> = week.iter().collect();
    let view = openlens_ui::UptimeView {
        state: st.state,
        since: st.since.map(|t| when(t, &tz)),
        url,
        avail: (share(&day), share(&week_refs)),
        ms: (pct(0.5), pct(0.95)),
        incidents: state
            .meta
            .incidents(&p.id, now - 30 * 86400)
            .unwrap_or_default()
            .into_iter()
            .map(|(rule, start, end, cause, _)| {
                let mins = (end.unwrap_or(now) - start).max(0) / 60;
                (
                    rule,
                    when(start, &tz),
                    end.map(|e| when(e, &tz)),
                    mins,
                    cause,
                )
            })
            .collect(),
        last: week
            .iter()
            .rev()
            .take(15)
            .map(|c| (when(c.0, &tz), c.1, c.2, c.3, c.4.clone()))
            .collect(),
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

/// `GET /api/projects/{id}/status` (bearer): up, down, unknown or off,
/// with open incidents (for agents around a deploy).
pub(crate) async fn api_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !crate::auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    if load(&state, &id).await.is_none() {
        return crate::apierr::json(StatusCode::NOT_FOUND, "unknown project");
    }
    axum::Json(crate::alerts::status(&state, &id)).into_response()
}

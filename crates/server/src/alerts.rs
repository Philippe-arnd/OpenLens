//! Is the app down? (#256) The transports and the schedule of alerts:
//! the probe of the app address every minute, the certificate check, and
//! sending one message by email, webhook (Slack- and Discord-compatible)
//! or ntfy. The logic (settings, incidents, quiet hours, the checks behind
//! *traffic stopped*, *failing requests* and regressions) is the engine's
//! `alerts` commands and pass (#499).
//!
//! A non-production environment with a monitor (#410, Settings →
//! Environments) gets its own probe and certificate check, as incidents
//! `env:<env>:down|cert|cert_expired`, announced only at the `notify`
//! level; at `log` they are recorded on Uptime and never sent.

use openlens_engine::commands::alerts::{self as cmd, Settings};
use openlens_engine::commands::uptime;

use crate::AppState;

/// One probe: `(ok, status, ms, error)`. Up = any answer below 500.
fn probe(url: &str) -> (bool, Option<u16>, Option<i64>, Option<String>) {
    let agent = crate::icon::public_agent("OpenLens uptime probe", 10, false);
    let t = std::time::Instant::now();
    match agent.get(url).call() {
        Ok(r) => {
            let code = r.status().as_u16();
            let ms = i64::try_from(t.elapsed().as_millis()).unwrap_or(i64::MAX);
            (
                code < 500,
                Some(code),
                Some(ms),
                (code >= 500).then(|| format!("HTTP {code}")),
            )
        }
        Err(e) => (false, None, None, Some(e.to_string())),
    }
}

/// Send one message on every channel the project set (and email to the
/// alert list when SMTP is set). Returns what was tried and how it went.
/// The engine's alerts send through it (`Host::send_alert`).
pub(crate) async fn deliver(
    state: &AppState,
    project: &str,
    title: &str,
    body: &str,
    rule: &str,
    resolved: bool,
) -> Vec<cmd::Delivery> {
    let s = Settings::load(&state.meta, project);
    let mut out = Vec::new();
    if s.email
        && let Some((url, from)) = crate::digest::login_mail()
    {
        let mut to: Vec<String> = std::env::var("OPENLENS_ALERT_TO")
            .or_else(|_| std::env::var("OPENLENS_DIGEST_TO"))
            .unwrap_or_default()
            .split(',')
            .map(|x| x.trim().to_owned())
            .filter(|x| x.contains('@'))
            .collect();
        if to.is_empty() {
            to = state
                .meta
                .users()
                .unwrap_or_default()
                .into_iter()
                .filter(|u| u.role == "admin" && !u.disabled)
                .map(|u| u.email)
                .collect();
        }
        if !to.is_empty() {
            let r = crate::digest::send_mail(&url, &from, &to, title, body).await;
            out.push(("email".to_owned(), r));
        }
    }
    let text = format!("{title}\n{body}");
    if !s.webhook.is_empty() {
        let (url, payload) = (
            s.webhook.clone(),
            serde_json::json!({
                "text": text, "content": text,
                "project": project, "rule": rule, "state": if resolved { "resolved" } else { "open" },
            }),
        );
        let r = tokio::task::spawn_blocking(move || {
            crate::icon::public_agent("OpenLens alerts", 10, true)
                .post(&url)
                .send_json(&payload)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        out.push(("webhook".to_owned(), r));
    }
    if !s.ntfy.is_empty() {
        let url = if s.ntfy.starts_with("https://") {
            s.ntfy.clone()
        } else {
            format!("https://ntfy.sh/{}", s.ntfy)
        };
        let (t, b) = (title.to_owned(), body.to_owned());
        let prio = if resolved { "default" } else { "high" };
        let r = tokio::task::spawn_blocking(move || {
            crate::icon::public_agent("OpenLens alerts", 10, true)
                .post(&url)
                .header("Title", &t)
                .header("Priority", prio)
                .send(b.as_bytes())
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        out.push(("ntfy".to_owned(), r));
    }
    for (ch, r) in &out {
        if let Err(e) = r {
            tracing::warn!(project, channel = %ch, %e, "alert not delivered");
        }
    }
    out.into_iter()
        .map(|(channel, r)| cmd::Delivery {
            channel,
            error: r.err(),
        })
        .collect()
}

/// Every minute (background loop): probe each project with an address,
/// and every five minutes check whether its traffic stopped.
pub(crate) async fn tick(state: &AppState, minute: u64) {
    let Ok(projects) = state.meta.projects() else {
        return;
    };
    for p in projects {
        // Uptime history (#559): the days of the last week after a start,
        // then yesterday and today every hour.
        if minute == 1 {
            uptime::rebuild_recent(&state.meta, &p.id, chrono::Utc::now());
        } else if minute % 60 == 6 {
            uptime::close_days(&state.meta, &p.id, chrono::Utc::now());
        }
        let s = Settings::load(&state.meta, &p.id);
        if s.down
            && let Some(origin) = state.meta.project_setting(&p.id, "app_url").ok().flatten()
        {
            let url = format!("{origin}{}", s.health_path);
            let (ok, code, ms, err) = tokio::task::spawn_blocking(move || probe(&url))
                .await
                .unwrap_or((false, None, None, Some("probe crashed".into())));
            let failing = cmd::record_probe(&state.meta, &p.id, ok, code, ms, err.as_deref());
            let cause = err.unwrap_or_else(|| "the probe failed".into());
            if failing || ok {
                cmd::track(
                    state,
                    &p.id,
                    &p.name,
                    "down",
                    failing,
                    &format!("{origin}{}: {cause}", s.health_path),
                    i64::from(s.down_wait) * 60,
                )
                .await;
            }
        }
        Box::pin(environments(state, &p, &s, minute)).await;
        // Job check-ins (#609): missed, failed or slower runs.
        Box::pin(openlens_engine::commands::jobs::alert_pass(state, &p)).await;
        if s.regressions && minute % 60 == 1 {
            Box::pin(cmd::regressions(state, &p)).await;
        }
        if minute % 60 == 3 {
            Box::pin(crate::goals::hourly(state, &p, s.regressions)).await;
        }
        if minute % 60 == 4 {
            Box::pin(openlens_engine::commands::tickets::hourly(state, &p)).await;
        }
        if s.cert
            && minute % 60 == 2
            && let Some(origin) = state.meta.project_setting(&p.id, "app_url").ok().flatten()
            && origin.starts_with("https://")
        {
            let o = origin.clone();
            if let Ok(Ok(at)) = tokio::task::spawn_blocking(move || crate::tls::expiry(&o)).await {
                for (rule, failing, cause) in cmd::cert_checks(None, &origin, at) {
                    cmd::track(state, &p.id, &p.name, &rule, failing, &cause, 0).await;
                }
            }
        }
        if s.failing
            && minute.is_multiple_of(5)
            && let Some((failing, cause)) = Box::pin(cmd::failing_spike(&state.query, &p.id)).await
        {
            Box::pin(cmd::track(
                state, &p.id, &p.name, "failing", failing, &cause, 0,
            ))
            .await;
        }
        // Failing server routes (#594): the stats come in 10-minute windows.
        if s.failing && minute % 10 == 6 {
            Box::pin(cmd::failing_routes(state, &p)).await;
        }
        // Failing outgoing dependencies (#608): the latest closed window.
        if s.failing && minute % 5 == 1 {
            Box::pin(cmd::failing_dependencies(state, &p)).await;
        }
        // Error budgets of the environments with a target (#560).
        if minute % 5 == 2 {
            Box::pin(openlens_engine::slo::alert_pass(
                state,
                &p,
                &s,
                chrono::Utc::now(),
            ))
            .await;
        }
        Box::pin(cmd::flush_queue(state, &p.id, &s)).await;
        if s.traffic
            && minute.is_multiple_of(5)
            && let Some((stopped, cause)) =
                Box::pin(cmd::traffic_stopped(&state.query, &p.id)).await
        {
            Box::pin(cmd::track(
                state, &p.id, &p.name, "traffic", stopped, &cause, 0,
            ))
            .await;
        }
    }
}

/// The probe and certificate check of each monitored non-production
/// environment (#410): down after two failed checks in a row, as for
/// production, with the project's health path and switches. Their checks
/// are stored under their environment, for their own days (#559); the
/// last two stay in memory for the down check.
async fn environments(state: &AppState, p: &openlens_meta::Project, s: &Settings, minute: u64) {
    use openlens_engine::environments::incident_rule;
    let monitors = crate::environments::current(state, &p.id)
        .rules
        .monitors
        .clone();
    for m in monitors {
        if s.down {
            let url = format!("{}{}", m.url, s.health_path);
            let target = url.clone();
            let (ok, code, ms, err) = tokio::task::spawn_blocking(move || probe(&target))
                .await
                .unwrap_or((false, None, None, Some("probe crashed".into())));
            let found = openlens_meta::Probe {
                ok,
                status: code,
                ms,
                error: err.as_deref(),
            };
            uptime::record_env_probe(&state.meta, &p.id, &m.env, &found);
            let key = format!("{}|{}", p.id, m.env);
            let mut last = state.caches.env_checks.get(&key).unwrap_or_default();
            let failing = cmd::push_env_check(&mut last, ok);
            state.caches.env_checks.insert(key, last);
            if failing || ok {
                let cause = err.unwrap_or_else(|| "the probe failed".into());
                cmd::track(
                    state,
                    &p.id,
                    &p.name,
                    &incident_rule(&m.env, "down"),
                    failing,
                    &format!("{url}: {cause}"),
                    i64::from(s.down_wait) * 60,
                )
                .await;
            }
        }
        if s.cert && minute % 60 == 2 && m.url.starts_with("https://") {
            let o = m.url.clone();
            if let Ok(Ok(at)) = tokio::task::spawn_blocking(move || crate::tls::expiry(&o)).await {
                for (rule, failing, cause) in cmd::cert_checks(Some(&m.env), &m.url, at) {
                    Box::pin(cmd::track(state, &p.id, &p.name, &rule, failing, &cause, 0)).await;
                }
            }
        }
    }
}

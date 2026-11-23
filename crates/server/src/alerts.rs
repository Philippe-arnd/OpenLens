//! Is the app down? (#256) An optional probe of the app address (Settings)
//! every minute, *traffic stopped* learned from the last four weeks, and
//! incidents that open and close with one message each, by email, webhook
//! (Slack- and Discord-compatible) or ntfy. Rules are on by default and
//! each can be switched off or tuned per project (minimum config).

use serde::{Deserialize, Serialize};

use crate::AppState;

/// One project's alert settings (`project:<id>:alerts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct Settings {
    /// Probe the app address every minute and alert when it is down.
    pub down: bool,
    /// Minutes an incident lasts before it is announced.
    pub down_wait: u32,
    /// Alert when the traffic stops for much longer than usual.
    pub traffic: bool,
    /// Alert on regressions the board finds (a release or flag with more
    /// errors, pages turning slow) and on new error groups (#227).
    pub regressions: bool,
    /// Path the probe asks for (default `/`).
    pub health_path: String,
    /// Email the alerts (when SMTP is set).
    pub email: bool,
    /// Slack-, Discord- or generic JSON webhook URL.
    pub webhook: String,
    /// ntfy topic (`my-topic`, on ntfy.sh) or full topic URL.
    pub ntfy: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            down: true,
            down_wait: 2,
            traffic: true,
            regressions: true,
            health_path: "/".into(),
            email: true,
            webhook: String::new(),
            ntfy: String::new(),
        }
    }
}

impl Settings {
    pub(crate) fn load(meta: &openlens_meta::Meta, project: &str) -> Self {
        meta.project_setting(project, "alerts")
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(1..=60).contains(&self.down_wait) {
            return Err("wait 1 to 60 minutes".into());
        }
        if !self.health_path.starts_with('/')
            || self.health_path.len() > 200
            || self.health_path.contains(['\n', ' ', '#'])
        {
            return Err("the health path starts with / (for example /healthz)".into());
        }
        if !self.webhook.is_empty() && !self.webhook.starts_with("https://") {
            return Err("the webhook URL starts with https://".into());
        }
        if !self.ntfy.is_empty()
            && !self.ntfy.starts_with("https://")
            && !self
                .ntfy
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("ntfy: a topic name, or an https:// topic URL".into());
        }
        Ok(())
    }
}

/// The app's state for the status chip and the API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Status {
    /// `up`, `down`, `unknown` (no probe yet) or `off` (no address).
    pub state: &'static str,
    /// Since when (Unix seconds), when known.
    pub since: Option<i64>,
    /// Open incidents: (rule, started_at, cause).
    pub open: Vec<(String, i64, String)>,
}

pub(crate) fn status(state: &AppState, project: &str) -> Status {
    let m = &state.meta;
    let now = chrono::Utc::now().timestamp();
    let open: Vec<(String, i64, String)> = m
        .incidents(project, now)
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.2.is_none())
        .map(|i| (i.0, i.1, i.3))
        .collect();
    let has_url = m
        .project_setting(project, "app_url")
        .ok()
        .flatten()
        .is_some();
    let last = m.checks(project, now - 600).unwrap_or_default();
    let st = if open.iter().any(|o| o.0 == "down" || o.0 == "traffic") {
        "down"
    } else if !has_url && last.is_empty() {
        "off"
    } else if last.is_empty() {
        "unknown"
    } else {
        "up"
    };
    Status {
        state: st,
        since: open.first().map(|o| o.1),
        open,
    }
}

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

fn label(rule: &str) -> &'static str {
    match rule {
        "down" => "is down",
        "traffic" => "gets no traffic",
        r if r.starts_with("insight:") => "has a regression",
        _ => "has a problem",
    }
}

/// Send one message on every channel the project set (and email to the
/// alert list when SMTP is set). Returns what was tried and how it went.
pub(crate) async fn deliver(
    state: &AppState,
    project: &str,
    title: &str,
    body: &str,
    rule: &str,
    resolved: bool,
) -> Vec<(String, Result<(), String>)> {
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
    out
}

/// Open the rule's incident when `failing`, close it when not; announce
/// it once it has lasted `wait` seconds, and announce its end if it was.
async fn track(
    state: &AppState,
    project: &str,
    name: &str,
    rule: &str,
    failing: bool,
    cause: &str,
    wait: i64,
) {
    let m = &state.meta;
    let now = chrono::Utc::now().timestamp();
    let open = m.open_incident(project, rule).ok().flatten();
    match (open, failing) {
        (None, true) => {
            let _ = m.start_incident(project, rule, now, cause);
        }
        (Some((id, started, notified)), true) => {
            if !notified && now - started >= wait {
                let _ = m.incident_notified(id, now);
                let title = format!("🔴 {name} {}", label(rule));
                let body = format!(
                    "Since {} UTC: {cause}\n{}",
                    chrono::DateTime::from_timestamp(started, 0)
                        .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string()),
                    "See Uptime in OpenLens."
                );
                deliver(state, project, &title, &body, rule, false).await;
            }
        }
        (Some((id, started, notified)), false) => {
            let _ = m.end_incident(id, now);
            if notified {
                let mins = (now - started).max(0) / 60;
                let title = format!("✅ {name} is back");
                let body = format!("The incident lasted {mins} min ({}).", label(rule));
                deliver(state, project, &title, &body, rule, true).await;
            }
        }
        (None, false) => {}
    }
}

/// Every minute (background loop): probe each project with an address,
/// and every five minutes check whether its traffic stopped.
pub(crate) async fn tick(state: &AppState, minute: u64) {
    let Ok(projects) = state.meta.projects() else {
        return;
    };
    for p in projects {
        let s = Settings::load(&state.meta, &p.id);
        if s.down
            && let Some(origin) = state.meta.project_setting(&p.id, "app_url").ok().flatten()
        {
            let url = format!("{origin}{}", s.health_path);
            let (ok, code, ms, err) = tokio::task::spawn_blocking(move || probe(&url))
                .await
                .unwrap_or((false, None, None, Some("probe crashed".into())));
            let now = chrono::Utc::now().timestamp();
            let _ = state
                .meta
                .record_check(&p.id, now, ok, code, ms, err.as_deref());
            // Down after two failed checks in a row; up after one success.
            let recent = state.meta.checks(&p.id, now - 150).unwrap_or_default();
            let failing = recent.len() >= 2 && recent.iter().rev().take(2).all(|c| !c.1);
            let cause = err.unwrap_or_else(|| "the probe failed".into());
            if failing || ok {
                track(
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
        if s.regressions && minute % 60 == 1 {
            regressions(state, &p).await;
        }
        if s.traffic && minute % 5 == 0 {
            if let Some((stopped, cause)) = traffic_stopped(state, &p.id).await {
                track(state, &p.id, &p.name, "traffic", stopped, &cause, 0).await;
            }
        }
    }
}

/// Insight kinds that are alerts: something got worse.
const REGRESSIONS: [&str; 3] = ["release_regression", "flag_regression", "slow_pages"];

/// Hourly (#227): the board's regression cards become incidents (one
/// message when one appears, one when it is gone), and a new error group
/// hitting at least two users in the last hour is announced once.
async fn regressions(state: &AppState, p: &openlens_meta::Project) {
    let map = crate::load_map(state, &p.id).await;
    let f = openlens_engine::Filters {
        tz: p.timezone.clone(),
        ..openlens_engine::Filters::default()
    };
    let Ok(board) =
        openlens_engine::board(&state.query, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))).await
    else {
        return;
    };
    if board.total_events == 0 {
        return;
    }
    let now_cards: Vec<&openlens_engine::Insight> = board
        .insights
        .iter()
        .filter(|i| REGRESSIONS.contains(&i.kind))
        .collect();
    for c in &now_cards {
        let rule = format!("insight:{}:{}", c.kind, c.subject);
        track(
            state,
            &p.id,
            &p.name,
            &rule,
            true,
            &format!("{} — {}. {}", c.title, c.evidence, c.next_step),
            0,
        )
        .await;
    }
    // Cards that went away close their incident.
    let since = chrono::Utc::now().timestamp() - 90 * 86400;
    for (rule, _, end, cause, _) in state.meta.incidents(&p.id, since).unwrap_or_default() {
        if end.is_none()
            && rule.starts_with("insight:")
            && !now_cards
                .iter()
                .any(|c| rule == format!("insight:{}:{}", c.kind, c.subject))
        {
            track(state, &p.id, &p.name, &rule, false, &cause, 0).await;
        }
    }
    // New error groups: first seen in the last hour, two users or more.
    let src = openlens_engine::source(map.as_ref().map(|(m, _)| m));
    let Ok(groups) = openlens_engine::error_groups(&state.query, &p.id, &src, &f).await else {
        return;
    };
    let hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    for g in groups
        .iter()
        .filter(|g| g.users >= 2 && g.first_seen.replace('T', " ") >= hour_ago)
    {
        let rule = format!("error:{}", &crate::short_hash(&g.fingerprint));
        if state
            .meta
            .open_incident(&p.id, &rule)
            .ok()
            .flatten()
            .is_some()
            || state
                .meta
                .incidents(&p.id, 0)
                .unwrap_or_default()
                .iter()
                .any(|i| i.0 == rule)
        {
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        let cause = format!(
            "{} — {} users, {} times{}",
            g.sample,
            g.users,
            g.count,
            g.last_release
                .as_deref()
                .map(|r| format!(", release {r}"))
                .unwrap_or_default()
        );
        if let Ok(id) = state.meta.start_incident(&p.id, &rule, now, &cause) {
            let _ = state.meta.incident_notified(id, now);
            let _ = state.meta.end_incident(id, now);
            deliver(
                state,
                &p.id,
                &format!("🟠 {}: a new error", p.name),
                &format!("{cause}\nSee Errors in OpenLens."),
                &rule,
                false,
            )
            .await;
        }
    }
}

/// Has the traffic stopped for much longer than usual at this hour and
/// weekday? None when there is not enough history (4 weeks, 12 events an
/// hour at this time) to know.
async fn traffic_stopped(state: &AppState, project: &str) -> Option<(bool, String)> {
    use chrono::{Datelike, Timelike};
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::days(29)).format("%Y-%m-%d");
    let today = now.format("%Y-%m-%d");
    let (h, dow) = (now.hour(), now.weekday().num_days_from_sunday());
    let sql = format!(
        "SELECT min(date) AS first, \
         count(CASE WHEN date < '{today}' AND date_part('hour', timestamp) = {h} AND date_part('dow', timestamp) = {dow} THEN 1 END) AS same, \
         CAST(max(timestamp) AS VARCHAR) AS last \
         FROM raw_events WHERE date >= '{since}' AND environment <> 'bot'"
    );
    let r = state.query.sql(project, &sql).await.ok()?;
    let row = r.rows.first()?;
    let first = row.get("first")?.as_str()?.to_owned();
    if first
        > (now - chrono::Duration::days(27))
            .format("%Y-%m-%d")
            .to_string()
    {
        return None;
    }
    let per_hour = row
        .get("same")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0) as f64
        / 4.0;
    if per_hour < 12.0 {
        return None;
    }
    let window = (5.0 * 60.0 / per_hour).clamp(3.0, 120.0);
    let last = row.get("last")?.as_str()?;
    let last = chrono::NaiveDateTime::parse_from_str(
        &last.replace('T', " ")[..19.min(last.len())],
        "%Y-%m-%d %H:%M:%S",
    )
    .ok()?;
    let idle = (now.naive_utc() - last).num_seconds() as f64 / 60.0;
    Some((
        idle > window,
        format!("no event for {idle:.0} min; usually about {per_hour:.0} an hour at this time"),
    ))
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn settings_have_defaults_and_are_checked() {
        let s = Settings::default();
        assert!(s.down && s.traffic && s.email && s.validate().is_ok());
        assert!(
            Settings {
                webhook: "http://x".into(),
                ..s.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Settings {
                ntfy: "my topic".into(),
                ..s.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Settings {
                ntfy: "ops-alerts".into(),
                ..s.clone()
            }
            .validate()
            .is_ok()
        );
        assert!(
            Settings {
                health_path: "healthz".into(),
                ..s.clone()
            }
            .validate()
            .is_err()
        );
        assert!(Settings { down_wait: 0, ..s }.validate().is_err());
        let partial: Settings = serde_json::from_str(r#"{"traffic": false}"#).unwrap_or_default();
        assert!(
            !partial.traffic && partial.down,
            "missing fields take the defaults"
        );
    }
}

//! Alerts as commands (#256, #227, #266, #410), moved out of the server as
//! is (#499): a project's alert settings, the app's status and its uptime,
//! a test message, and the logic of the minute pass (incidents that open
//! and close with one message each, quiet hours, and the checks behind
//! *traffic stopped*, *failing requests* and the board's regressions).
//!
//! The process that runs them (the server) keeps the transports: the
//! probe of the app address, the certificate check, and sending a message
//! by email, webhook or ntfy ([`Host::send_alert`]). It also keeps the
//! schedule: its minute job calls the functions here.
//!
//! A non-production environment with a monitor (#410, Settings →
//! Environments) gets its own probe and certificate check, as incidents
//! `env:<env>:down|cert|cert_expired`, announced only at the `notify`
//! level; at `log` they are recorded on Uptime and never sent.

use openlens_meta::{Check, Meta, Project};
use openlens_query::QueryEngine;
use serde::{Deserialize, Serialize};

use super::maintenance::MaintenanceWindow;
use super::{Access, Caller, Command, Error, Host, short_hash};
use crate::environments::{NOTIFY, incident_rule, split_incident_rule};
use crate::maintenance::{PRODUCTION, counted, overlap_seconds, running_at};

/// One project's alert settings (`project:<id>:alerts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    /// Probe the app address every minute and alert when it is down.
    pub down: bool,
    /// Minutes an incident lasts before it is announced.
    pub down_wait: u32,
    /// Alert when the traffic stops for much longer than usual.
    pub traffic: bool,
    /// Alert on regressions the board finds (a release or flag with more
    /// errors, pages turning slow) and on new error groups (#227).
    pub regressions: bool,
    /// Warn when the app's TLS certificate expires within 14 days (#266).
    pub cert: bool,
    /// Alert on a spike of failing requests and errors across users (#266).
    pub failing: bool,
    /// Alert when an environment with an availability target has spent
    /// its 30-day error budget (#560).
    pub budget_spent: bool,
    /// Alert when, at the last day's rate, the error budget left is gone
    /// within two days (#560).
    pub budget_burn: bool,
    /// Quiet hours in the project's time zone (`[start, end)`, hours 0–23):
    /// non-urgent alerts raised then wait until they end. *Down* and an
    /// expired certificate are never held.
    pub quiet: Option<(u8, u8)>,
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
            cert: true,
            failing: true,
            budget_spent: true,
            budget_burn: true,
            quiet: None,
            health_path: "/".into(),
            email: true,
            webhook: String::new(),
            ntfy: String::new(),
        }
    }
}

impl Settings {
    /// The project's settings, or the defaults.
    #[must_use]
    pub fn load(meta: &Meta, project: &str) -> Self {
        meta.project_setting(project, "alerts")
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    /// Check the settings before they are stored.
    ///
    /// # Errors
    /// What is wrong, for the person.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=60).contains(&self.down_wait) {
            return Err("wait 1 to 60 minutes".into());
        }
        if !self.health_path.starts_with('/')
            || self.health_path.len() > 200
            || self.health_path.contains(['\n', ' ', '#'])
        {
            return Err("the health path starts with / (for example /healthz)".into());
        }
        if self.quiet.is_some_and(|(a, b)| a > 23 || b > 23 || a == b) {
            return Err("quiet hours: two different hours, 0 to 23".into());
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
pub struct Status {
    /// `up`, `down`, `maintenance` (a planned window covers production
    /// now, #563), `unknown` (no probe yet) or `off` (no address).
    pub state: &'static str,
    /// Since when (Unix seconds), when known.
    pub since: Option<i64>,
    /// Open incidents: (rule, started_at, cause).
    pub open: Vec<(String, i64, String)>,
    /// Maintenance windows running now or starting within 7 days, any
    /// component (#563), dates in the project's zone.
    pub maintenance: Vec<MaintenanceWindow>,
}

/// The app's state now.
#[must_use]
pub fn status(m: &Meta, project: &str) -> Status {
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
    let tz = m
        .project(project)
        .ok()
        .flatten()
        .map_or_else(|| "UTC".to_owned(), |p| p.timezone);
    let windows = m
        .maintenance_windows(project, now, now + 7 * 86400)
        .unwrap_or_default();
    let planned = running_at(&windows, PRODUCTION, now);
    let st = if planned.is_some() {
        "maintenance"
    } else if open.iter().any(|o| o.0 == "down" || o.0 == "traffic") {
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
        since: planned.map(|w| w.starts_at).or_else(|| {
            open.iter()
                .find(|o| o.0 == "down" || o.0 == "traffic")
                .or(open.first())
                .map(|o| o.1)
        }),
        maintenance: windows
            .iter()
            .map(|w| MaintenanceWindow::of(w, now, &tz))
            .collect(),
        open,
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

/// `alerts.status`: up, down, unknown or off, with the open incidents.
pub struct GetStatus;

impl Command for GetStatus {
    const NAME: &'static str = "alerts.status";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Status;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProjectInput) -> Result<Status, Error> {
        let p = project(host, &input.project).await?;
        Ok(status(host.meta(), &p.id))
    }
}

/// An incident on the Uptime page.
#[derive(Debug, Clone, Serialize)]
pub struct UptimeIncident {
    /// Its rule (`down`, `traffic`, `insight:…`, `env:<env>:down`…).
    pub rule: String,
    /// Start (Unix seconds).
    pub start: i64,
    /// End (Unix seconds), while it is not open.
    pub end: Option<i64>,
    /// How long it lasted, or has lasted so far, in minutes.
    pub minutes: i64,
    /// What happened.
    pub cause: String,
    /// It started during planned maintenance of its component (#563): it
    /// was not announced while the window ran.
    pub planned: bool,
}

/// What the Uptime page shows (#256).
#[derive(Debug, Clone, Serialize)]
pub struct Uptime {
    /// The app's state now.
    pub status: Status,
    /// The address the probe asks for, when the probe is on.
    pub url: Option<String>,
    /// Share of successful checks over the last day and week, the checks
    /// during planned maintenance of production left out (#563, D-076);
    /// with the project's switch on, job check-ins counted too (#609).
    pub avail: (Option<f64>, Option<f64>),
    /// Seconds of planned maintenance of production left out of the
    /// availability of the last day and week.
    pub maintenance_left_out: (i64, i64),
    /// Planned and running maintenance windows, and those ended within 7
    /// days, by start (dates in the project's zone).
    pub maintenance: Vec<MaintenanceWindow>,
    /// Median and 95th percentile response time over the last day (ms).
    pub ms: (Option<i64>, Option<i64>),
    /// Incidents of the last 30 days.
    pub incidents: Vec<UptimeIncident>,
    /// The last 15 checks, latest first.
    pub last: Vec<Check>,
}

/// `alerts.uptime`: the status, availability, response times, incidents
/// and last checks of a project.
pub struct GetUptime;

impl Command for GetUptime {
    const NAME: &'static str = "alerts.uptime";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Uptime;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProjectInput) -> Result<Uptime, Error> {
        let p = project(host, &input.project).await?;
        let meta = host.meta();
        let now = chrono::Utc::now().timestamp();
        let st = status(meta, &p.id);
        let settings = Settings::load(meta, &p.id);
        let url = meta
            .project_setting(&p.id, "app_url")
            .ok()
            .flatten()
            .filter(|_| settings.down)
            .map(|o| format!("{o}{}", settings.health_path));
        let all = meta.checks(&p.id, now - 7 * 86400).unwrap_or_default();
        // Planned maintenance is not counted (#563, D-076).
        let windows = meta
            .maintenance_windows(
                &p.id,
                now - 7 * 86400,
                now + super::maintenance::MAX_AHEAD_DAYS * 86400,
            )
            .unwrap_or_default();
        let week_refs = counted(&all, &windows, PRODUCTION);
        let day: Vec<&Check> = week_refs
            .iter()
            .copied()
            .filter(|c| c.0 >= now - 86400)
            .collect();
        let incident_windows = meta
            .maintenance_windows(&p.id, now - 30 * 86400, now)
            .unwrap_or_default();
        // Job check-ins count only when the project says so (#609, D-086).
        let jobs_on = super::jobs::in_availability(meta, &p.id);
        let share = |v: &[&Check], since: i64| {
            let (mut good, mut total) = (v.iter().filter(|c| c.1).count(), v.len());
            if jobs_on {
                let (g, t) = super::jobs::availability_counts(meta, &p.id, since);
                good += g;
                total += t;
            }
            (total > 0).then(|| good as f64 / total as f64)
        };
        let mut ms: Vec<i64> = day.iter().filter_map(|c| c.3).collect();
        ms.sort_unstable();
        let pct = |q: f64| {
            (!ms.is_empty())
                .then(|| {
                    ms.get(((ms.len() - 1) as f64 * q).round() as usize)
                        .copied()
                })
                .flatten()
        };
        Ok(Uptime {
            status: st,
            url,
            avail: (share(&day, now - 86400), share(&week_refs, now - 7 * 86400)),
            maintenance_left_out: (
                overlap_seconds(&windows, PRODUCTION, now - 86400, now),
                overlap_seconds(&windows, PRODUCTION, now - 7 * 86400, now),
            ),
            maintenance: windows
                .iter()
                .map(|w| MaintenanceWindow::of(w, now, &p.timezone))
                .collect(),
            ms: (pct(0.5), pct(0.95)),
            incidents: meta
                .incidents(&p.id, now - 30 * 86400)
                .unwrap_or_default()
                .into_iter()
                .map(|(rule, start, end, cause, _)| UptimeIncident {
                    minutes: (end.unwrap_or(now) - start).max(0) / 60,
                    planned: crate::maintenance::held_component(&rule)
                        .is_some_and(|c| running_at(&incident_windows, c, start).is_some()),
                    rule,
                    start,
                    end,
                    cause,
                })
                .collect(),
            last: all.iter().rev().take(15).cloned().collect(),
        })
    }
}

/// `alerts.settings`: a project's alert settings (the defaults when it
/// set none).
pub struct GetSettings;

impl Command for GetSettings {
    const NAME: &'static str = "alerts.settings";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Settings;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProjectInput) -> Result<Settings, Error> {
        Ok(Settings::load(host.meta(), &input.project))
    }
}

/// The input of `alerts.set`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSettingsInput {
    /// Project id.
    pub project: String,
    /// The new settings.
    #[serde(flatten)]
    pub settings: Settings,
}

/// `alerts.set`: store a project's alert settings, once checked.
pub struct SetSettings;

impl Command for SetSettings {
    const NAME: &'static str = "alerts.set";
    const ACCESS: Access = Access::Write;
    type Input = SetSettingsInput;
    type Output = ();

    async fn run<H: Host>(host: &H, _: &Caller, input: SetSettingsInput) -> Result<(), Error> {
        let p = project(host, &input.project).await?;
        input.settings.validate().map_err(Error::Invalid)?;
        let j =
            serde_json::to_string(&input.settings).map_err(|e| Error::Storage(e.to_string()))?;
        let _ = host.meta().set_project_setting(&p.id, "alerts", &j);
        Ok(())
    }
}

/// How one channel took a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Delivery {
    /// `email`, `webhook` or `ntfy`.
    pub channel: String,
    /// Why it was not delivered.
    pub error: Option<String>,
}

/// `alerts.test`: one message on every channel the project set. Empty
/// when it set none.
pub struct Test;

impl Command for Test {
    const NAME: &'static str = "alerts.test";
    const ACCESS: Access = Access::Write;
    type Input = ProjectInput;
    type Output = Vec<Delivery>;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: ProjectInput,
    ) -> Result<Vec<Delivery>, Error> {
        let p = project(host, &input.project).await?;
        Ok(host
            .send_alert(
                &p.id,
                &format!("🔔 Test alert for {}", p.name),
                "This is a test from OpenLens: alerts reach you here.",
                "test",
                false,
            )
            .await)
    }
}

// The minute pass: plain functions the server's job calls (a schedule,
// not a command, like goals' hourly pass).

fn label(rule: &str) -> &'static str {
    if let Some((_, check)) = split_incident_rule(rule) {
        return label(check);
    }
    match rule {
        "down" => "is down",
        "traffic" => "gets no traffic",
        r if r.starts_with("insight:") => "has a regression",
        r if r.starts_with("guardrail:") => "has a goal guardrail breached",
        "cert" => "has a certificate about to expire",
        "cert_expired" => "has an expired certificate",
        "failing" => "has many failing requests",
        r if r.starts_with("key:") => "has a key action failing",
        r if r.starts_with("route:") => "has a failing route",
        r if r.starts_with("dep:") => "has a dependency failing",
        "budget_spent" => "has spent its error budget",
        "budget_burn" => "is burning its error budget fast",
        r if r.starts_with("job:") && r.ends_with(":missed") => "has a job that did not run",
        r if r.starts_with("job:") && r.ends_with(":failed") => "has a job that failed",
        r if r.starts_with("job:") && r.ends_with(":slower") => "has a job running slower",
        _ => "has a problem",
    }
}

/// An error budget rule (#560): announced once, never reminded.
fn budget(rule: &str) -> bool {
    split_incident_rule(rule)
        .map_or(rule, |(_, c)| c)
        .starts_with("budget_")
}

/// Urgent rules are never held by quiet hours.
fn urgent(rule: &str) -> bool {
    let rule = split_incident_rule(rule).map_or(rule, |(_, c)| c);
    rule == "down" || rule == "cert_expired" || rule == "test" || rule.starts_with("key:")
}

/// Is an incident of this rule announced? Production's always are; a
/// monitored environment's only at the `notify` level (#410).
fn announced<H: Host>(host: &H, project: &str, rule: &str) -> bool {
    split_incident_rule(rule).is_none_or(|(env, _)| host.alert_level(project, env) == NOTIFY)
}

/// Is it within the project's quiet hours now?
fn quiet_now(meta: &Meta, project: &str, s: &Settings) -> bool {
    use chrono::Timelike;
    let Some((a, b)) = s.quiet else { return false };
    let tz = meta
        .project(project)
        .ok()
        .flatten()
        .and_then(|p| p.timezone.parse::<chrono_tz::Tz>().ok())
        .unwrap_or(chrono_tz::UTC);
    let h = u8::try_from(chrono::Utc::now().with_timezone(&tz).hour()).unwrap_or(0);
    if a < b {
        (a..b).contains(&h)
    } else {
        h >= a || h < b
    }
}

/// Send one message, unless it is non-urgent and quiet hours hold it
/// (then it waits in the project's queue and goes out when they end).
pub async fn notify<H: Host>(
    host: &H,
    project: &str,
    title: &str,
    body: &str,
    rule: &str,
    resolved: bool,
) {
    let meta = host.meta();
    let s = Settings::load(meta, project);
    if !urgent(rule) && quiet_now(meta, project, &s) {
        let mut q: Vec<(String, String, String, bool)> = meta
            .project_setting(project, "alert_queue")
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        if q.len() < 100 {
            q.push((title.to_owned(), body.to_owned(), rule.to_owned(), resolved));
        }
        if let Ok(j) = serde_json::to_string(&q) {
            let _ = meta.set_project_setting(project, "alert_queue", &j);
        }
        return;
    }
    host.send_alert(project, title, body, rule, resolved).await;
}

/// Send what quiet hours held, once they are over.
pub async fn flush_queue<H: Host>(host: &H, project: &str, s: &Settings) {
    let meta = host.meta();
    if quiet_now(meta, project, s) {
        return;
    }
    let Some(j) = meta.project_setting(project, "alert_queue").ok().flatten() else {
        return;
    };
    let _ = meta.set_project_setting(project, "alert_queue", "");
    let q: Vec<(String, String, String, bool)> = serde_json::from_str(&j).unwrap_or_default();
    for (t, b, r, res) in q {
        host.send_alert(project, &format!("{t} (held by quiet hours)"), &b, &r, res)
            .await;
    }
}

/// Open the rule's incident when `failing`, close it when not; announce
/// it once it has lasted `wait` seconds, and announce its end if it was.
/// During a planned maintenance window of its component (#563, D-076)
/// the incident opens but is neither announced nor reminded; still
/// failing when the window ends, it is announced from then.
pub async fn track<H: Host>(
    host: &H,
    project: &str,
    name: &str,
    rule: &str,
    failing: bool,
    cause: &str,
    wait: i64,
) {
    let now = chrono::Utc::now().timestamp();
    let seen = Seen {
        rule,
        failing,
        cause,
        wait,
    };
    track_at(host, project, name, &seen, now).await;
}

/// One check's result, as [`track`] takes it.
#[derive(Debug, Clone, Copy)]
pub struct Seen<'a> {
    /// The incident rule.
    pub rule: &'a str,
    /// Failing now.
    pub failing: bool,
    /// What happened.
    pub cause: &'a str,
    /// Seconds an incident lasts before it is announced.
    pub wait: i64,
}

/// [`track`] at `now` (Unix seconds).
pub async fn track_at<H: Host>(host: &H, project: &str, name: &str, seen: &Seen<'_>, now: i64) {
    let Seen {
        rule,
        failing,
        cause,
        wait,
    } = *seen;
    let m = host.meta();
    let open = m.open_incident(project, rule).ok().flatten();
    let env_name;
    let name = match split_incident_rule(rule) {
        Some((env, _)) => {
            env_name = format!("{name} ({env})");
            env_name.as_str()
        }
        None => name,
    };
    // At the `log` level an incident is recorded, never announced.
    let announce = announced(host, project, rule);
    // Planned maintenance holds it: recorded, not announced (#563).
    let held = crate::maintenance::holding(m, project, rule, now).is_some();
    match (open, failing) {
        (None, true) => {
            let _ = m.start_incident(project, rule, now, cause);
        }
        (Some(_), true) if !announce || held => {}
        (Some((id, started, notified)), true) => {
            let since = chrono::DateTime::from_timestamp(started, 0)
                .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string());
            if !notified && now - started >= wait {
                let _ = m.incident_notified(id, now);
                let title = format!("🔴 {name} {}", label(rule));
                let after = crate::maintenance::ended_between(m, project, rule, started, now)
                    .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                    .map(|t| {
                        format!(
                            "\nStill failing after the planned maintenance that ended at {} UTC.",
                            t.format("%H:%M")
                        )
                    })
                    .unwrap_or_default();
                let body = format!("Since {since} UTC: {cause}{after}\nSee Uptime in OpenLens.");
                notify(host, project, &title, &body, rule, false).await;
            } else if notified
                && !rule.starts_with("insight:")
                && !budget(rule)
                // A job's incident is announced once (#609, D-086).
                && !rule.starts_with("job:")
                && m.incident_last_notified(id)
                    .ok()
                    .flatten()
                    .is_some_and(|t| now - t >= 3600)
            {
                // Still going: a reminder at most every hour.
                let _ = m.incident_notified(id, now);
                let mins = (now - started).max(0) / 60;
                let title = format!(
                    "🔴 {name} still {} ({mins} min)",
                    label(rule)
                        .trim_start_matches("is ")
                        .trim_start_matches("has ")
                );
                let body = format!("Since {since} UTC: {cause}");
                notify(host, project, &title, &body, rule, false).await;
            }
        }
        (Some((id, started, notified)), false) => {
            let _ = m.end_incident(id, now);
            if notified {
                let mins = (now - started).max(0) / 60;
                let title = if budget(rule) {
                    format!("✅ {name}: error budget back on track")
                } else {
                    format!("✅ {name} is back")
                };
                let body = format!("The incident lasted {mins} min ({}).", label(rule));
                notify(host, project, &title, &body, rule, true).await;
            }
        }
        (None, false) => {}
    }
}

/// Record a probe of production's app address and say whether it is
/// down: after two failed checks in a row; up after one success.
pub fn record_probe(
    meta: &Meta,
    project: &str,
    ok: bool,
    code: Option<u16>,
    ms: Option<i64>,
    err: Option<&str>,
) -> bool {
    let now = chrono::Utc::now().timestamp();
    let _ = meta.record_check(project, now, ok, code, ms, err);
    let recent = meta.checks(project, now - 150).unwrap_or_default();
    recent.len() >= 2 && recent.iter().rev().take(2).all(|c| !c.1)
}

/// Add a probe of a monitored environment to its last results (kept by
/// the caller, at most two) and say whether it is down: two failed checks
/// in a row, as for production.
pub fn push_env_check(last: &mut Vec<bool>, ok: bool) -> bool {
    last.push(ok);
    if last.len() > 2 {
        last.remove(0);
    }
    last.len() == 2 && last.iter().all(|ok| !ok)
}

/// The two certificate checks of `origin`, whose certificate expires at
/// `at` (Unix seconds): `(rule, failing, cause)` for *about to expire*
/// (within 14 days) and *expired*. `env` names a monitored environment's
/// rules (#410); `None` is production's.
#[must_use]
pub fn cert_checks(env: Option<&str>, origin: &str, at: i64) -> [(String, bool, String); 2] {
    let days = (at - chrono::Utc::now().timestamp()) / 86400;
    let when = chrono::DateTime::from_timestamp(at, 0)
        .map_or_else(String::new, |t| t.format("%Y-%m-%d").to_string());
    let rule = |check: &str| env.map_or_else(|| check.to_owned(), |e| incident_rule(e, check));
    [
        (
            rule("cert"),
            (0..=14).contains(&days),
            format!("{origin}: the certificate expires on {when} ({days} days)"),
        ),
        (
            rule("cert_expired"),
            days < 0,
            format!("{origin}: the certificate expired on {when}"),
        ),
    ]
}

/// Insight kinds that are alerts: something got worse.
const REGRESSIONS: [&str; 3] = ["release_regression", "flag_regression", "slow_pages"];

/// Hourly (#227): the board's regression cards become incidents (one
/// message when one appears, one when it is gone), and a new error group
/// hitting at least two users in the last hour is announced once.
pub async fn regressions<H: Host>(host: &H, p: &Project) {
    let map = host.feature_map(&p.id).await;
    let f = crate::Filters {
        tz: p.timezone.clone(),
        ..crate::Filters::default()
    };
    let Ok(board) = host
        .board(&p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
        .await
    else {
        return;
    };
    if board.total_events == 0 {
        return;
    }
    let now_cards: Vec<&crate::Insight> = board
        .insights
        .iter()
        .filter(|i| REGRESSIONS.contains(&i.kind))
        .collect();
    for c in &now_cards {
        let rule = format!("insight:{}:{}", c.kind, c.subject);
        Box::pin(track(
            host,
            &p.id,
            &p.name,
            &rule,
            true,
            &format!("{} — {}. {}", c.title, c.evidence, c.next_step),
            0,
        ))
        .await;
    }
    let meta = host.meta();
    // Cards that went away close their incident.
    let since = chrono::Utc::now().timestamp() - 90 * 86400;
    for (rule, _, end, cause, _) in meta.incidents(&p.id, since).unwrap_or_default() {
        if end.is_none()
            && rule.starts_with("insight:")
            && !now_cards
                .iter()
                .any(|c| rule == format!("insight:{}:{}", c.kind, c.subject))
        {
            Box::pin(track(host, &p.id, &p.name, &rule, false, &cause, 0)).await;
        }
    }
    // New error groups: first seen in the last hour, two users or more.
    let src = crate::source(map.as_ref().map(|(m, _)| m));
    let Ok(groups) = crate::error_groups(host.query(), &p.id, &src, &f).await else {
        return;
    };
    let hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    for g in groups
        .iter()
        .filter(|g| g.users >= 2 && g.first_seen.replace('T', " ") >= hour_ago)
    {
        let rule = format!("error:{}", short_hash(&g.fingerprint));
        if meta.open_incident(&p.id, &rule).ok().flatten().is_some()
            || meta
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
        if let Ok(id) = meta.start_incident(&p.id, &rule, now, &cause) {
            let _ = meta.incident_notified(id, now);
            let _ = meta.end_incident(id, now);
            Box::pin(notify(
                host,
                &p.id,
                &format!("🟠 {}: a new error", p.name),
                &format!("{cause}\nSee Errors in OpenLens."),
                &rule,
                false,
            ))
            .await;
        }
    }
}

/// Every 10 minutes (#594, D-083): each production route failing over the
/// last hour opens its incident (`key:` for a feature's key action, which
/// is urgent, `route:` otherwise); a route that recovered closes it.
pub async fn failing_routes<H: Host>(host: &H, p: &Project) {
    let map = host.feature_map(&p.id).await;
    let Ok(now) =
        crate::routes::failing_routes(host.query(), &p.id, map.as_ref().map(|(m, _)| m)).await
    else {
        return;
    };
    let rule_of = |f: &crate::routes::Failing| {
        let kind = if f.key_of.is_some() { "key" } else { "route" };
        format!("{kind}:{}", f.route)
    };
    for f in &now {
        Box::pin(track(host, &p.id, &p.name, &rule_of(f), true, &f.cause, 0)).await;
    }
    let since = chrono::Utc::now().timestamp() - 90 * 86400;
    for (rule, _, end, cause, _) in host.meta().incidents(&p.id, since).unwrap_or_default() {
        if end.is_none()
            && (rule.starts_with("route:") || rule.starts_with("key:"))
            && !now.iter().any(|f| rule_of(f) == rule)
        {
            Box::pin(track(host, &p.id, &p.name, &rule, false, &cause, 0)).await;
        }
    }
}

/// Every 5 minutes (#608, D-090): each external host of production
/// failing in the latest closed window opens its incident (`dep:<host>`),
/// announced once it has failed for 15 minutes; a host that recovered
/// closes it.
pub async fn failing_dependencies<H: Host>(host: &H, p: &Project) {
    let Ok(now) = crate::dependencies::failing_hosts(host.query(), &p.id).await else {
        return;
    };
    track_dependencies_at(host, p, &now, chrono::Utc::now().timestamp()).await;
}

/// [`failing_dependencies`] with the hosts failing at `now` (Unix
/// seconds): open or keep each one's incident, close the others'.
pub async fn track_dependencies_at<H: Host>(
    host: &H,
    p: &Project,
    failing: &[crate::dependencies::Failing],
    now: i64,
) {
    for f in failing {
        let rule = crate::dependencies::rule(&f.host);
        let seen = Seen {
            rule: &rule,
            failing: true,
            cause: &f.cause,
            wait: crate::dependencies::FAILING_WAIT_SECS,
        };
        Box::pin(track_at(host, &p.id, &p.name, &seen, now)).await;
    }
    let since = now - 90 * 86400;
    for (rule, _, end, cause, _) in host.meta().incidents(&p.id, since).unwrap_or_default() {
        if end.is_none()
            && rule.starts_with("dep:")
            && !failing
                .iter()
                .any(|f| crate::dependencies::rule(&f.host) == rule)
        {
            let seen = Seen {
                rule: &rule,
                failing: false,
                cause: &cause,
                wait: crate::dependencies::FAILING_WAIT_SECS,
            };
            Box::pin(track_at(host, &p.id, &p.name, &seen, now)).await;
        }
    }
}

/// Many users hitting failing requests or errors in the last 15 minutes,
/// against the same quarter-hour of the same weekday over four weeks.
pub async fn failing_spike(query: &QueryEngine, project: &str) -> Option<(bool, String)> {
    use chrono::{Datelike, Timelike};
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::days(29)).format("%Y-%m-%d");
    let today = now.format("%Y-%m-%d");
    let (h, dow) = (now.hour(), now.weekday().num_days_from_sunday());
    let sql = format!(
        "SELECT count(DISTINCT CASE WHEN timestamp >= now() - INTERVAL '15 minutes' THEN distinct_id END) AS now_users, \
         count(DISTINCT CASE WHEN date < '{today}' AND date_part('hour', timestamp) = {h} AND date_part('dow', timestamp) = {dow} \
               THEN concat(distinct_id, '|', date) END) AS base \
         FROM raw_events WHERE date >= '{since}' AND environment = 'production' AND event IN ('$request_failed', '$error')"
    );
    let r = query.sql(project, &sql).await.ok()?;
    let row = r.rows.first()?;
    let now_users = row
        .get("now_users")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    // Users per hour on that weekday, over four weeks → per quarter-hour.
    let base = row
        .get("base")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0) as f64
        / 16.0;
    let spike = now_users >= 5 && now_users as f64 >= 3.0 * base + 1.0;
    Some((
        spike,
        format!(
            "{now_users} users hit failing requests or errors in 15 minutes (usually about {base:.1})"
        ),
    ))
}

/// Has the traffic stopped for much longer than usual at this hour and
/// weekday? None when there is not enough history (4 weeks, 12 events an
/// hour at this time) to know.
pub async fn traffic_stopped(query: &QueryEngine, project: &str) -> Option<(bool, String)> {
    use chrono::{Datelike, Timelike};
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::days(29)).format("%Y-%m-%d");
    let today = now.format("%Y-%m-%d");
    let (h, dow) = (now.hour(), now.weekday().num_days_from_sunday());
    let sql = format!(
        "SELECT min(date) AS first, \
         count(CASE WHEN date < '{today}' AND date_part('hour', timestamp) = {h} AND date_part('dow', timestamp) = {dow} THEN 1 END) AS same, \
         CAST(max(timestamp) AS VARCHAR) AS last \
         FROM raw_events WHERE date >= '{since}' AND environment <> 'bot' AND event NOT IN ('$request_stats', '$runtime_stats', '$client_stats')"
    );
    let r = query.sql(project, &sql).await.ok()?;
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
mod tests;

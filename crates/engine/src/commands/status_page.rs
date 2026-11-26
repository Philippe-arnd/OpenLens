//! The public status page (#561, D-079): a read-only page anyone can open,
//! without signing in, that says whether the app works and how it did
//! lately.
//!
//! Three commands:
//! - `status_page.get` (read): the settings, and the environments that can
//!   be shown;
//! - `status_page.set` (write, audited by the front end): on or off, the
//!   slug of `/status/<slug>`, the components and their public names, and
//!   whether each shows its response time;
//! - `status_page.public` ([`Access::Public`]): what the page shows, built
//!   only from public fields. An unknown or disabled slug is
//!   [`Error::NotFound`], whatever the reason, so a scan cannot tell them
//!   apart.
//!
//! What [`public`] returns never holds a project id, an address, a health
//! path, a probe's error text, an incident's cause, an event, a person or
//! who scheduled a maintenance window: components carry their public name
//! only, and a detected incident is a component, a start and an end.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use openlens_meta::status_page::StatusPageRow;
use openlens_meta::{Check, Meta, MetaError, UptimeDay};
use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host, announcements, uptime};
use crate::environments::{MAX_MONITORS, Rules, split_incident_rule};
use crate::maintenance::{PRODUCTION, Window, covers, running_at, state_at};

/// Shortest slug.
pub const SLUG_MIN: usize = 3;
/// Longest slug.
pub const SLUG_MAX: usize = 40;
/// Longest public name of a component, in characters.
pub const MAX_NAME: usize = 60;
/// Components a page shows at most: production and every monitored
/// environment.
pub const MAX_COMPONENTS: usize = 1 + MAX_MONITORS;
/// Days of bars per component.
pub const DAYS: i64 = 90;
/// The availability windows, in order (the badge takes the same names).
pub const WINDOWS: [&str; 4] = ["24h", "7d", "30d", "90d"];
/// Production's public name when none is given.
pub const DEFAULT_NAME: &str = "App";
/// Announcements the page lists at most.
pub const MAX_ANNOUNCEMENTS: usize = 50;
/// Days ahead the page lists planned maintenance.
pub const UPCOMING_DAYS: i64 = 30;
/// A component without a check in this many seconds is `unknown`.
const STALE_SECS: i64 = 600;

/// The overall and component states, from the calmest.
pub const STATES: [&str; 5] = ["operational", "unknown", "maintenance", "degraded", "down"];

fn rank(state: &str) -> usize {
    STATES.iter().position(|s| *s == state).unwrap_or(0)
}

fn storage(e: MetaError) -> Error {
    match e {
        MetaError::Invalid(m) => Error::Invalid(m.to_owned()),
        e => Error::Storage(e.to_string()),
    }
}

/// One line of the page: an environment and its public name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    /// `production`, or a monitored environment (#410).
    pub environment: String,
    /// What the page calls it ("API", "Web app").
    #[serde(default)]
    pub name: String,
    /// Show its response time (p95 over 24 hours).
    #[serde(default)]
    pub response_time: bool,
}

/// A project's status page settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Settings {
    /// Served to anyone at `/status/<slug>`.
    pub enabled: bool,
    /// The slug, once one was chosen.
    pub slug: Option<String>,
    /// Production first, then the chosen environments.
    pub components: Vec<Component>,
}

fn production() -> Component {
    Component {
        environment: PRODUCTION.to_owned(),
        name: DEFAULT_NAME.to_owned(),
        response_time: false,
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            slug: None,
            components: vec![production()],
        }
    }
}

/// Is `s` a slug as stored: 3 to 40 of `a-z`, `0-9` and `-`, not starting
/// or ending with `-`? The public routes call it before any lookup.
#[must_use]
pub fn valid_slug(s: &str) -> bool {
    (SLUG_MIN..=SLUG_MAX).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// A slug as stored: trimmed and lowercased, then [`valid_slug`].
///
/// # Errors
/// What is wrong, for the person.
pub fn check_slug(raw: &str) -> Result<String, Error> {
    let s = raw.trim().to_ascii_lowercase();
    if valid_slug(&s) {
        Ok(s)
    } else {
        Err(Error::Invalid(format!(
            "the address is {SLUG_MIN} to {SLUG_MAX} lowercase letters, digits and dashes (not at the ends)"
        )))
    }
}

/// A public name as stored: trimmed, one line, 1 to [`MAX_NAME`]
/// characters, no control characters.
///
/// # Errors
/// What is wrong, for the person.
pub fn check_name(raw: &str) -> Result<String, Error> {
    let n = raw.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME || n.chars().any(char::is_control) {
        return Err(Error::Invalid(format!(
            "a component's public name is one line of 1 to {MAX_NAME} characters"
        )));
    }
    Ok(n.to_owned())
}

/// The environments that can be shown besides production: the monitored
/// ones (Settings → Environments).
#[must_use]
pub fn monitored(meta: &Meta, project: &str) -> Vec<String> {
    let json = meta.project_setting(project, "environments").ok().flatten();
    Rules::parse(json.as_deref())
        .monitors
        .into_iter()
        .map(|m| m.env)
        .collect()
}

/// Components as stored: production first (named [`DEFAULT_NAME`] when
/// not given), then the given monitored environments in order, each once,
/// with a public name (the environment's own when empty).
///
/// # Errors
/// An environment that is not monitored, a bad name, or too many.
pub fn check_components(raw: &[Component], monitored: &[String]) -> Result<Vec<Component>, Error> {
    if raw.len() > MAX_COMPONENTS {
        return Err(Error::Invalid(format!(
            "{MAX_COMPONENTS} components at most"
        )));
    }
    let mut prod = None;
    let mut rest: Vec<Component> = Vec::new();
    for c in raw {
        let env = c.environment.trim().to_ascii_lowercase();
        let name = if c.name.trim().is_empty() {
            if env == PRODUCTION {
                DEFAULT_NAME.to_owned()
            } else {
                env.clone()
            }
        } else {
            check_name(&c.name)?
        };
        let comp = Component {
            environment: env.clone(),
            name,
            response_time: c.response_time,
        };
        if env == PRODUCTION {
            if prod.is_some() {
                return Err(Error::Invalid("production is listed twice".into()));
            }
            prod = Some(comp);
        } else if !monitored.contains(&env) {
            return Err(Error::Invalid(format!(
                "{:?} is not a monitored environment (Settings → Environments)",
                c.environment.chars().take(64).collect::<String>()
            )));
        } else if rest.iter().any(|r| r.environment == env) {
            return Err(Error::Invalid(format!("{env} is listed twice")));
        } else {
            rest.push(comp);
        }
    }
    let mut out = vec![prod.unwrap_or_else(production)];
    out.extend(rest);
    Ok(out)
}

/// The settings of a stored row. Components that no longer pass the
/// checks (an environment that lost its monitor) are left out, so the
/// page never shows what the admin did not choose.
fn settings_of(row: &StatusPageRow, monitored: &[String]) -> Settings {
    let stored: Vec<Component> = serde_json::from_str(&row.components).unwrap_or_default();
    let mut components: Vec<Component> = Vec::new();
    for c in stored {
        let ok = c.environment == PRODUCTION || monitored.contains(&c.environment);
        let named = check_name(&c.name).is_ok_and(|n| n == c.name);
        if ok && named && !components.iter().any(|x| x.environment == c.environment) {
            components.push(c);
        }
    }
    if components
        .first()
        .is_none_or(|c| c.environment != PRODUCTION)
    {
        components.retain(|c| c.environment != PRODUCTION);
        components.insert(0, production());
    }
    components.truncate(MAX_COMPONENTS);
    Settings {
        enabled: row.enabled,
        slug: Some(row.slug.clone()),
        components,
    }
}

/// The project's settings, or the defaults (off, no slug).
///
/// # Errors
/// When the store cannot be read.
pub fn load(meta: &Meta, project: &str) -> Result<Settings, Error> {
    Ok(meta
        .status_page(project)
        .map_err(storage)?
        .map_or_else(Settings::default, |row| {
            settings_of(&row, &monitored(meta, project))
        }))
}

async fn project<H: Host>(host: &H, id: &str) -> Result<openlens_meta::Project, Error> {
    host.project(id).await.ok_or(Error::NotFound)
}

/// What `status_page.get` returns.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    /// The settings.
    #[serde(flatten)]
    pub settings: Settings,
    /// `/status/<slug>` once a slug is chosen (prefix it with the
    /// server's public address).
    pub path: Option<String>,
    /// The environments that can be shown besides production.
    pub monitored: Vec<String>,
}

/// The input of the commands that only name a project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
}

/// `status_page.get`: the status page settings of a project.
pub struct Get;

impl Command for Get {
    const NAME: &'static str = "status_page.get";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = View;

    async fn run<H: Host>(host: &H, _: &Caller, i: ProjectInput) -> Result<View, Error> {
        let p = project(host, &i.project).await?;
        let settings = load(host.meta(), &p.id)?;
        Ok(View {
            path: settings.slug.as_ref().map(|s| format!("/status/{s}")),
            monitored: monitored(host.meta(), &p.id),
            settings,
        })
    }
}

/// The input of `status_page.set`: only the fields given change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetInput {
    /// Project id.
    #[serde(default)]
    pub project: String,
    /// Serve the page (needs a slug).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// The slug of `/status/<slug>`.
    #[serde(default)]
    pub slug: Option<String>,
    /// Production and the monitored environments to show, with their
    /// public names and whether their response time shows.
    #[serde(default)]
    pub components: Option<Vec<Component>>,
}

/// The settings once `i` is applied to `current`, checked.
///
/// # Errors
/// What is wrong, for the person.
pub fn changed(current: &Settings, i: &SetInput, monitored: &[String]) -> Result<Settings, Error> {
    let mut s = current.clone();
    if let Some(slug) = &i.slug {
        s.slug = Some(check_slug(slug)?);
    }
    if let Some(on) = i.enabled {
        s.enabled = on;
    }
    if let Some(c) = &i.components {
        s.components = check_components(c, monitored)?;
    }
    if s.slug.is_none() && (s.enabled || i.components.is_some()) {
        return Err(Error::Invalid(
            "choose the page's address (slug) first".into(),
        ));
    }
    Ok(s)
}

/// `status_page.set`: turn the page on or off, choose its slug (unique
/// across projects), its components and their public names, and whether
/// each shows its response time. The front end audits it.
pub struct Set;

impl Command for Set {
    const NAME: &'static str = "status_page.set";
    const ACCESS: Access = Access::Write;
    type Input = SetInput;
    type Output = View;

    async fn run<H: Host>(host: &H, _: &Caller, i: SetInput) -> Result<View, Error> {
        let p = project(host, &i.project).await?;
        let meta = host.meta();
        let envs = monitored(meta, &p.id);
        let current = load(meta, &p.id)?;
        let s = changed(&current, &i, &envs)?;
        if let Some(slug) = &s.slug {
            let json =
                serde_json::to_string(&s.components).map_err(|e| Error::Storage(e.to_string()))?;
            let now = Utc::now().timestamp();
            meta.save_status_page(&p.id, slug, s.enabled, &json, now)
                .map_err(storage)?;
        }
        let settings = load(meta, &p.id)?;
        Ok(View {
            path: settings.slug.as_ref().map(|s| format!("/status/{s}")),
            monitored: envs,
            settings,
        })
    }
}

/// One day of a component's bars.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicDay {
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// `up`, `partial`, `down` or `none` (not checked).
    pub state: &'static str,
    /// Share of the day's checks that succeeded.
    pub availability: Option<f64>,
}

/// Availability over one of [`WINDOWS`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Figure {
    /// `24h`, `7d`, `30d` or `90d`.
    pub window: &'static str,
    /// Share of the measured time that was up; `None` when nothing was
    /// measured.
    pub availability: Option<f64>,
}

/// The SLO of a component, when a target is set (#560).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicSlo {
    /// Target availability (`0.999`).
    pub target: f64,
    /// Availability over the SLO window.
    pub availability: Option<f64>,
    /// Seconds of down time the target allows.
    pub budget_seconds: i64,
    /// Seconds of it left.
    pub left_seconds: i64,
}

/// One component of the page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicComponent {
    /// Its public name.
    pub name: String,
    /// `operational`, `degraded`, `down`, `maintenance` or `unknown`.
    pub state: &'static str,
    /// Since when (Unix seconds), when known.
    pub since: Option<i64>,
    /// Availability over each of [`WINDOWS`].
    pub availability: Vec<Figure>,
    /// The last [`DAYS`] days, oldest first.
    pub days: Vec<PublicDay>,
    /// p95 response time over 24 hours, ms, when the admin shows it.
    pub response_p95_ms: Option<i64>,
    /// The SLO, when a target is set.
    pub slo: Option<PublicSlo>,
}

/// A dated update of an announcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicUpdate {
    /// Posted (Unix seconds).
    pub at: i64,
    /// `investigating`, `identified`, `monitoring` or `resolved`.
    pub state: String,
    /// Plain text.
    pub body: String,
}

/// An announcement, as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicAnnouncement {
    /// Its id (stable, for the feed).
    pub id: i64,
    /// Title.
    pub title: String,
    /// Plain text.
    pub body: String,
    /// `information`, `degraded`, `outage` or `resolved`.
    pub level: String,
    /// The public names of the components it concerns.
    pub components: Vec<String>,
    /// Start (Unix seconds).
    pub starts_at: i64,
    /// End, if any.
    pub ends_at: Option<i64>,
    /// Started and not ended.
    pub active: bool,
    /// The pinned notice.
    pub pinned: bool,
    /// Last change, or its last update (Unix seconds).
    pub updated_at: i64,
    /// Updates, oldest first.
    pub updates: Vec<PublicUpdate>,
}

/// Planned maintenance, as the page shows it: when and what it covers
/// (its text and who scheduled it stay inside).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicMaintenance {
    /// Start (Unix seconds).
    pub starts_at: i64,
    /// End (Unix seconds).
    pub ends_at: i64,
    /// `planned` or `running`.
    pub state: &'static str,
    /// The public names of the components it covers.
    pub components: Vec<String>,
}

/// A detected incident no announcement follows: a component was down
/// from `started_at` to `ended_at` (its cause stays inside).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicIncident {
    /// The component's public name.
    pub component: String,
    /// Start (Unix seconds).
    pub started_at: i64,
    /// End, once over.
    pub ended_at: Option<i64>,
    /// Its length so far, seconds.
    pub seconds: i64,
}

/// What the public status page shows, and nothing else.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Public {
    /// The project's name.
    pub name: String,
    /// `operational`, `degraded`, `down`, `maintenance` or `unknown`.
    pub state: &'static str,
    /// Since when (Unix seconds), when known.
    pub since: Option<i64>,
    /// When this was computed (Unix seconds).
    pub generated_at: i64,
    /// Production first, then the chosen environments.
    pub components: Vec<PublicComponent>,
    /// The pinned notice, if one is active.
    pub notice: Option<PublicAnnouncement>,
    /// Announcements of the last [`announcements::PUBLIC_DAYS`] days,
    /// newest first.
    pub announcements: Vec<PublicAnnouncement>,
    /// Maintenance running now or planned within [`UPCOMING_DAYS`].
    pub maintenance: Vec<PublicMaintenance>,
    /// Detected incidents of the last [`DAYS`] days that no announcement
    /// follows, newest first.
    pub incidents: Vec<PublicIncident>,
}

impl Public {
    /// The availability of production over `window`.
    #[must_use]
    pub fn availability(&self, window: &str) -> Option<f64> {
        self.components
            .first()?
            .availability
            .iter()
            .find(|f| f.window == window)?
            .availability
    }
}

/// The component an incident rule is about, and whether the rule takes it
/// down (`down`, `traffic`) or only degrades it (`failing`).
fn incident_component(rule: &str) -> Option<(&str, &'static str)> {
    match split_incident_rule(rule) {
        Some((env, "down")) => Some((env, "down")),
        Some(_) => None,
        None => match rule {
            "down" | "traffic" => Some((PRODUCTION, "down")),
            "failing" => Some((PRODUCTION, "degraded")),
            _ => None,
        },
    }
}

fn share(ok: i64, total: i64) -> Option<f64> {
    (total > 0).then(|| ok as f64 / total as f64)
}

/// The 95th percentile of `ms`.
fn p95(mut ms: Vec<i64>) -> Option<i64> {
    if ms.is_empty() {
        return None;
    }
    ms.sort_unstable();
    let i = (ms.len() * 95).div_ceil(100).saturating_sub(1);
    ms.get(i.min(ms.len() - 1)).copied()
}

/// The availability of the last `n` days of `days` (by their checks).
fn over_days(days: &[uptime::Day], n: usize) -> Option<f64> {
    let (mut checks, mut failed) = (0, 0);
    for s in days
        .iter()
        .rev()
        .take(n)
        .filter_map(|d| d.stats.as_ref())
        .map(|s: &UptimeDay| (s.checks, s.failed))
    {
        checks += s.0;
        failed += s.1;
    }
    share(checks - failed, checks)
}

/// One component's line, as of `now`.
fn component(
    meta: &Meta,
    project: &str,
    c: &Component,
    open: &[(&str, &'static str, i64)],
    windows: &[Window],
    now: DateTime<Utc>,
) -> Result<PublicComponent, Error> {
    let t = now.timestamp();
    let env = c.environment.as_str();
    let today = now.date_naive();
    let from = today
        .checked_sub_days(chrono::Days::new(u64::try_from(DAYS - 1).unwrap_or(0)))
        .unwrap_or(today);
    let days = uptime::days(meta, project, env, from, today, now)?;
    let raw: Vec<Check> = meta.checks_in(project, env, t - 86_400).map_err(storage)?;
    let counted = crate::maintenance::counted(&raw, windows, env);
    let report = crate::slo::report(meta, project, env, "UTC", now).ok();
    let figure = |label: &str| {
        report
            .as_ref()
            .and_then(|r| r.windows.iter().find(|w| w.label == label))
    };
    let availability = WINDOWS
        .iter()
        .map(|&w| {
            let from_report = figure(w).and_then(|f| f.availability);
            let fallback = || match w {
                "24h" => share(
                    counted.iter().filter(|c| c.1).count() as i64,
                    counted.len() as i64,
                ),
                "7d" => over_days(&days.days, 7),
                "30d" => over_days(&days.days, 30),
                _ => over_days(&days.days, 90),
            };
            Figure {
                window: w,
                availability: from_report.or_else(fallback),
            }
        })
        .collect();
    let response_p95_ms = if c.response_time {
        figure("24h")
            .and_then(|f| f.p95_ms)
            .or_else(|| p95(counted.iter().filter_map(|c| c.3).collect()))
    } else {
        None
    };
    // State: maintenance, then what the open incidents say, then whether
    // it was checked lately.
    let (state, since) = if let Some(w) = running_at(windows, env, t) {
        ("maintenance", Some(w.starts_at))
    } else if let Some(&(_, s, at)) = open
        .iter()
        .filter(|(e, _, _)| *e == env)
        .max_by_key(|(_, s, _)| rank(s))
    {
        (s, Some(at))
    } else if raw.iter().any(|c| c.0 >= t - STALE_SECS) {
        ("operational", None)
    } else {
        ("unknown", None)
    };
    Ok(PublicComponent {
        name: c.name.clone(),
        state,
        since,
        availability,
        days: days
            .days
            .into_iter()
            .map(|d| PublicDay {
                day: d.day,
                state: d.state,
                availability: d.availability,
            })
            .collect(),
        response_p95_ms,
        slo: report.and_then(|r| r.slo).map(|s| PublicSlo {
            target: s.target,
            availability: s.availability,
            budget_seconds: s.budget_seconds,
            left_seconds: s.left_seconds,
        }),
    })
}

/// What the page of `project` (named `name`) shows for `components`, as
/// of `now`: only public fields.
///
/// # Errors
/// When the store cannot be read.
pub fn public(
    meta: &Meta,
    project: &str,
    name: &str,
    components: &[Component],
    now: DateTime<Utc>,
) -> Result<Public, Error> {
    let t = now.timestamp();
    let since_90 = t - DAYS * 86_400;
    let name_of = |env: &str| {
        components
            .iter()
            .find(|c| c.environment == env)
            .map(|c| c.name.clone())
    };
    let shown: HashSet<&str> = components.iter().map(|c| c.environment.as_str()).collect();
    let refs = meta.incident_refs(project, since_90).map_err(storage)?;
    let open: Vec<(&str, &'static str, i64)> = refs
        .iter()
        .filter(|i| i.ended_at.is_none())
        .filter_map(|i| incident_component(&i.rule).map(|(env, state)| (env, state, i.started_at)))
        .collect();
    let windows = meta
        .maintenance_windows(project, since_90, t + UPCOMING_DAYS * 86_400)
        .map_err(storage)?;
    let lines = components
        .iter()
        .map(|c| component(meta, project, c, &open, &windows, now))
        .collect::<Result<Vec<_>, _>>()?;

    // Announcements about a shown component, with public names only.
    let all = announcements::public(meta, project, t)?;
    let followed: HashSet<i64> = all.iter().filter_map(|a| a.fields.incident).collect();
    let list: Vec<PublicAnnouncement> = all
        .iter()
        .filter_map(|a| {
            let names: Vec<String> = a
                .fields
                .components
                .iter()
                .filter_map(|c| name_of(c.as_str()))
                .collect();
            if names.is_empty() {
                return None;
            }
            Some(PublicAnnouncement {
                id: a.id,
                title: a.fields.title.clone(),
                body: a.fields.body.clone(),
                level: a.fields.level.clone(),
                components: names,
                starts_at: a.fields.starts_at,
                ends_at: a.fields.ends_at,
                active: announcements::active(a, t),
                pinned: a.fields.pinned,
                updated_at: a
                    .updates
                    .last()
                    .map_or(a.updated_at, |u| u.at.max(a.updated_at)),
                updates: a
                    .updates
                    .iter()
                    .map(|u| PublicUpdate {
                        at: u.at,
                        state: u.state.clone(),
                        body: u.body.clone(),
                    })
                    .collect(),
            })
        })
        .take(MAX_ANNOUNCEMENTS)
        .collect();
    let notice = list
        .iter()
        .find(|a| a.pinned && a.active && a.level == "information")
        .cloned();

    let maintenance: Vec<PublicMaintenance> = windows
        .iter()
        .filter(|w| w.ends_at > t)
        .filter_map(|w| {
            let names: Vec<String> = components
                .iter()
                .filter(|c| covers(w, &c.environment))
                .map(|c| c.name.clone())
                .collect();
            (!names.is_empty()).then(|| PublicMaintenance {
                starts_at: w.starts_at,
                ends_at: w.ends_at,
                state: state_at(w, t),
                components: names,
            })
        })
        .collect();

    // Detected incidents no announcement follows, outside maintenance.
    let incidents: Vec<PublicIncident> = refs
        .iter()
        .filter(|i| !followed.contains(&i.id))
        .filter_map(|i| {
            let (env, state) = incident_component(&i.rule)?;
            if state != "down" || !shown.contains(env) {
                return None;
            }
            if running_at(&windows, env, i.started_at).is_some() {
                return None;
            }
            Some(PublicIncident {
                component: name_of(env)?,
                started_at: i.started_at,
                ended_at: i.ended_at,
                seconds: (i.ended_at.unwrap_or(t) - i.started_at).max(0),
            })
        })
        .collect();

    let (state, since) = overall(&lines, &list, &incidents);
    Ok(Public {
        name: name.to_owned(),
        state,
        since,
        generated_at: t,
        components: lines,
        notice,
        announcements: list,
        maintenance,
        incidents,
    })
}

/// The page's state: the worst component (another component down is a
/// partial outage, `degraded`; all unknown is `unknown`), raised by an
/// active announcement (`outage` → `down`, `degraded` → `degraded`).
/// Since when: the start of what made it so, or, when everything works,
/// the end of the last incident.
fn overall(
    lines: &[PublicComponent],
    list: &[PublicAnnouncement],
    incidents: &[PublicIncident],
) -> (&'static str, Option<i64>) {
    let mut state = "operational";
    let mut since = None;
    for (k, c) in lines.iter().enumerate() {
        let s = match c.state {
            "down" if k > 0 => "degraded",
            "unknown" => continue,
            s => s,
        };
        if rank(s) > rank(state) {
            state = s;
            since = c.since;
        }
    }
    if !lines.is_empty() && lines.iter().all(|c| c.state == "unknown") {
        state = "unknown";
    }
    for a in list.iter().filter(|a| a.active) {
        let s = match a.level.as_str() {
            "outage" => "down",
            "degraded" => "degraded",
            _ => continue,
        };
        if rank(s) > rank(state) {
            state = s;
            since = Some(a.starts_at);
        }
    }
    if state == "operational" {
        since = incidents.iter().filter_map(|i| i.ended_at).max();
    }
    (state, since)
}

/// The input of `status_page.public`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublicInput {
    /// The slug of `/status/<slug>`.
    pub slug: String,
}

/// `status_page.public`: what the public page of `slug` shows. Anyone may
/// run it; an unknown or disabled slug is [`Error::NotFound`].
pub struct GetPublic;

impl Command for GetPublic {
    const NAME: &'static str = "status_page.public";
    const ACCESS: Access = Access::Public;
    type Input = PublicInput;
    type Output = Public;

    async fn run<H: Host>(host: &H, _: &Caller, i: PublicInput) -> Result<Public, Error> {
        if !valid_slug(&i.slug) {
            return Err(Error::NotFound);
        }
        let meta = host.meta();
        let row = meta
            .status_page_by_slug(&i.slug)
            .map_err(storage)?
            .filter(|r| r.enabled)
            .ok_or(Error::NotFound)?;
        let p = project(host, &row.project).await?;
        let s = settings_of(&row, &monitored(meta, &p.id));
        public(meta, &p.id, &p.name, &s.components, Utc::now())
    }
}

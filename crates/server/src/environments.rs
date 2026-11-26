//! Environments per project (#410, D-034): host rules and aliases applied
//! to every captured batch, the upgrade seed, and the API that reads and
//! changes them (rules, aliases, hidden environments, monitors). The rules and their precedence live in the engine
//! ([`openlens_engine::environments`]); this module stores them, caches
//! them and runs them.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use openlens_engine::environments::{self as env, Resolution, Rules};
use openlens_event::Event;
use serde_json::json;

use crate::{AppState, auth};

/// The project setting holding the rules.
const RULES: &str = "environments";
/// The project setting recording when the upgrade seed ran.
const SEEDED: &str = "environments_seeded";

/// A project's rules, as ingestion uses them.
#[derive(Debug, Default)]
pub(crate) struct ProjectEnv {
    pub(crate) rules: Rules,
    /// The upgrade seed ran for this project: unclassified hosts become
    /// `unknown`. Until then they keep what they sent.
    pub(crate) seeded: bool,
}

fn load(state: &AppState, project: &str) -> ProjectEnv {
    let rules = state.meta.project_setting(project, RULES).ok().flatten();
    let seeded = state
        .meta
        .project_setting(project, SEEDED)
        .ok()
        .flatten()
        .is_some();
    ProjectEnv {
        rules: Rules::parse(rules.as_deref()),
        seeded,
    }
}

/// The project's rules, from the cache (a minute) or the metadata store.
pub(crate) fn current(state: &AppState, project: &str) -> Arc<ProjectEnv> {
    if let Some(p) = state.caches.env_rules.get(&project.to_owned()) {
        return p;
    }
    let p = Arc::new(load(state, project));
    state.caches.env_rules.insert(project.to_owned(), p.clone());
    p
}

/// Store `rules` for `project`; ingestion uses them from the next batch.
pub(crate) fn save(state: &AppState, project: &str, rules: &Rules) -> Result<(), String> {
    let r = state
        .meta
        .set_project_setting(project, RULES, &rules.to_json())
        .map_err(|e| e.to_string());
    state.caches.env_rules.remove(&project.to_owned());
    r
}

/// Apply the project's rules to a captured batch: events of an ignored
/// host are dropped, the others get the environment the rules give them.
/// Starts the upgrade seed the first time a project is seen.
pub(crate) fn apply(state: &AppState, project: &str, events: &mut Vec<Event>) {
    let p = current(state, project);
    if !p.seeded {
        spawn_seed(state, project);
    }
    events.retain_mut(|e| {
        match env::resolve(&p.rules, p.seeded, e.url.as_deref(), &e.environment) {
            Resolution::Keep => true,
            Resolution::Set(name) => {
                e.environment = name;
                true
            }
            Resolution::Drop => false,
        }
    });
}

/// The upgrade seed (D-034), once per project, in the background: every
/// host that sent production events in the last 30 days and that nothing
/// classifies gets a `production` rule, so an upgrade moves no host out of
/// production. A failure is logged and retried ten minutes later.
pub(crate) fn spawn_seed(state: &AppState, project: &str) {
    let key = project.to_owned();
    if state.caches.env_seeding.get(&key).is_some() {
        return;
    }
    state.caches.env_seeding.insert(key.clone(), ());
    let st = state.clone();
    tokio::spawn(async move {
        let before = current(&st, &key);
        match env::seed_rules(&st.query, &key, &before.rules).await {
            Ok(seeded) => {
                let (s, k) = (st.clone(), key.clone());
                let stored = tokio::task::spawn_blocking(move || -> Result<usize, String> {
                    // Rules set while the seed ran are kept: merge into the
                    // stored value, not over it.
                    let mut now = load(&s, &k).rules;
                    let mut added = 0;
                    for r in &seeded.rules {
                        if now.lookup(&r.host).is_none()
                            && env::set_environment_rule(&mut now, &r.host, &r.env).is_ok()
                        {
                            added += 1;
                        }
                    }
                    save(&s, &k, &now)?;
                    s.meta
                        .set_project_setting(&k, SEEDED, &chrono::Utc::now().to_rfc3339())
                        .map_err(|e| e.to_string())?;
                    s.caches.env_rules.remove(&k);
                    Ok(added)
                })
                .await;
                match stored {
                    Ok(Ok(n)) => {
                        st.caches.env_seeding.remove(&key);
                        tracing::info!(project = %key, rules = n, "environments: hosts kept in production")
                    }
                    Ok(Err(err)) => {
                        tracing::warn!(project = %key, %err, "environments: seed not stored")
                    }
                    Err(err) => tracing::warn!(project = %key, %err, "environments: seed crashed"),
                }
            }
            Err(err) => {
                tracing::warn!(project = %key, %err, "environments: seed failed; retried later")
            }
        }
        // On failure the marker stays until it expires (10 minutes), so a
        // failing seed is retried then, not on every captured batch.
    });
}

fn bad(status: StatusCode, msg: &str) -> Response {
    crate::apierr::json(status, msg)
}

/// `GET /api/projects/{id}/environments`: hosts per environment over the
/// last 30 days, the rules, and the hosts still unclassified.
pub(crate) async fn api_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return bad(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let (st, id) = (state.clone(), project.clone());
    let p = match tokio::task::spawn_blocking(move || {
        st.meta.project(&id).map(|o| o.map(|_| current(&st, &id)))
    })
    .await
    {
        Ok(Ok(Some(p))) => p,
        Ok(Ok(None)) => return bad(StatusCode::NOT_FOUND, "unknown project"),
        _ => {
            return bad(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metadata store unavailable",
            );
        }
    };
    if !p.seeded {
        spawn_seed(&state, &project);
    }
    match env::environments(&state.query, &project, &p.rules).await {
        Ok(e) => Json(json!({
            "days": e.days,
            "hosts": e.hosts,
            "rules": e.rules,
            "unclassified": e.unclassified,
            "aliases": e.aliases,
            "hidden": e.hidden,
            "monitors": e.monitors,
            "seeded": p.seeded,
        }))
        .into_response(),
        Err(e) => bad(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// A monitor as sent: the level may be left out (`log`).
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct MonitorChange {
    pub(crate) env: String,
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) level: String,
}

/// A change to the environment settings sent with `PATCH
/// /api/projects/{id}`.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct RuleChange {
    /// Replace every rule.
    pub(crate) environment_rules: Option<Vec<env::Rule>>,
    /// Add or change one rule: `{"host", "env"}`.
    pub(crate) environment_rule: Option<env::Rule>,
    /// Remove the rule for this host.
    pub(crate) remove_environment_rule: Option<String>,
    /// Add or change an alias: `{"from", "to"}`.
    pub(crate) environment_alias: Option<env::Alias>,
    /// Remove the alias of this name.
    pub(crate) remove_environment_alias: Option<String>,
    /// Hide this environment from the selector.
    pub(crate) hide_environment: Option<String>,
    /// Show this hidden environment again.
    pub(crate) show_environment: Option<String>,
    /// Add or change a monitor: `{"env", "url", "level"}`.
    pub(crate) environment_monitor: Option<MonitorChange>,
    /// Remove the monitor of this environment.
    pub(crate) remove_environment_monitor: Option<String>,
}

impl RuleChange {
    pub(crate) fn is_empty(&self) -> bool {
        self.environment_rules.is_none()
            && self.environment_rule.is_none()
            && self.remove_environment_rule.is_none()
            && self.environment_alias.is_none()
            && self.remove_environment_alias.is_none()
            && self.hide_environment.is_none()
            && self.show_environment.is_none()
            && self.environment_monitor.is_none()
            && self.remove_environment_monitor.is_none()
    }
}

/// Apply a [`RuleChange`] to `project` and store it (engine commands
/// `set_environment_rule`, `remove_environment_rule`,
/// `set_environment_alias`, `remove_environment_alias`,
/// `hide_environment`, `set_environment_monitor` and
/// `remove_environment_monitor`). Returns the settings now in force, or
/// what is wrong with the request; nothing is stored on an error.
pub(crate) fn change(
    state: &AppState,
    project: &str,
    c: &RuleChange,
) -> Result<Rules, (StatusCode, String)> {
    let bad_request = |e: &str| (StatusCode::BAD_REQUEST, e.to_owned());
    let mut rules = load(state, project).rules;
    if let Some(all) = &c.environment_rules {
        if all.len() > env::MAX_RULES {
            return Err((
                StatusCode::BAD_REQUEST,
                "a project holds at most 200 host rules".into(),
            ));
        }
        rules = Rules::default();
        for r in all {
            env::set_environment_rule(&mut rules, &r.host, &r.env)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("{}: {e}", r.host)))?;
        }
    }
    if let Some(h) = &c.remove_environment_rule
        && !env::remove_environment_rule(&mut rules, h)
    {
        return Err((StatusCode::NOT_FOUND, format!("no rule for {h}")));
    }
    if let Some(r) = &c.environment_rule {
        env::set_environment_rule(&mut rules, &r.host, &r.env).map_err(bad_request)?;
    }
    if let Some(f) = &c.remove_environment_alias
        && !env::remove_environment_alias(&mut rules, f)
    {
        return Err((StatusCode::NOT_FOUND, format!("no alias for {f}")));
    }
    if let Some(a) = &c.environment_alias {
        env::set_environment_alias(&mut rules, &a.from, &a.to).map_err(bad_request)?;
    }
    if let Some(e) = &c.hide_environment {
        env::hide_environment(&mut rules, e, true).map_err(bad_request)?;
    }
    if let Some(e) = &c.show_environment {
        env::hide_environment(&mut rules, e, false).map_err(bad_request)?;
    }
    if let Some(e) = &c.remove_environment_monitor
        && !env::remove_environment_monitor(&mut rules, e)
    {
        return Err((StatusCode::NOT_FOUND, format!("no monitor for {e}")));
    }
    if let Some(m) = &c.environment_monitor {
        env::set_environment_monitor(&mut rules, &m.env, &m.url, &m.level).map_err(bad_request)?;
    }
    save(state, project, &rules).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(rules)
}

/// The environment settings as the API answers a change.
pub(crate) fn as_json(r: &Rules) -> serde_json::Value {
    json!({
        "environment_rules": r.rules,
        "environment_aliases": r.aliases,
        "hidden_environments": r.hidden,
        "environment_monitors": r.monitors,
    })
}

/// The environments a selector lists for `project` (hidden ones left out,
/// except the one opened), engine command `visible`.
pub(crate) fn visible(
    state: &AppState,
    project: &str,
    seen: Vec<String>,
    selected: &str,
) -> Vec<String> {
    env::visible(&current(state, project).rules, seen, selected)
}

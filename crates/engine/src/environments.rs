//! Environments per project (#410, D-034): host rules applied at
//! ingestion, and the list of environments with the hosts they came from.
//!
//! Precedence, for an event that carries a page address:
//! 1. an environment the app set itself (it differs from what the SDK's
//!    hostname patterns give for that host) wins;
//! 2. otherwise a host rule of the project decides (`ignore` drops it);
//! 3. otherwise a hostname pattern's environment is kept;
//! 4. otherwise the host is `unknown`, not `production`.
//!
//! Events with no page address (server SDKs) keep what they sent.
//!
//! The SDK's fallback for an unmatched host was `production` and is now
//! `unknown`: either value, for a host no pattern matches, counts as
//! detected, so old and new SDKs are read alike.
//!
//! The same setting also holds (#410, parts 3 and 4):
//! - aliases: an environment name the app sends, stored as another
//!   (`live-eu` → `production`), applied last at ingestion;
//! - hidden environments: still stored and queryable, left out of the
//!   environment selector;
//! - monitors: a checked address and an alert level for a non-production
//!   environment (production keeps the project's app address and every
//!   alert).

use openlens_event::{
    DEFAULT_ENVIRONMENT, UNKNOWN_ENVIRONMENT, host_of, normalize_environment, sdk_guess,
};
use openlens_query::QueryEngine;
use serde::{Deserialize, Serialize};

use crate::{Result, int, text};

/// Rules a project holds at most.
pub const MAX_RULES: usize = 200;
/// The longest host name (RFC 1035).
pub const MAX_HOST: usize = 253;
/// The longest environment name.
pub const MAX_ENV: usize = 64;
/// The rule target that drops a host's events at ingestion.
pub const IGNORE: &str = "ignore";
/// Days of events the environment list and the upgrade seed read.
pub const WINDOW_DAYS: i64 = 30;
/// Hosts the environment list returns at most.
const MAX_HOSTS: usize = 500;
/// Aliases a project holds at most.
pub const MAX_ALIASES: usize = 50;
/// Hidden environments a project holds at most.
pub const MAX_HIDDEN: usize = 50;
/// Monitored non-production environments a project holds at most.
pub const MAX_MONITORS: usize = 20;
/// The longest monitored address.
pub const MAX_URL: usize = 200;
/// Alert level: incidents are recorded on Uptime and announced on the
/// project's channels, as production's are.
pub const NOTIFY: &str = "notify";
/// Alert level: incidents are recorded on Uptime, never announced.
pub const LOG: &str = "log";

/// One host rule: an exact host or `*.suffix`, and its environment (or
/// [`IGNORE`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// `app.example.com` or `*.preview.example.com`, lowercase.
    pub host: String,
    /// Environment name, or `ignore`.
    pub env: String,
}

/// An alias: events sent as `from` are stored as `to` (or dropped when
/// `to` is [`IGNORE`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alias {
    /// The name the app sends, normalized.
    pub from: String,
    /// The environment it is stored as, or `ignore`.
    pub to: String,
}

/// A checked address and an alert level for one non-production
/// environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    /// The environment (never `production`: its address is the project's
    /// app address).
    pub env: String,
    /// `https://staging.example.com`, probed every minute (with the
    /// project's health path) and checked for TLS expiry.
    pub url: String,
    /// [`NOTIFY`] or [`LOG`].
    #[serde(default = "default_level")]
    pub level: String,
}

fn default_level() -> String {
    LOG.to_owned()
}

/// A project's environment settings, as stored in its `environments`
/// setting: host rules, aliases, hidden environments and monitors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    /// The rules, in the order they were added.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Aliases, in the order they were added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<Alias>,
    /// Environments left out of the selector.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    /// Monitored non-production environments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<Monitor>,
}

/// A host pattern as stored: lowercase, an exact host or `*.suffix`.
pub fn check_pattern(raw: &str) -> std::result::Result<String, &'static str> {
    let h = raw.trim().to_ascii_lowercase();
    let bare = h.strip_prefix("*.").unwrap_or(&h);
    if bare.is_empty() || h.len() > MAX_HOST {
        return Err("host must be 1-253 characters");
    }
    let ok = bare
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-.:[]_".contains(&b));
    if !ok || bare.starts_with('.') || bare.contains('*') {
        return Err("host must be a host name like app.example.com or *.example.com");
    }
    Ok(h)
}

/// A rule target as stored: a normalized environment name, or `ignore`.
pub fn check_target(raw: &str) -> std::result::Result<String, &'static str> {
    let t = raw.trim().to_ascii_lowercase();
    if t == IGNORE {
        return Ok(t);
    }
    if t.is_empty() {
        // `normalize_environment` reads an empty name as production.
        return Err("environment must be 1-64 characters: a-z, 0-9, - and _");
    }
    let e = normalize_environment(&t);
    if e.is_empty()
        || e.len() > MAX_ENV
        || !e
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
    {
        return Err("environment must be 1-64 characters: a-z, 0-9, - and _");
    }
    if e == UNKNOWN_ENVIRONMENT || e == "bot" {
        return Err("unknown and bot are set by OpenLens, not by a rule");
    }
    Ok(e)
}

/// An environment name as stored (aliases, hidden, monitors): what
/// [`check_target`] accepts, without `ignore`. `unknown` and `bot` are
/// accepted when `system` is true (they can be hidden).
fn check_name(raw: &str, system: bool) -> std::result::Result<String, &'static str> {
    let t = raw.trim().to_ascii_lowercase();
    if system && (t == UNKNOWN_ENVIRONMENT || t == "bot") {
        return Ok(t);
    }
    let e = check_target(&t)?;
    if e == IGNORE {
        return Err("ignore is a rule target, not an environment");
    }
    Ok(e)
}

/// A monitored address as stored: `http(s)://host[:port][/path]`, no
/// spaces, at most [`MAX_URL`] characters, trailing `/` removed.
pub fn check_url(raw: &str) -> std::result::Result<String, &'static str> {
    let u = raw.trim().trim_end_matches('/');
    let ok = (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= MAX_URL
        && !u.contains(|c: char| c.is_whitespace() || c.is_control() || c == '#')
        && host_of(u).is_some();
    if ok {
        Ok(u.to_owned())
    } else {
        Err("address must start with https:// (or http://), 200 characters at most")
    }
}

/// An alert level as stored: [`NOTIFY`] or [`LOG`] (the default).
pub fn check_level(raw: &str) -> std::result::Result<String, &'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | LOG => Ok(LOG.to_owned()),
        NOTIFY => Ok(NOTIFY.to_owned()),
        _ => Err("level must be notify (announce incidents) or log (record them only)"),
    }
}

impl Rules {
    /// The settings stored in a project setting. Entries that do not pass
    /// the checks (an older or hand-edited value) are left out.
    #[must_use]
    pub fn parse(json: Option<&str>) -> Self {
        let mut r: Self = json
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        r.rules.retain(|x| {
            check_pattern(&x.host).is_ok_and(|h| h == x.host)
                && check_target(&x.env).is_ok_and(|e| e == x.env)
        });
        r.rules.truncate(MAX_RULES);
        let stored = std::mem::take(&mut r.aliases);
        for a in stored.into_iter().take(MAX_ALIASES) {
            // Through the command, so a stored chain or loop is refused too.
            if set_environment_alias(&mut r, &a.from, &a.to).is_err() {
                continue;
            }
        }
        r.hidden
            .retain(|h| check_name(h, true).is_ok_and(|n| &n == h && n != DEFAULT_ENVIRONMENT));
        r.hidden.truncate(MAX_HIDDEN);
        r.monitors.retain(|m| {
            check_name(&m.env, false).is_ok_and(|e| e == m.env && e != DEFAULT_ENVIRONMENT)
                && check_url(&m.url).is_ok_and(|u| u == m.url)
                && check_level(&m.level).is_ok_and(|l| l == m.level)
        });
        r.monitors.truncate(MAX_MONITORS);
        r
    }

    /// The environment events sent as `name` are stored as, when an alias
    /// renames it.
    #[must_use]
    pub fn alias(&self, name: &str) -> Option<&str> {
        self.aliases
            .iter()
            .find(|a| a.from == name)
            .map(|a| a.to.as_str())
    }

    /// Is `env` hidden from the selector?
    #[must_use]
    pub fn is_hidden(&self, env: &str) -> bool {
        self.hidden.iter().any(|h| h == env)
    }

    /// The alert level of `env`: production gets every alert; another
    /// environment, its monitor's level, or [`LOG`] without one.
    #[must_use]
    pub fn level(&self, env: &str) -> &str {
        if env == DEFAULT_ENVIRONMENT {
            return NOTIFY;
        }
        self.monitors
            .iter()
            .find(|m| m.env == env)
            .map_or(LOG, |m| m.level.as_str())
    }

    /// The value to store.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"rules\":[]}".into())
    }

    /// The target of the rule for `host`: an exact rule first, then the
    /// longest `*.suffix` that matches.
    #[must_use]
    pub fn lookup(&self, host: &str) -> Option<&str> {
        if let Some(r) = self.rules.iter().find(|r| r.host == host) {
            return Some(&r.env);
        }
        self.rules
            .iter()
            .filter_map(|r| {
                let suffix = r.host.strip_prefix("*.")?;
                let under = host.strip_suffix(suffix)?;
                under
                    .ends_with('.')
                    .then_some((suffix.len(), r.env.as_str()))
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, env)| env)
    }
}

/// Add or change the rule for a host (engine command).
pub fn set_environment_rule(
    rules: &mut Rules,
    host: &str,
    env: &str,
) -> std::result::Result<Rule, &'static str> {
    let rule = Rule {
        host: check_pattern(host)?,
        env: check_target(env)?,
    };
    if let Some(r) = rules.rules.iter_mut().find(|r| r.host == rule.host) {
        r.env.clone_from(&rule.env);
    } else if rules.rules.len() >= MAX_RULES {
        return Err("a project holds at most 200 host rules");
    } else {
        rules.rules.push(rule.clone());
    }
    Ok(rule)
}

/// Remove the rule for a host (engine command); false when there was none.
pub fn remove_environment_rule(rules: &mut Rules, host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    let before = rules.rules.len();
    rules.rules.retain(|r| r.host != h);
    rules.rules.len() < before
}

/// Add or change an alias (engine command `set_environment_alias`):
/// events sent as `from` are stored as `to`, or dropped when `to` is
/// `ignore`. `from` is read after the built-in normalization (`prod` is
/// already `production`); production itself is not aliased; an alias
/// never points to another alias, so one step always ends it.
pub fn set_environment_alias(
    rules: &mut Rules,
    from: &str,
    to: &str,
) -> std::result::Result<Alias, &'static str> {
    let from = check_name(from, false)?;
    let to = check_target(to)?;
    if from == DEFAULT_ENVIRONMENT {
        return Err("production is the default environment: alias another name to it");
    }
    if from == to {
        return Err("that name is already read as this environment");
    }
    if rules.aliases.iter().any(|a| a.to == from) {
        return Err("another alias points to this name: alias to the final name instead");
    }
    if rules.aliases.iter().any(|a| a.from == to) {
        return Err("the target is itself an alias: point to its final name instead");
    }
    let alias = Alias { from, to };
    if let Some(a) = rules.aliases.iter_mut().find(|a| a.from == alias.from) {
        a.to.clone_from(&alias.to);
    } else if rules.aliases.len() >= MAX_ALIASES {
        return Err("a project holds at most 50 aliases");
    } else {
        rules.aliases.push(alias.clone());
    }
    Ok(alias)
}

/// Remove the alias of a name (engine command); false when there was none.
pub fn remove_environment_alias(rules: &mut Rules, from: &str) -> bool {
    let f = normalize_environment(from);
    let before = rules.aliases.len();
    rules.aliases.retain(|a| a.from != f);
    rules.aliases.len() < before
}

/// Hide an environment from the selector, or show it again (engine
/// command `hide_environment`). Its events are still stored and can be
/// read by name. Production, the default view, is never hidden. Returns
/// whether anything changed.
pub fn hide_environment(
    rules: &mut Rules,
    env: &str,
    hidden: bool,
) -> std::result::Result<bool, &'static str> {
    let e = check_name(env, true)?;
    if e == DEFAULT_ENVIRONMENT {
        return Err("production is the default view and cannot be hidden");
    }
    match (hidden, rules.is_hidden(&e)) {
        (true, false) => {
            if rules.hidden.len() >= MAX_HIDDEN {
                return Err("a project hides at most 50 environments");
            }
            rules.hidden.push(e);
            Ok(true)
        }
        (false, true) => {
            rules.hidden.retain(|h| *h != e);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The environments a selector lists: `seen` without the hidden ones,
/// except `selected` (a hidden environment opened by name stays shown).
#[must_use]
pub fn visible(rules: &Rules, seen: Vec<String>, selected: &str) -> Vec<String> {
    seen.into_iter()
        .filter(|e| e == selected || !rules.is_hidden(e))
        .collect()
}

/// Add or change the monitor of a non-production environment (engine
/// command `set_environment_monitor`): its address is probed every minute
/// and its certificate checked, and its incidents are announced
/// ([`NOTIFY`]) or only recorded ([`LOG`]).
pub fn set_environment_monitor(
    rules: &mut Rules,
    env: &str,
    url: &str,
    level: &str,
) -> std::result::Result<Monitor, &'static str> {
    let env = check_name(env, false)?;
    if env == DEFAULT_ENVIRONMENT {
        return Err("production is checked at the project's app address (General settings)");
    }
    let m = Monitor {
        env,
        url: check_url(url)?,
        level: check_level(level)?,
    };
    if let Some(x) = rules.monitors.iter_mut().find(|x| x.env == m.env) {
        *x = m.clone();
    } else if rules.monitors.len() >= MAX_MONITORS {
        return Err("a project monitors at most 20 environments");
    } else {
        rules.monitors.push(m.clone());
    }
    Ok(m)
}

/// Remove an environment's monitor (engine command); false when there was
/// none.
pub fn remove_environment_monitor(rules: &mut Rules, env: &str) -> bool {
    let e = normalize_environment(env);
    let before = rules.monitors.len();
    rules.monitors.retain(|m| m.env != e);
    rules.monitors.len() < before
}

/// The incident rule of a monitored environment's check: `env:<env>:down`,
/// `env:<env>:cert` or `env:<env>:cert_expired`.
#[must_use]
pub fn incident_rule(env: &str, check: &str) -> String {
    format!("env:{env}:{check}")
}

/// The environment and check of an incident rule made by
/// [`incident_rule`]; `None` for production's own rules.
#[must_use]
pub fn split_incident_rule(rule: &str) -> Option<(&str, &str)> {
    rule.strip_prefix("env:")?.rsplit_once(':')
}

/// What ingestion does with one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the environment it has.
    Keep,
    /// Store it under this environment instead.
    Set(String),
    /// Drop it (a host the project ignores).
    Drop,
}

/// The environment of an event with page address `url` that arrived as
/// `received` (already normalized). `seeded` is false until the project's
/// upgrade seed ran: until then an unclassified host stays in production,
/// so an upgrade never moves an existing host out of it. The project's
/// aliases apply last, to whatever the host gave.
#[must_use]
pub fn resolve(rules: &Rules, seeded: bool, url: Option<&str>, received: &str) -> Resolution {
    let by_host = by_host(rules, seeded, url, received);
    if rules.aliases.is_empty() {
        return by_host;
    }
    let name = match &by_host {
        Resolution::Drop => return Resolution::Drop,
        Resolution::Keep => received,
        Resolution::Set(name) => name.as_str(),
    };
    match rules.alias(name) {
        Some(IGNORE) => Resolution::Drop,
        Some(to) => Resolution::Set(to.to_owned()),
        None => by_host,
    }
}

fn by_host(rules: &Rules, seeded: bool, url: Option<&str>, received: &str) -> Resolution {
    let Some(host) = url.and_then(host_of) else {
        return Resolution::Keep;
    };
    let pattern = openlens_event::detected_environment(&host);
    // What an SDK detects for this host: a pattern's environment or, for a
    // host no pattern matches, `production` (older SDKs) or `unknown`.
    let detected =
        received == sdk_guess(&host) || (pattern.is_none() && received == UNKNOWN_ENVIRONMENT);
    if !detected {
        // Set by the app (`init({ environment })`): it wins.
        return Resolution::Keep;
    }
    let keep_or = |env: &str| {
        if env == received {
            Resolution::Keep
        } else {
            Resolution::Set(env.to_owned())
        }
    };
    match rules.lookup(&host) {
        Some(IGNORE) => Resolution::Drop,
        Some(env) => keep_or(env),
        None if pattern.is_some() => Resolution::Keep,
        None if !seeded => keep_or(DEFAULT_ENVIRONMENT),
        None => keep_or(UNKNOWN_ENVIRONMENT),
    }
}

/// One host seen under one environment.
#[derive(Debug, Clone, Serialize)]
pub struct HostRow {
    /// Environment the events were stored under.
    pub environment: String,
    /// Host, or empty for events with no page address (server SDKs).
    pub host: String,
    /// Events in the window.
    pub events: i64,
    /// Last event (UTC).
    pub last_seen: String,
    /// The rule that applies to this host now, if any.
    pub rule: Option<String>,
}

/// The environments of a project: hosts per environment, rules, and the
/// hosts no rule classifies yet.
#[derive(Debug, Clone, Serialize)]
pub struct Environments {
    /// Days read.
    pub days: i64,
    /// Hosts per environment, most events first.
    pub hosts: Vec<HostRow>,
    /// The project's rules.
    pub rules: Vec<Rule>,
    /// Hosts stored as `unknown` in the window and still without a rule.
    pub unclassified: Vec<String>,
    /// The project's aliases.
    pub aliases: Vec<Alias>,
    /// Environments left out of the selector.
    pub hidden: Vec<String>,
    /// Monitored non-production environments.
    pub monitors: Vec<Monitor>,
}

/// The SQL of a page address's host, as [`host_of`] reads it (IPv6
/// literals aside).
const HOST_SQL: &str = "lower(split_part(split_part(split_part(url, '/', 3), '?', 1), ':', 1))";

/// The environments of `project` over the last [`WINDOW_DAYS`] days
/// (engine command `environments`).
pub async fn environments(q: &QueryEngine, project: &str, rules: &Rules) -> Result<Environments> {
    let since = (chrono::Utc::now() - chrono::Duration::days(WINDOW_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let sql = format!(
        "SELECT environment, COALESCE({HOST_SQL}, '') AS host, COUNT(*) AS events, \
         CAST(MAX(timestamp) AS VARCHAR) AS last_seen FROM events \
         WHERE date >= '{since}' GROUP BY 1, 2 ORDER BY events DESC LIMIT {MAX_HOSTS}"
    );
    let r = q.sql(project, &sql).await?;
    let hosts: Vec<HostRow> = r
        .rows
        .iter()
        .map(|row| {
            let host = text(row, "host").unwrap_or_default();
            HostRow {
                environment: text(row, "environment").unwrap_or_default(),
                rule: rules.lookup(&host).map(str::to_owned),
                events: int(row, "events"),
                last_seen: text(row, "last_seen").unwrap_or_default(),
                host,
            }
        })
        .collect();
    let mut unclassified: Vec<String> = hosts
        .iter()
        .filter(|h| h.environment == UNKNOWN_ENVIRONMENT && h.rule.is_none() && !h.host.is_empty())
        .map(|h| h.host.clone())
        .collect();
    unclassified.dedup();
    Ok(Environments {
        days: WINDOW_DAYS,
        hosts,
        rules: rules.rules.clone(),
        unclassified,
        aliases: rules.aliases.clone(),
        hidden: rules.hidden.clone(),
        monitors: rules.monitors.clone(),
    })
}

/// The upgrade seed (D-034): a `production` rule for every host that sent
/// production events in the last [`WINDOW_DAYS`] days and that no rule or
/// hostname pattern classifies, most events first, within [`MAX_RULES`].
pub async fn seed_rules(q: &QueryEngine, project: &str, rules: &Rules) -> Result<Rules> {
    let since = (chrono::Utc::now() - chrono::Duration::days(WINDOW_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let sql = format!(
        "SELECT {HOST_SQL} AS host, COUNT(*) AS events FROM events \
         WHERE date >= '{since}' AND environment = 'production' AND url IS NOT NULL \
         GROUP BY 1 ORDER BY events DESC LIMIT {MAX_RULES}"
    );
    let r = q.sql(project, &sql).await?;
    let mut out = rules.clone();
    for row in &r.rows {
        let Some(host) = text(row, "host") else {
            continue;
        };
        if out.lookup(&host).is_some() || openlens_event::detected_environment(&host).is_some() {
            continue;
        }
        if set_environment_rule(&mut out, &host, DEFAULT_ENVIRONMENT).is_err() {
            // Full, or not a host name: the rest stays unclassified.
            continue;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(list: &[(&str, &str)]) -> Rules {
        let mut r = Rules::default();
        for (h, e) in list {
            assert!(set_environment_rule(&mut r, h, e).is_ok(), "{h} → {e}");
        }
        r
    }

    #[test]
    fn precedence_app_then_rule_then_pattern_then_unknown() {
        let r = rules(&[
            ("kanban-test.philapps.com", "staging"),
            ("*.junk.dev", "ignore"),
        ]);
        let url = |h: &str| format!("https://{h}/board");
        // 1. The app set it: kept, whatever the rule says.
        assert_eq!(
            resolve(&r, true, Some(&url("kanban-test.philapps.com")), "qa"),
            Resolution::Keep
        );
        // 2. Detected (the SDK's fallback): the rule decides.
        assert_eq!(
            resolve(
                &r,
                true,
                Some(&url("kanban-test.philapps.com")),
                "production"
            ),
            Resolution::Set("staging".into())
        );
        assert_eq!(
            resolve(&r, true, Some(&url("a.junk.dev")), "production"),
            Resolution::Drop
        );
        // 3. A hostname pattern, no rule: kept.
        assert_eq!(
            resolve(&r, true, Some(&url("staging.app.com")), "staging"),
            Resolution::Keep
        );
        // 4. Nothing classifies it: unknown, not production.
        assert_eq!(
            resolve(&r, true, Some(&url("other.example.com")), "production"),
            Resolution::Set("unknown".into())
        );
        // Server SDKs (no page address) keep what they sent.
        assert_eq!(resolve(&r, true, None, "production"), Resolution::Keep);
    }

    #[test]
    fn a_new_sdk_sending_unknown_reads_like_an_old_one_sending_production() {
        let r = rules(&[("app.example.com", "staging")]);
        let url = |h: &str| format!("https://{h}/");
        for sent in ["production", "unknown"] {
            assert_eq!(
                resolve(&r, true, Some(&url("app.example.com")), sent),
                Resolution::Set("staging".into()),
                "{sent}"
            );
            assert_eq!(
                resolve(
                    &Rules::default(),
                    false,
                    Some(&url("other.example.com")),
                    sent
                ),
                if sent == "production" {
                    Resolution::Keep
                } else {
                    Resolution::Set("production".into())
                },
                "before the seed, {sent}"
            );
        }
        assert_eq!(
            resolve(&r, true, Some(&url("other.example.com")), "unknown"),
            Resolution::Keep
        );
        // A pattern host sent as unknown was set by the app: kept.
        assert_eq!(
            resolve(&r, true, Some(&url("staging.example.com")), "unknown"),
            Resolution::Keep
        );
    }

    #[test]
    fn aliases_apply_last_and_never_chain() {
        let mut r = rules(&[("box.example.com", "qa")]);
        assert!(set_environment_alias(&mut r, "live-eu", "production").is_ok());
        assert!(set_environment_alias(&mut r, "qa", "staging").is_ok());
        assert!(set_environment_alias(&mut r, "sandbox", "ignore").is_ok());
        // Sent by the app or by a server SDK.
        assert_eq!(
            resolve(&r, true, None, "live-eu"),
            Resolution::Set("production".into())
        );
        assert_eq!(resolve(&r, true, None, "sandbox"), Resolution::Drop);
        // Given by a host rule.
        assert_eq!(
            resolve(&r, true, Some("https://box.example.com/"), "production"),
            Resolution::Set("staging".into())
        );
        // No chain, no loop, production not renamed.
        assert!(set_environment_alias(&mut r, "staging", "preview").is_err());
        assert!(set_environment_alias(&mut r, "old", "qa").is_err());
        assert!(set_environment_alias(&mut r, "prod", "staging").is_err());
        assert!(set_environment_alias(&mut r, "dev", "development").is_err());
        assert!(remove_environment_alias(&mut r, "qa"));
        assert_eq!(resolve(&r, true, None, "qa"), Resolution::Keep);
    }

    #[test]
    fn hidden_environments_leave_the_selector_but_not_the_one_opened() {
        let mut r = Rules::default();
        assert_eq!(hide_environment(&mut r, "qa", true), Ok(true));
        assert!(hide_environment(&mut r, "production", true).is_err());
        let seen = vec![
            "production".to_owned(),
            "qa".to_owned(),
            "staging".to_owned(),
        ];
        assert!(!visible(&r, seen.clone(), "production").contains(&"qa".to_owned()));
        assert!(visible(&r, seen.clone(), "qa").contains(&"qa".to_owned()));
        assert_eq!(hide_environment(&mut r, "qa", false), Ok(true));
        assert_eq!(visible(&r, seen.clone(), "production"), seen);
    }

    #[test]
    fn monitors_give_non_production_environments_their_own_level() {
        let mut r = Rules::default();
        assert_eq!(r.level("production"), NOTIFY);
        assert_eq!(r.level("staging"), LOG);
        assert!(
            set_environment_monitor(&mut r, "staging", "https://stg.example.com/", "notify")
                .is_ok()
        );
        assert_eq!(r.level("staging"), NOTIFY);
        assert_eq!(r.monitors[0].url, "https://stg.example.com");
        assert!(set_environment_monitor(&mut r, "production", "https://x.com", "log").is_err());
        assert!(set_environment_monitor(&mut r, "qa", "ftp://x.com", "log").is_err());
        assert!(set_environment_monitor(&mut r, "qa", "https://x.com", "page").is_err());
        let rule = incident_rule("staging", "down");
        assert_eq!(split_incident_rule(&rule), Some(("staging", "down")));
        assert_eq!(split_incident_rule("down"), None);
        assert_eq!(split_incident_rule("insight:slow_pages:/a"), None);
        let back = Rules::parse(Some(&r.to_json()));
        assert_eq!(back, r);
    }

    #[test]
    fn before_the_upgrade_seed_nothing_leaves_production() {
        let r = Rules::default();
        assert_eq!(
            resolve(&r, false, Some("https://other.example.com/"), "production"),
            Resolution::Keep
        );
    }

    #[test]
    fn an_exact_rule_beats_a_pattern_and_a_longer_suffix_beats_a_shorter_one() {
        let r = rules(&[
            ("*.example.com", "preview"),
            ("*.qa.example.com", "staging"),
            ("app.example.com", "production"),
        ]);
        assert_eq!(r.lookup("app.example.com"), Some("production"));
        assert_eq!(r.lookup("x.qa.example.com"), Some("staging"));
        assert_eq!(r.lookup("x.example.com"), Some("preview"));
        // `*.example.com` covers sub-domains, not the domain or look-alikes.
        assert_eq!(r.lookup("example.com"), None);
        assert_eq!(r.lookup("badexample.com"), None);
    }

    #[test]
    fn rules_are_bounded_and_checked() {
        let mut r = Rules::default();
        for i in 0..MAX_RULES {
            assert!(set_environment_rule(&mut r, &format!("h{i}.example.com"), "staging").is_ok());
        }
        assert!(set_environment_rule(&mut r, "one-more.example.com", "staging").is_err());
        // Changing an existing rule still works when full.
        assert!(set_environment_rule(&mut r, "h1.example.com", "preview").is_ok());
        for bad_host in ["", "*.", "a b.com", "*.*.x.com", &"a".repeat(300)] {
            assert!(check_pattern(bad_host).is_err(), "{bad_host:?}");
        }
        for bad_env in ["", "unknown", "bot", "Pro duction", &"e".repeat(65)] {
            assert!(check_target(bad_env).is_err(), "{bad_env:?}");
        }
        assert_eq!(check_target("prod"), Ok("production".into()));
    }

    #[test]
    fn a_stored_value_that_does_not_pass_the_checks_is_dropped_not_trusted() {
        let r = Rules::parse(Some(
            r#"{"rules":[{"host":"ok.example.com","env":"staging"},{"host":"BAD host","env":"x"},{"host":"x.com","env":"unknown"}]}"#,
        ));
        assert_eq!(r.rules.len(), 1);
        assert_eq!(Rules::parse(Some("not json")), Rules::default());
        let back = Rules::parse(Some(&r.to_json()));
        assert_eq!(back, r);
    }
}

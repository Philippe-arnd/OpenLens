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

use openlens_event::{UNKNOWN_ENVIRONMENT, host_of, normalize_environment, sdk_guess};
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

/// One host rule: an exact host or `*.suffix`, and its environment (or
/// [`IGNORE`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// `app.example.com` or `*.preview.example.com`, lowercase.
    pub host: String,
    /// Environment name, or `ignore`.
    pub env: String,
}

/// A project's host rules, as stored in its `environments` setting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    /// The rules, in the order they were added.
    #[serde(default)]
    pub rules: Vec<Rule>,
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

impl Rules {
    /// The rules stored in a project setting. Rules that do not pass the
    /// checks (an older or hand-edited value) are left out.
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
        r
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
/// upgrade seed ran: until then an unclassified host stays as it came, so
/// an upgrade never moves an existing host out of production.
#[must_use]
pub fn resolve(rules: &Rules, seeded: bool, url: Option<&str>, received: &str) -> Resolution {
    let Some(host) = url.and_then(host_of) else {
        return Resolution::Keep;
    };
    if received != sdk_guess(&host) {
        // Set by the app (`init({ environment })`): it wins.
        return Resolution::Keep;
    }
    match rules.lookup(&host) {
        Some(IGNORE) => Resolution::Drop,
        Some(env) if env == received => Resolution::Keep,
        Some(env) => Resolution::Set(env.to_owned()),
        None if openlens_event::detected_environment(&host).is_some() || !seeded => {
            Resolution::Keep
        }
        None => Resolution::Set(UNKNOWN_ENVIRONMENT.to_owned()),
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
        if set_environment_rule(&mut out, &host, openlens_event::DEFAULT_ENVIRONMENT).is_err() {
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

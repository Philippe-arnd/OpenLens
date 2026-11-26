//! The environments of a project as stored: hosts per environment with
//! their counts, and the upgrade seed that reads them.

use openlens_event::{DEFAULT_ENVIRONMENT, UNKNOWN_ENVIRONMENT};
use openlens_query::QueryEngine;
use serde::Serialize;

use super::{Alias, MAX_RULES, Monitor, Rule, Rules, WINDOW_DAYS, set_environment_rule};
use crate::{Result, int, text};

/// Hosts the environment list returns at most.
const MAX_HOSTS: usize = 500;

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

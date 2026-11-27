//! Delivery metrics (#523, part of #237, D-068): how often a project ships
//! to each environment, how long a change takes to reach its users, how
//! often a change fails and how long it takes to recover, over a period
//! (90 days by default). Definitions in `docs/kpis.md`.
//!
//! A **change** is one of:
//! - with the deploy hook (D-053) in the environment: a succeeded deploy,
//!   or a failed one (a change that never shipped);
//! - else, from traffic (D-049): a release that reached live (95 % of a
//!   day's users) or that was rolled back (it took the majority, then an
//!   older release took it back). It ships on the first day it held the
//!   majority, and is live on the first day it reached 95 %.
//!
//! A change **failed** when its deploy failed, a later deploy reported a
//! rollback, traffic shows its release rolled back, or its release's
//! verdict (#237 part 5) is *regressed*. It is **restored** at the
//! rollback, or at the next shipped change that did not fail.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use openlens_meta::deploys::Deploy;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::deploys::{self, Seen, Status};
use crate::releases::{LIVE_SHARE, PRODUCTION, Release, STAGING, WINDOW_DAYS};
use crate::verdict::Verdict;
use crate::{EngineError, Filters, Result, float, int, text};

/// The default period, in days, and the longest: release traffic is read
/// over [`WINDOW_DAYS`].
pub const DEFAULT_DAYS: i64 = WINDOW_DAYS;
/// (environment, release, day) rows read at most (latest days first).
const MAX_DAILY: usize = 20_000;
/// Why lead time is unknown when no commit range was recorded.
pub const NO_COMMITS: &str = "unknown: no release recorded when its first commit was made (`openlens-cli release --commit` reads it from the git history; with a GitHub link, the hourly read of each recorded range finds it too)";

/// Where an environment's changes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The deploy hook's succeeded and failed deploys.
    Deploys,
    /// Releases reaching live, from the share of daily users.
    Releases,
    /// Nothing shipped in the period.
    None,
}

/// What the other parts of #237 know about releases, when they know it.
#[derive(Debug, Clone, Default)]
pub struct Known {
    /// First commit of each release (Unix seconds), from the commit range
    /// `openlens-cli release` records (#237 part 4).
    pub first_commit: HashMap<String, i64>,
    /// (environment, release) judged *regressed* (#237 part 5).
    pub regressed: HashSet<(String, String)>,
}

/// One change to one environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// When it reached users (deploy time, or its first majority day), or
    /// was attempted, for a failed deploy (Unix seconds).
    pub at: i64,
    /// When it was live (Unix seconds): the end of its lead time.
    pub live_at: i64,
    /// Its release, when known.
    pub release: Option<String>,
    /// It reached users (false for a failed deploy).
    pub shipped: bool,
    /// It failed.
    pub failed: bool,
    /// When a rollback put the previous release back, if one did.
    pub restored_at: Option<i64>,
}

/// The delivery metrics of one environment.
#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    /// Environment.
    pub environment: String,
    /// Where its changes come from.
    pub source: Source,
    /// Changes shipped in the period (deploy frequency).
    pub deploys: usize,
    /// Changes shipped per week.
    pub per_week: f64,
    /// Median seconds from a release's first commit to live.
    pub lead_time_secs: Option<i64>,
    /// Releases the lead time is the median of.
    pub lead_time_samples: usize,
    /// Why there is no lead time, when there is none.
    pub lead_time_note: Option<String>,
    /// Changes in the period: shipped, or failed deploys.
    pub changes: usize,
    /// Changes that failed.
    pub failures: usize,
    /// `failures / changes`; none with no change.
    pub change_failure_rate: Option<f64>,
    /// Median seconds from a failed change to its restore.
    pub restore_secs: Option<i64>,
    /// Failures restored.
    pub restored: usize,
    /// Failures not restored yet.
    pub unrestored: usize,
}

/// The delivery metrics of a project.
#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    /// Days of the period.
    pub days: i64,
    /// Time zone of the days.
    pub timezone: String,
    /// Production first, then staging, then the others by name.
    pub environments: Vec<Metrics>,
}

/// Users of one release on one day of one environment, with its first
/// event that day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    /// Day (`YYYY-MM-DD`).
    pub day: String,
    /// Release.
    pub release: String,
    /// Distinct users.
    pub users: i64,
    /// First event that day (Unix seconds).
    pub first: i64,
}

/// The first event of each release in `daily`.
fn firsts(daily: &[Day]) -> HashMap<&str, i64> {
    let mut first: HashMap<&str, i64> = HashMap::new();
    for d in daily.iter().filter(|d| d.users > 0) {
        let e = first.entry(d.release.as_str()).or_insert(d.first);
        *e = (*e).min(d.first);
    }
    first
}

/// The changes of one environment from its traffic: each release that
/// reached live or was rolled back, oldest first.
#[must_use]
pub fn from_traffic(daily: &[Day]) -> Vec<Change> {
    let first = firsts(daily);
    let rank = |r: &str| (first.get(r).copied().unwrap_or(i64::MAX), r.to_owned());
    let mut days: BTreeMap<&str, Vec<&Day>> = BTreeMap::new();
    for d in daily.iter().filter(|d| d.users > 0) {
        days.entry(d.day.as_str()).or_default().push(d);
    }
    // Per release: first majority, first live, rollback.
    #[derive(Default)]
    struct Track {
        major: Option<i64>,
        live: Option<i64>,
        rolled_back: Option<i64>,
    }
    let mut track: HashMap<&str, Track> = HashMap::new();
    let mut holder: Option<&str> = None;
    for rows in days.values() {
        let total = rows.iter().map(|d| d.users).sum::<i64>().max(1);
        for d in rows {
            let r = d.release.as_str();
            let share = d.users as f64 / total as f64;
            if share > 0.5 {
                if let Some(h) = holder.filter(|h| *h != r)
                    && rank(r) < rank(h)
                    && let Some(old) = track.get_mut(h)
                    && old.rolled_back.is_none()
                {
                    old.rolled_back = Some(d.first);
                }
                let t = track.entry(r).or_default();
                t.major = t.major.or(Some(d.first));
                holder = Some(r);
            }
            if share >= LIVE_SHARE {
                let t = track.entry(r).or_default();
                t.live = t.live.or(Some(d.first));
            }
        }
    }
    let mut out: Vec<Change> = track
        .into_iter()
        .filter(|(_, t)| t.live.is_some() || t.rolled_back.is_some())
        .filter_map(|(r, t)| {
            let at = t.major.or(t.live)?;
            Some(Change {
                at,
                live_at: t.live.unwrap_or(at),
                release: Some(r.to_owned()),
                shipped: true,
                failed: t.rolled_back.is_some(),
                restored_at: t.rolled_back,
            })
        })
        .collect();
    out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.release.cmp(&b.release)));
    out
}

/// The changes of one environment from its deploys (oldest first, each
/// with the release it is tied to): each succeeded deploy, and each
/// failed one. A rolled-back deploy marks the latest succeeded one before
/// it failed, restored then.
#[must_use]
pub fn from_deploys(deploys: &[(&Deploy, Option<String>)]) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    for (d, release) in deploys {
        match Status::parse(&d.status) {
            // A deploy not tied to traffic yet keeps the release it names
            // (a release recorded before its first user, #633).
            Some(Status::Succeeded) => out.push(Change {
                at: d.at,
                live_at: d.at,
                release: release.clone().or_else(|| d.release.clone()),
                shipped: true,
                failed: false,
                restored_at: None,
            }),
            Some(Status::Failed) => out.push(Change {
                at: d.at,
                live_at: d.at,
                release: release.clone().or_else(|| d.release.clone()),
                shipped: false,
                failed: true,
                restored_at: None,
            }),
            Some(Status::RolledBack) => {
                if let Some(c) = out
                    .iter_mut()
                    .rev()
                    .find(|c| c.shipped && c.restored_at.is_none())
                {
                    c.failed = true;
                    c.restored_at = Some(d.at);
                }
            }
            Some(Status::Started) | None => {}
        }
    }
    out
}

fn median(mut v: Vec<i64>) -> Option<i64> {
    v.sort_unstable();
    let mid = v.len() / 2;
    let upper = *v.get(mid)?;
    if mid * 2 == v.len() {
        // An even count: the mean of the two middle values.
        Some((*v.get(mid.checked_sub(1)?)? + upper) / 2)
    } else {
        Some(upper)
    }
}

/// The metrics of one environment from its changes (oldest first), those
/// at or after `since` counted, over a period of `days`.
#[must_use]
pub fn metrics(
    environment: &str,
    source: Source,
    changes: &[Change],
    since: i64,
    days: i64,
    known: &Known,
) -> Metrics {
    let counted: Vec<&Change> = changes.iter().filter(|c| c.at >= since).collect();
    let deploys = counted.iter().filter(|c| c.shipped).count();
    let failures: Vec<&Change> = counted.iter().copied().filter(|c| c.failed).collect();
    let mut restores = Vec::new();
    for f in &failures {
        let next = changes
            .iter()
            .filter(|c| c.at > f.at && c.shipped && !c.failed)
            .map(|c| c.at)
            .min();
        if let Some(t) = [f.restored_at, next].into_iter().flatten().min() {
            restores.push((t - f.at).max(0));
        }
    }
    let leads: Vec<i64> = counted
        .iter()
        .filter(|c| c.shipped)
        .filter_map(|c| {
            let t = *known.first_commit.get(c.release.as_deref()?)?;
            (t <= c.live_at).then_some(c.live_at - t)
        })
        .collect();
    let lead_time_note = if !leads.is_empty() {
        None
    } else if known.first_commit.is_empty() {
        Some(NO_COMMITS.to_owned())
    } else {
        Some("unknown: no release with a recorded commit range shipped in the period".to_owned())
    };
    let (n, failed) = (counted.len(), failures.len());
    Metrics {
        environment: environment.to_owned(),
        source,
        deploys,
        per_week: deploys as f64 * 7.0 / days.max(1) as f64,
        lead_time_samples: leads.len(),
        lead_time_secs: median(leads),
        lead_time_note,
        changes: n,
        failures: failed,
        change_failure_rate: (n > 0).then(|| failed as f64 / n as f64),
        restored: restores.len(),
        unrestored: failed.saturating_sub(restores.len()),
        restore_secs: median(restores),
    }
}

/// The (environment, release) pairs judged *regressed* among `releases`,
/// as `releases.list` judged them (one environment at a time: the others
/// have no verdict).
#[must_use]
pub fn regressed(releases: &[Release]) -> HashSet<(String, String)> {
    releases
        .iter()
        .filter(|r| r.verdict == Some(Verdict::Regressed))
        .map(|r| (r.environment.clone(), r.release.clone()))
        .collect()
}

/// Mark the changes of `env` whose release was judged regressed as failed.
fn with_verdicts(changes: &mut [Change], env: &str, known: &Known) {
    for c in changes.iter_mut() {
        if let Some(r) = &c.release
            && known.regressed.contains(&(env.to_owned(), r.clone()))
        {
            c.failed = true;
        }
    }
}

fn env_rank(e: &str) -> (u8, &str) {
    match e {
        PRODUCTION => (0, e),
        STAGING => (1, e),
        _ => (2, e),
    }
}

/// The delivery metrics of `project` over the last `days` days (1 to
/// [`WINDOW_DAYS`]), days counted in `tz`, in every environment or in
/// `environment` only (engine command `releases.delivery`). `deploys` are
/// the hook's ([`openlens_meta::Meta::deploys`]), oldest first; `known`
/// what the commit ranges and verdicts say.
///
/// # Errors
/// An invalid time zone or environment, or a failed query.
pub async fn delivery(
    q: &QueryEngine,
    project: &str,
    tz: &str,
    environment: Option<&str>,
    days: i64,
    deploys: &[Deploy],
    known: &Known,
) -> Result<Delivery> {
    let days = days.clamp(1, WINDOW_DAYS);
    let f = Filters {
        tz: tz.to_owned(),
        ..Filters::default()
    };
    let day = f.day()?;
    let mut params = openlens_sqlgen::Params::new();
    let mut w = format!(
        "date >= '{}' AND timestamp >= now() - INTERVAL '{WINDOW_DAYS} days' \
         AND release IS NOT NULL AND release <> '' AND environment <> 'bot' \
         AND distinct_id <> '$server'",
        (Utc::now() - chrono::Duration::days(WINDOW_DAYS + 1)).format("%Y-%m-%d")
    );
    if let Some(e) = environment {
        let p = params
            .set("env", e)
            .map_err(|_| EngineError::InvalidFilter("environment"))?;
        w.push_str(&format!(" AND environment = {p}"));
    }
    let sql = format!(
        "SELECT environment, release, {day} AS day, count(DISTINCT distinct_id) AS users, \
         date_part('epoch', min(timestamp)) AS first \
         FROM events WHERE {w} GROUP BY 1, 2, 3 ORDER BY day DESC LIMIT {MAX_DAILY}"
    );
    let rows = q.with_params(&params).sql(project, &sql).await?;
    let mut per_env: HashMap<String, Vec<Day>> = HashMap::new();
    for row in &rows.rows {
        per_env
            .entry(text(row, "environment").unwrap_or_default())
            .or_default()
            .push(Day {
                day: text(row, "day").unwrap_or_default(),
                release: text(row, "release").unwrap_or_default(),
                users: int(row, "users"),
                first: float(row, "first") as i64,
            });
    }
    let since = Utc::now().timestamp() - days * 86_400;
    Ok(assemble(
        &per_env,
        deploys,
        environment,
        (since, days),
        tz,
        known,
    ))
}

/// The metrics of every environment with traffic or deploys; `period` is
/// its start (Unix seconds) and its length in days.
fn assemble(
    per_env: &HashMap<String, Vec<Day>>,
    deploys: &[Deploy],
    environment: Option<&str>,
    period: (i64, i64),
    tz: &str,
    known: &Known,
) -> Delivery {
    let (since, days) = period;
    let ds: Vec<Deploy> = deploys
        .iter()
        .filter(|d| environment.is_none_or(|e| d.environment == e))
        .cloned()
        .collect();
    // The releases seen, to tie deploys to them as `releases.list` does.
    let seen: Vec<Seen> = per_env
        .iter()
        .flat_map(|(env, rows)| {
            firsts(rows).into_iter().map(move |(r, t)| Seen {
                environment: env.clone(),
                release: r.to_owned(),
                first: t,
            })
        })
        .collect();
    let tied = deploys::link(&ds, &seen);
    let mut envs: Vec<String> = per_env.keys().cloned().collect();
    envs.extend(ds.iter().map(|d| d.environment.clone()));
    envs.sort_by(|a, b| env_rank(a).cmp(&env_rank(b)));
    envs.dedup();
    let mut out = Vec::with_capacity(envs.len());
    for env in envs {
        let traffic = from_traffic(per_env.get(&env).map(Vec::as_slice).unwrap_or_default());
        let mine: Vec<(&Deploy, Option<String>)> = ds
            .iter()
            .zip(&tied)
            .filter(|(d, _)| d.environment == env)
            .map(|(d, t)| (d, t.clone()))
            .collect();
        let hooked = mine.iter().any(|(d, _)| {
            d.at >= since
                && matches!(
                    Status::parse(&d.status),
                    Some(Status::Succeeded | Status::Failed)
                )
        });
        let (source, mut changes) = if hooked {
            let mut cs = from_deploys(&mine);
            // A release traffic shows rolled back fails its deploys.
            for c in &mut cs {
                let back = c.release.as_ref().and_then(|r| {
                    traffic
                        .iter()
                        .find(|t| t.release.as_ref() == Some(r) && t.failed)
                });
                if let Some(t) = back {
                    c.failed = true;
                    c.restored_at = c.restored_at.or(t.restored_at);
                }
            }
            (Source::Deploys, cs)
        } else if traffic.iter().any(|c| c.at >= since) {
            (Source::Releases, traffic)
        } else {
            (Source::None, Vec::new())
        };
        with_verdicts(&mut changes, &env, known);
        out.push(metrics(&env, source, &changes, since, days, known));
    }
    Delivery {
        days,
        timezone: tz.to_owned(),
        environments: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Days `1..` of `(release, users)`, a day apart.
    fn days(rows: &[&[(&str, i64)]]) -> Vec<Day> {
        rows.iter()
            .enumerate()
            .flat_map(|(i, rs)| {
                rs.iter().map(move |(r, n)| Day {
                    day: format!("2026-11-{:02}", i + 1),
                    release: (*r).to_owned(),
                    users: *n,
                    first: i as i64 * 86_400,
                })
            })
            .collect()
    }

    fn dep(release: &str, status: Status, at: i64) -> Deploy {
        Deploy {
            environment: PRODUCTION.into(),
            release: Some(release.into()),
            commit: None,
            status: status.as_str().into(),
            at,
            source: "plain".into(),
        }
    }

    #[test]
    fn traffic_counts_releases_reaching_live_and_rollbacks_as_failures() {
        let d = days(&[
            &[("v1", 100)],
            &[("v1", 10), ("v2", 90)],
            &[("v1", 99), ("v2", 1)],
            &[("v1", 2), ("v3", 98)],
        ]);
        let cs = from_traffic(&d);
        let names: Vec<_> = cs.iter().filter_map(|c| c.release.as_deref()).collect();
        assert_eq!(names, ["v1", "v2", "v3"]);
        let v2 = &cs[1];
        assert!(v2.failed, "an older release took the majority back");
        assert!(v2.restored_at > Some(v2.at));
        let m = metrics(PRODUCTION, Source::Releases, &cs, 0, 7, &Known::default());
        assert_eq!((m.deploys, m.failures, m.restored), (3, 1, 1));
        assert!(m.change_failure_rate > Some(0.0) && m.change_failure_rate < Some(1.0));
        assert!(m.lead_time_secs.is_none());
        assert!(m.lead_time_note.is_some(), "an unknown lead time says why");
    }

    #[test]
    fn a_canary_that_never_took_the_users_is_not_a_change() {
        let d = days(&[&[("v1", 100)], &[("v1", 95), ("v2", 5)]]);
        let cs = from_traffic(&d);
        assert!(cs.iter().all(|c| c.release.as_deref() == Some("v1")));
    }

    #[test]
    fn deploys_fail_by_status_rollback_or_verdict_and_are_restored_after() {
        let ds = [
            dep("v1", Status::Started, 0),
            dep("v1", Status::Succeeded, 100),
            dep("v2", Status::Failed, 1_000),
            dep("v3", Status::Succeeded, 2_000),
            dep("v3", Status::RolledBack, 2_500),
            dep("v4", Status::Succeeded, 5_000),
            dep("v5", Status::Succeeded, 9_000),
        ];
        let tied: Vec<(&Deploy, Option<String>)> =
            ds.iter().map(|d| (d, d.release.clone())).collect();
        let mut cs = from_deploys(&tied);
        let mut known = Known::default();
        known.regressed.insert((PRODUCTION.into(), "v4".into()));
        known.first_commit.insert("v5".into(), 8_000);
        with_verdicts(&mut cs, PRODUCTION, &known);
        let m = metrics(PRODUCTION, Source::Deploys, &cs, 0, 7, &known);
        assert_eq!(m.deploys, 4, "succeeded deploys only");
        assert_eq!(m.changes, 5, "a failed deploy is a change too");
        assert_eq!(m.failures, 3, "failed, rolled back, regressed");
        assert_eq!((m.restored, m.unrestored), (3, 0));
        assert_eq!(m.lead_time_samples, 1);
        assert!(m.lead_time_secs > Some(0));
        assert!(m.lead_time_note.is_none());
    }

    fn judged(env: &str, release: &str, verdict: Option<Verdict>) -> Release {
        Release {
            release: release.into(),
            environment: env.into(),
            state: crate::releases::State::Superseded,
            share: 0.0,
            first_seen: String::new(),
            last_seen: String::new(),
            users: 1,
            events: 1,
            staging_secs: None,
            staged_as: None,
            deployed_at: None,
            deploy: None,
            verdict,
            reasons: Vec::new(),
            runtime: None,
        }
    }

    /// #611: v1 and v2 reached live and both were judged regressed by
    /// `releases.list`; Delivery showed 0 failed of 2.
    #[test]
    fn releases_judged_regressed_fail_their_changes() {
        let mut per_env = HashMap::new();
        per_env.insert(
            PRODUCTION.to_owned(),
            days(&[&[("v1", 100)], &[("v1", 1), ("v2", 99)]]),
        );
        let listed = [
            judged(PRODUCTION, "v1", Some(Verdict::Regressed)),
            judged(PRODUCTION, "v2", Some(Verdict::Regressed)),
            // Another environment's verdict does not fail production's
            // change of the same name.
            judged(STAGING, "v3", Some(Verdict::Regressed)),
            judged(PRODUCTION, "v3", Some(Verdict::Healthy)),
        ];
        let known = Known {
            regressed: regressed(&listed),
            ..Known::default()
        };
        let before = assemble(&per_env, &[], None, (0, 7), "UTC", &Known::default());
        let after = assemble(&per_env, &[], None, (0, 7), "UTC", &known);
        let (b, a) = (&before.environments[0], &after.environments[0]);
        assert!(a.changes > 0);
        assert_eq!(a.changes, b.changes, "a verdict adds no change");
        assert!(a.failures > b.failures, "regressed releases fail");
        assert_eq!(a.failures, a.changes, "every change here was regressed");
        assert!(a.change_failure_rate > b.change_failure_rate);
        assert!(
            !known
                .regressed
                .contains(&(PRODUCTION.to_owned(), "v3".to_owned()))
        );
    }

    #[test]
    fn the_hook_wins_over_traffic_and_a_traffic_rollback_fails_its_deploy() {
        let mut per_env = HashMap::new();
        per_env.insert(
            PRODUCTION.to_owned(),
            days(&[&[("v1", 100)], &[("v1", 10), ("v2", 90)], &[("v1", 99)]]),
        );
        per_env.insert(STAGING.to_owned(), days(&[&[("v2", 10)]]));
        let ds = [
            dep("v1", Status::Succeeded, 0),
            dep("v2", Status::Succeeded, 80_000),
        ];
        let d = assemble(&per_env, &ds, None, (0, 7), "UTC", &Known::default());
        let envs: Vec<_> = d
            .environments
            .iter()
            .map(|m| m.environment.as_str())
            .collect();
        assert_eq!(envs, [PRODUCTION, STAGING]);
        let prod = &d.environments[0];
        assert_eq!(prod.source, Source::Deploys);
        assert_eq!((prod.deploys, prod.failures, prod.restored), (2, 1, 1));
        assert_eq!(d.environments[1].source, Source::Releases);
    }
}

//! Release lifecycle (#500, part of #237, D-049): where each release of a
//! project stands in each environment, from its share of the environment's
//! daily users, and how long it spent in staging before production.
//!
//! - **Share**: a release's distinct users on a day (the project's day)
//!   over the sum of every release's distinct users that day in the same
//!   environment. Events with no release and bots are left out.
//! - **Majority**: more than half of a day's share.
//! - **Older / newer**: by first event in the environment.
//!
//! The state of a release, read on the environment's latest day with data:
//! after the last day it held the majority (or since it appeared, if it
//! never did), the last release to hold the majority decides:
//! - an older one: **rolled back** (it had the majority, an older one took
//!   it back);
//! - a newer one: **superseded**;
//! - none: **live** when its share on the latest day is at least 95 %, or
//!   when it reached 95 % before and still holds the majority (a canary of
//!   the next release does not demote it); otherwise **rolling out**.
//!
//! Deploys reported by the deploy hook (#504, D-053) add each release's
//! deploy time and last deploy status; a deploy tied to no release with
//! traffic is listed on its own: **failed**, **rolled back**, or
//! **deploying** (started or succeeded, no traffic yet).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use openlens_meta::deploys::Deploy;
use openlens_meta::releases::ReleaseBuild;
use openlens_query::QueryEngine;
use serde::{Deserialize, Serialize};

use crate::deploys::{self, DeployRow, Seen, Status};
use crate::{EngineError, Filters, Result, float, int, text};

/// Days read.
pub const WINDOW_DAYS: i64 = 90;
/// Share of the latest day's users from which a release is live.
pub const LIVE_SHARE: f64 = 0.95;
/// The environment whose builds count as time in staging.
pub const STAGING: &str = "staging";
/// The environment releases are promoted to.
pub const PRODUCTION: &str = openlens_event::DEFAULT_ENVIRONMENT;
/// Releases returned at most (newest last events first).
const MAX_RELEASES: usize = 500;
/// (environment, release, day) rows read at most (latest days first).
const MAX_DAILY: usize = 20_000;

/// Where a release stands in an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Below 95 % and not overtaken: still gaining (or a canary).
    RollingOut,
    /// At least 95 % of the latest day's users.
    Live,
    /// A newer release took the majority.
    Superseded,
    /// It had the majority, then an older release took it back; or, with
    /// no traffic, the deploy hook said it was rolled back.
    RolledBack,
    /// No traffic, and its last deploy failed (deploy hook).
    Failed,
    /// No traffic yet, deploying or deployed (deploy hook).
    Deploying,
}

impl State {
    /// As shown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::RollingOut => "rolling out",
            Self::Live => "live",
            Self::Superseded => "superseded",
            Self::RolledBack => "rolled back",
            Self::Failed => "failed",
            Self::Deploying => "deploying",
        }
    }
}

/// One release in one environment.
#[derive(Debug, Clone, Serialize)]
pub struct Release {
    /// Release name.
    pub release: String,
    /// Environment.
    pub environment: String,
    /// Where it stands.
    pub state: State,
    /// Its share of the environment's users on the latest day with data
    /// (0 to 1).
    pub share: f64,
    /// First event in the window (`YYYY-MM-DD HH:MM`, in `timezone`).
    pub first_seen: String,
    /// Last event in the window.
    pub last_seen: String,
    /// Distinct users in the window.
    pub users: i64,
    /// Events in the window.
    pub events: i64,
    /// Production only: seconds between its first event in staging (same
    /// name, or the same build fingerprint) and its first in production.
    pub staging_secs: Option<i64>,
    /// The name it had in staging, when it was staged.
    pub staged_as: Option<String>,
    /// Its latest succeeded deploy in the environment (deploy hook).
    pub deployed_at: Option<String>,
    /// The status of its latest deploy in the environment (deploy hook).
    pub deploy: Option<Status>,
    /// Healthy, regressed or inconclusive against the release before it
    /// (#522, D-067); none until it has enough traffic or is a day old,
    /// and for the environments not judged (`releases.list` judges
    /// production, or the one environment asked for).
    pub verdict: Option<crate::verdict::Verdict>,
    /// Why, in plain words (also why there is no verdict yet).
    pub reasons: Vec<String>,
}

/// The releases of a project.
#[derive(Debug, Clone, Serialize)]
pub struct Releases {
    /// Days read.
    pub days: i64,
    /// Time zone of the days and times.
    pub timezone: String,
    /// What a share counts.
    pub measure: &'static str,
    /// Production first, then staging, then the others by name; newest
    /// first within one.
    pub releases: Vec<Release>,
    /// Deploys reported by the deploy hook in the window, newest first.
    pub deploys: Vec<DeployRow>,
    /// When the project's deploy hook URL was made, if it has one.
    pub deploy_hook: Option<String>,
    /// The newest judged release's verdict by layer, with the routes that
    /// got worse (#623): set by `releases.list`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<crate::verdict_backend::LatestVerdict>,
}

/// Users of one release on one day of one environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Daily {
    /// Day (`YYYY-MM-DD`).
    pub day: String,
    /// Release.
    pub release: String,
    /// Distinct users.
    pub users: i64,
}

/// The state and latest-day share of each release of one environment.
/// `order` lists its releases oldest first; one missing from it counts as
/// the newest.
#[must_use]
pub fn states(daily: &[Daily], order: &[String]) -> HashMap<String, (State, f64)> {
    let rank = |r: &str| order.iter().position(|o| o == r).unwrap_or(order.len());
    // Per day: users per release, and the day's total.
    let mut days: BTreeMap<&str, Vec<(&str, i64)>> = BTreeMap::new();
    for d in daily.iter().filter(|d| d.users > 0) {
        days.entry(d.day.as_str())
            .or_default()
            .push((d.release.as_str(), d.users));
    }
    let shares: Vec<(&str, Vec<(&str, f64)>)> = days
        .into_iter()
        .map(|(day, rs)| {
            let total: i64 = rs.iter().map(|r| r.1).sum();
            let s = rs
                .into_iter()
                .map(|(r, n)| (r, n as f64 / total.max(1) as f64))
                .collect();
            (day, s)
        })
        .collect();
    let holder = |i: usize| -> Option<&str> {
        shares
            .get(i)
            .and_then(|(_, s)| s.iter().find(|(_, x)| *x > 0.5).map(|(r, _)| *r))
    };
    let share_on = |i: usize, r: &str| -> f64 {
        shares
            .get(i)
            .and_then(|(_, s)| s.iter().find(|(x, _)| *x == r).map(|(_, v)| *v))
            .unwrap_or(0.0)
    };
    let last = shares.len().saturating_sub(1);
    let mut out = HashMap::new();
    for d in daily {
        let r = d.release.as_str();
        if out.contains_key(r) {
            continue;
        }
        let seen: Vec<usize> = (0..shares.len())
            .filter(|&i| share_on(i, r) > 0.0)
            .collect();
        let Some(&first) = seen.first() else {
            continue;
        };
        let last_major = (0..shares.len()).rev().find(|&i| holder(i) == Some(r));
        let from = last_major.map_or(first, |i| i + 1);
        let taker = (from..shares.len())
            .rev()
            .find_map(|i| holder(i).filter(|h| *h != r));
        let now = share_on(last, r);
        let state = match taker {
            Some(h) if rank(h) > rank(r) => State::Superseded,
            Some(_) if last_major.is_some() => State::RolledBack,
            _ if now >= LIVE_SHARE => State::Live,
            _ if holder(last) == Some(r) && seen.iter().any(|&i| share_on(i, r) >= LIVE_SHARE) => {
                State::Live
            }
            _ => State::RollingOut,
        };
        out.insert(r.to_owned(), (state, now));
    }
    out
}

/// The production releases' time in staging: per production release, the
/// earliest staging first event of the same name or of a name its build
/// fingerprints had in staging, when before its first production event.
fn staging(
    prod: &[(String, i64)],
    staged: &HashMap<String, i64>,
    builds: &[ReleaseBuild],
) -> HashMap<String, (i64, String)> {
    let mut out = HashMap::new();
    for (name, at) in prod {
        let mut names = vec![name.clone()];
        for b in builds
            .iter()
            .filter(|b| b.environment == PRODUCTION && &b.name == name)
        {
            names.extend(
                builds
                    .iter()
                    .filter(|s| s.environment == STAGING && s.build == b.build)
                    .map(|s| s.name.clone()),
            );
        }
        let best = names
            .into_iter()
            .filter_map(|n| staged.get(&n).map(|t| (*t, n)))
            .min();
        if let Some((t, n)) = best.filter(|(t, _)| t < at) {
            out.insert(name.clone(), (at - t, n));
        }
    }
    out
}

pub(crate) fn local(secs: f64, tz: &str) -> String {
    let tz = tz.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC);
    DateTime::<Utc>::from_timestamp(secs as i64, 0)
        .map(|t| t.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn env_rank(e: &str) -> (u8, &str) {
    match e {
        PRODUCTION => (0, e),
        STAGING => (1, e),
        _ => (2, e),
    }
}

/// The releases of `project` over the last [`WINDOW_DAYS`] days, days
/// counted in `tz`, in every environment or in `environment` only
/// (engine command `releases.list`). `builds` are the project's build
/// fingerprints ([`openlens_meta::Meta::release_builds`]); `deploys` the
/// deploys the hook recorded ([`openlens_meta::Meta::deploys`]), oldest
/// first.
///
/// # Errors
/// An invalid time zone or environment, or a failed query.
pub async fn releases(
    q: &QueryEngine,
    project: &str,
    tz: &str,
    environment: Option<&str>,
    builds: &[ReleaseBuild],
    deploys: &[Deploy],
) -> Result<Releases> {
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
    let q = q.with_params(&params);
    let totals = format!(
        "SELECT environment, release, count(DISTINCT distinct_id) AS users, count(*) AS events, \
         date_part('epoch', min(timestamp)) AS first, date_part('epoch', max(timestamp)) AS last \
         FROM events WHERE {w} GROUP BY 1, 2 ORDER BY last DESC LIMIT {MAX_RELEASES}"
    );
    let daily = format!(
        "SELECT environment, release, {day} AS day, count(DISTINCT distinct_id) AS users \
         FROM events WHERE {w} GROUP BY 1, 2, 3 ORDER BY day DESC LIMIT {MAX_DAILY}"
    );
    let (totals, daily) = futures::join!(q.sql(project, &totals), q.sql(project, &daily));
    let (totals, daily) = (totals?, daily?);

    let mut per_env: HashMap<String, Vec<Daily>> = HashMap::new();
    for row in &daily.rows {
        per_env
            .entry(text(row, "environment").unwrap_or_default())
            .or_default()
            .push(Daily {
                day: text(row, "day").unwrap_or_default(),
                release: text(row, "release").unwrap_or_default(),
                users: int(row, "users"),
            });
    }
    struct Total {
        environment: String,
        release: String,
        users: i64,
        events: i64,
        first: f64,
        last: f64,
    }
    let totals: Vec<Total> = totals
        .rows
        .iter()
        .map(|row| Total {
            environment: text(row, "environment").unwrap_or_default(),
            release: text(row, "release").unwrap_or_default(),
            users: int(row, "users"),
            events: int(row, "events"),
            first: float(row, "first"),
            last: float(row, "last"),
        })
        .collect();
    // Oldest first, per environment.
    let mut order: HashMap<&str, Vec<(f64, String)>> = HashMap::new();
    for t in &totals {
        order
            .entry(t.environment.as_str())
            .or_default()
            .push((t.first, t.release.clone()));
    }
    let mut state: HashMap<&str, HashMap<String, (State, f64)>> = HashMap::new();
    for (env, mut o) in order {
        o.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let o: Vec<String> = o.into_iter().map(|x| x.1).collect();
        let d = per_env.get(env).map(Vec::as_slice).unwrap_or_default();
        state.insert(env, states(d, &o));
    }
    let prod: Vec<(String, i64)> = totals
        .iter()
        .filter(|t| t.environment == PRODUCTION)
        .map(|t| (t.release.clone(), t.first as i64))
        .collect();
    let staged: HashMap<String, i64> = totals
        .iter()
        .filter(|t| t.environment == STAGING)
        .map(|t| (t.release.clone(), t.first as i64))
        .collect();
    let promoted = staging(&prod, &staged, builds);

    let mut releases: Vec<Release> = totals
        .iter()
        .map(|t| {
            let (st, share) = state
                .get(t.environment.as_str())
                .and_then(|s| s.get(&t.release))
                .copied()
                .unwrap_or((State::RollingOut, 0.0));
            let promo = (t.environment == PRODUCTION)
                .then(|| promoted.get(&t.release))
                .flatten();
            Release {
                release: t.release.clone(),
                environment: t.environment.clone(),
                state: st,
                share,
                first_seen: local(t.first, tz),
                last_seen: local(t.last, tz),
                users: t.users,
                events: t.events,
                staging_secs: promo.map(|p| p.0),
                staged_as: promo.map(|p| p.1.clone()),
                deployed_at: None,
                deploy: None,
                verdict: None,
                reasons: Vec::new(),
            }
        })
        .collect();
    let seen: Vec<Seen> = totals
        .iter()
        .map(|t| Seen {
            environment: t.environment.clone(),
            release: t.release.clone(),
            first: t.first as i64,
        })
        .collect();
    let rows = with_deploys(&mut releases, deploys, &seen, environment, tz);
    releases.sort_by(|a, b| {
        env_rank(&a.environment)
            .cmp(&env_rank(&b.environment))
            .then_with(|| b.first_seen.cmp(&a.first_seen))
            .then_with(|| a.release.cmp(&b.release))
    });
    Ok(Releases {
        days: WINDOW_DAYS,
        timezone: tz.to_owned(),
        measure: "distinct users per day",
        releases,
        deploys: rows,
        deploy_hook: None,
        latest: None,
    })
}

/// Add the deploys to the releases: each release's deploy time and last
/// status, and a row for the deploys tied to no release with traffic
/// (each failed or rolled-back one; of those that name nothing, only the
/// environment's latest when it started or succeeded, so a start and its
/// end are one row). Returns the deploys as listed, newest first.
fn with_deploys(
    releases: &mut Vec<Release>,
    deploys: &[Deploy],
    seen: &[Seen],
    environment: Option<&str>,
    tz: &str,
) -> Vec<DeployRow> {
    let ds: Vec<Deploy> = deploys
        .iter()
        .filter(|d| environment.is_none_or(|e| d.environment == e))
        .filter(|d| Status::parse(&d.status).is_some())
        .cloned()
        .collect();
    let tied = deploys::link(&ds, seen);
    type Key = (String, String);
    let mut last: HashMap<Key, Status> = HashMap::new();
    let mut ok_at: HashMap<Key, i64> = HashMap::new();
    let mut alone: Vec<(Key, Status, i64, bool)> = Vec::new();
    let mut rows = Vec::with_capacity(ds.len());
    for (d, t) in ds.iter().zip(&tied) {
        let Some(st) = Status::parse(&d.status) else {
            continue;
        };
        let at = local(d.at as f64, tz);
        rows.push(DeployRow {
            environment: d.environment.clone(),
            release: t.clone(),
            reported_release: d.release.clone(),
            commit: d.commit.clone(),
            status: st,
            at: at.clone(),
            source: d.source.clone(),
        });
        match t {
            Some(r) => {
                let k = (d.environment.clone(), r.clone());
                if st == Status::Succeeded {
                    ok_at.insert(k.clone(), d.at);
                }
                last.insert(k, st);
            }
            None => {
                let named = d.release.is_some() || d.commit.is_some();
                let k = (d.environment.clone(), deploys::label(d, &at));
                if named {
                    alone.retain(|(x, ..)| *x != k);
                }
                alone.push((k, st, d.at, named));
            }
        }
    }
    for r in releases.iter_mut() {
        let k = (r.environment.clone(), r.release.clone());
        r.deploy = last.get(&k).copied();
        r.deployed_at = ok_at.get(&k).map(|t| local(*t as f64, tz));
    }
    // Of the unnamed deploys still deploying, the environment's latest only.
    let mut latest_unnamed: HashMap<String, i64> = HashMap::new();
    for ((env, _), _, at, named) in &alone {
        if !named {
            let e = latest_unnamed.entry(env.clone()).or_insert(*at);
            *e = (*e).max(*at);
        }
    }
    for ((env, label), st, at, named) in alone {
        let state = match st {
            Status::Failed => State::Failed,
            Status::RolledBack => State::RolledBack,
            Status::Started | Status::Succeeded => {
                if !named && latest_unnamed.get(&env) != Some(&at) {
                    continue;
                }
                State::Deploying
            }
        };
        let when = local(at as f64, tz);
        releases.push(Release {
            release: label,
            environment: env,
            state,
            share: 0.0,
            first_seen: when.clone(),
            last_seen: when.clone(),
            users: 0,
            events: 0,
            staging_secs: None,
            staged_as: None,
            deployed_at: (st == Status::Succeeded).then_some(when),
            deploy: Some(st),
            verdict: None,
            reasons: Vec::new(),
        });
    }
    rows.reverse();
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Days `0..` of `(release, users)` each.
    fn days(rows: &[&[(&str, i64)]]) -> Vec<Daily> {
        rows.iter()
            .enumerate()
            .flat_map(|(i, rs)| {
                rs.iter().map(move |(r, n)| Daily {
                    day: format!("2026-11-{:02}", i + 1),
                    release: (*r).to_owned(),
                    users: *n,
                })
            })
            .collect()
    }

    fn order(rs: &[&str]) -> Vec<String> {
        rs.iter().map(|r| (*r).to_owned()).collect()
    }

    fn state(daily: &[Daily], o: &[String], r: &str) -> State {
        states(daily, o)[r].0
    }

    #[test]
    fn a_release_rolls_out_then_goes_live_and_supersedes_the_old_one() {
        let o = order(&["v1", "v2"]);
        let rolling = days(&[&[("v1", 100)], &[("v1", 80), ("v2", 20)]]);
        assert_eq!(state(&rolling, &o, "v2"), State::RollingOut);
        assert_eq!(
            state(&rolling, &o, "v1"),
            State::Live,
            "a canary does not demote it"
        );
        let live = days(&[
            &[("v1", 100)],
            &[("v1", 40), ("v2", 60)],
            &[("v1", 2), ("v2", 98)],
        ]);
        assert_eq!(state(&live, &o, "v2"), State::Live);
        assert_eq!(state(&live, &o, "v1"), State::Superseded);
        // The majority but below 95 % and never above: still rolling out.
        let most = days(&[&[("v1", 100)], &[("v1", 30), ("v2", 70)]]);
        assert_eq!(state(&most, &o, "v2"), State::RollingOut);
        assert_eq!(state(&most, &o, "v1"), State::Superseded);
    }

    #[test]
    fn an_older_release_taking_the_majority_back_is_a_rollback() {
        let o = order(&["v1", "v2"]);
        let back = days(&[
            &[("v1", 100)],
            &[("v1", 10), ("v2", 90)],
            &[("v1", 99), ("v2", 1)],
        ]);
        assert_eq!(state(&back, &o, "v2"), State::RolledBack);
        assert_eq!(state(&back, &o, "v1"), State::Live, "back in place");
        // Shipped again and live: no longer rolled back.
        let again = days(&[
            &[("v1", 100)],
            &[("v1", 10), ("v2", 90)],
            &[("v1", 99), ("v2", 1)],
            &[("v2", 100)],
        ]);
        assert_eq!(state(&again, &o, "v2"), State::Live);
        assert_eq!(state(&again, &o, "v1"), State::Superseded);
        // A newer release after the rollback supersedes both.
        let o3 = order(&["v1", "v2", "v3"]);
        let next = days(&[
            &[("v1", 10), ("v2", 90)],
            &[("v1", 99), ("v2", 1)],
            &[("v3", 100)],
        ]);
        assert_eq!(state(&next, &o3, "v2"), State::Superseded);
        assert_eq!(state(&next, &o3, "v3"), State::Live);
    }

    #[test]
    fn a_release_never_in_the_majority_is_superseded_by_a_newer_one() {
        let o = order(&["v1", "v2", "v3"]);
        let skipped = days(&[&[("v1", 90), ("v2", 10)], &[("v1", 10), ("v3", 90)]]);
        assert_eq!(state(&skipped, &o, "v2"), State::Superseded);
        // A canary next to an older majority keeps rolling out.
        let canary = days(&[&[("v1", 95), ("v2", 5)], &[("v1", 90), ("v2", 10)]]);
        assert_eq!(state(&canary, &o, "v2"), State::RollingOut);
    }

    #[test]
    fn the_shares_are_of_the_latest_day() {
        let o = order(&["v1", "v2"]);
        let s = states(&days(&[&[("v1", 100)], &[("v1", 1), ("v2", 3)]]), &o);
        assert!(s["v2"].1 > s["v1"].1);
        assert!((s["v1"].1 + s["v2"].1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn deploys_give_the_deploy_time_and_list_failed_deploys_with_no_traffic() {
        let row = |name: &str| Release {
            release: name.into(),
            environment: PRODUCTION.into(),
            state: State::Live,
            share: 1.0,
            first_seen: String::new(),
            last_seen: String::new(),
            users: 3,
            events: 3,
            staging_secs: None,
            staged_as: None,
            deployed_at: None,
            deploy: None,
            verdict: None,
            reasons: Vec::new(),
        };
        let dep = |release: Option<&str>, status: Status, at: i64| Deploy {
            environment: PRODUCTION.into(),
            release: release.map(str::to_owned),
            commit: None,
            status: status.as_str().into(),
            at,
            source: "plain".into(),
        };
        let mut rs = vec![row("v1")];
        let seen = [Seen {
            environment: PRODUCTION.into(),
            release: "v1".into(),
            first: 1_000,
        }];
        let deploys = [
            dep(Some("v1"), Status::Started, 900),
            dep(Some("v1"), Status::Succeeded, 950),
            dep(Some("v2"), Status::Started, 2_000),
            dep(Some("v2"), Status::Failed, 2_100),
            // Unnamed: only the latest one still deploying is listed.
            dep(None, Status::Started, 3_000),
            dep(None, Status::Succeeded, 3_100),
        ];
        let listed = with_deploys(&mut rs, &deploys, &seen, None, "UTC");
        assert_eq!(listed.len(), deploys.len());
        assert_eq!(listed[0].status, Status::Succeeded, "newest first");
        assert_eq!(rs[0].deploy, Some(Status::Succeeded));
        assert!(rs[0].deployed_at.is_some());
        let v2: Vec<_> = rs.iter().filter(|r| r.release == "v2").collect();
        assert_eq!(v2.len(), 1, "a start and its failure are one row");
        assert_eq!((v2[0].state, v2[0].users), (State::Failed, 0));
        let unnamed: Vec<_> = rs
            .iter()
            .filter(|r| r.release.starts_with("deploy "))
            .collect();
        assert_eq!(unnamed.len(), 1);
        assert_eq!(unnamed[0].state, State::Deploying);
    }

    #[test]
    fn a_build_staged_under_another_name_counts_its_time_in_staging() {
        let b = |env: &str, build: &str, name: &str| ReleaseBuild {
            environment: env.into(),
            build: build.into(),
            name: name.into(),
            first_seen: 0,
        };
        let builds = [
            b(STAGING, "aaaa", "2026-11-01.1"),
            b(PRODUCTION, "aaaa", "2026-11-03.1"),
        ];
        let prod = [
            ("2026-11-03.1".to_owned(), 5_000),
            ("v2".to_owned(), 9_000),
            ("v3".to_owned(), 9_000),
        ];
        let staged: HashMap<String, i64> = [
            ("2026-11-01.1".to_owned(), 1_000),
            ("v2".to_owned(), 8_000),
            // Seen in staging only after production: not a promotion.
            ("v3".to_owned(), 9_500),
        ]
        .into();
        let s = staging(&prod, &staged, &builds);
        assert_eq!(s["2026-11-03.1"], (4_000, "2026-11-01.1".to_owned()));
        assert_eq!(s["v2"], (1_000, "v2".to_owned()));
        assert!(!s.contains_key("v3"));
    }
}

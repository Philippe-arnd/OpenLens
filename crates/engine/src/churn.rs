//! Churn as one section (#247), around the PM's questions: how many am I
//! losing, after how long, at which moment, with what friction, and who is
//! at risk now. A user has churned after `churn_after_days` without an
//! event (docs/kpis.md), or at once when they send the map's
//! `churn_event` (an explicit cancel, #269). Last 90 days, identified users.
//!
//! Part 2 (#269): why they leave (features adopted early by those who
//! stay, friction on the last day, the release they arrived on, survival
//! by role), the top reasons ranked with their evidence, and what brought
//! back the users who returned.

use std::collections::{HashMap, HashSet};

use chrono::{NaiveDate, Utc};
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{Filters, INTERACTION, Result, any_person, identified, int, source, text};

/// A user at risk: active within the window, quiet for over half of it,
/// and less active than in the window before.
#[derive(Debug, Clone, Serialize)]
pub struct AtRisk {
    /// User id.
    pub distinct_id: String,
    /// Raw role, if any.
    pub role: Option<String>,
    /// Last active day (YYYY-MM-DD).
    pub last_day: String,
    /// Days since then.
    pub idle_days: i64,
    /// Active days in the last window vs the one before.
    pub recent_days: i64,
    /// Active days in the window before.
    pub before_days: i64,
}

/// The churn section's numbers.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChurnReport {
    /// The churn window used (days).
    pub window: i64,
    /// A window suggested by the data (the 90th percentile of gaps between
    /// active days, rounded to 7, 14, 21, 30, 45 or 60), when enough data.
    pub suggested: Option<i64>,
    /// Users active at the start of the last window.
    pub active_at_start: i64,
    /// Of them, churned since.
    pub churned_recent: i64,
    /// New users in the last window.
    pub new_recent: i64,
    /// Users who churned and came back in the last 30 days.
    pub resurrected: i64,
    /// (week start YYYY-MM-DD, users whose last active day fell that week
    /// and who have churned), oldest first.
    pub by_week: Vec<(String, i64)>,
    /// Survival: (days since first visit, share still active), Kaplan–Meier.
    pub survival: Vec<(i64, f64)>,
    /// Median lifetime in days, when the curve crosses one half.
    pub median_days: Option<i64>,
    /// Churned users by week of life at departure (0 = first week).
    pub leave_week: Vec<(i64, i64)>,
    /// Last place before leaving: (feature or page, churned users, share
    /// of churned users, share of retained users' last place).
    pub last_place: Vec<(String, i64, f64, f64)>,
    /// Share of churned users with an error in their last active day, and
    /// the same for retained users.
    pub errors_at_end: (f64, f64),
    /// Users at risk now, quietest first.
    pub at_risk: Vec<AtRisk>,
    /// Users counted.
    pub users: i64,
    /// Features adopted in the first 14 days of life, with the churn of
    /// adopters and non-adopters (correlation, not cause), biggest gap first.
    pub lift: Vec<Lift>,
    /// Friction on the last active day: (kind, share of churned users,
    /// share of retained users).
    pub friction: Vec<(String, f64, f64)>,
    /// Churn by the release users arrived on: (release, users, churned share).
    pub by_release: Vec<(String, i64, f64)>,
    /// Survival by role: (role, users, share still active 30 days after
    /// their first visit).
    pub by_role: Vec<(String, i64, f64)>,
    /// Users who sent the map's `churn_event` (counted as churned).
    pub cancelled: i64,
    /// The top reasons, ranked (at most 5).
    pub reasons: Vec<Reason>,
    /// Users who came back after churning, last 30 days.
    pub winback: WinBack,
}

/// One feature's link with staying (#269).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Lift {
    /// Feature key.
    pub feature: String,
    /// Users who used it in their first 14 days.
    pub adopters: i64,
    /// Users who did not.
    pub non_adopters: i64,
    /// Churned share of adopters.
    pub churn_adopters: f64,
    /// Churned share of non-adopters.
    pub churn_non: f64,
}

/// A reason users leave, with the numbers behind it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Reason {
    /// `feature`, `friction`, `place` or `release`.
    pub kind: &'static str,
    /// In words.
    pub title: String,
    /// The numbers.
    pub evidence: String,
    /// Gap in churned share (or in share of churned vs retained): the rank.
    pub weight: f64,
}

/// What brought churned users back (#269).
#[derive(Debug, Clone, Default, Serialize)]
pub struct WinBack {
    /// Users back in the last 30 days after a gap longer than the window.
    pub users: i64,
    /// (release on the day they returned, users).
    pub by_release: Vec<(String, i64)>,
    /// (first feature used on the day they returned, users).
    pub by_feature: Vec<(String, i64)>,
    /// Mean active days since they returned.
    pub mean_active_days: f64,
}

/// Below this many users on a side, a comparison is not shown.
const MIN_SIDE: i64 = 8;
/// Days of life in which adoption is measured.
const EARLY_DAYS: i64 = 14;

/// Rank features by how much less their early adopters churn (pure).
/// `first`: user → first day; `gone`: user → churned; `adopted`: (user,
/// feature, first use day). Only users first seen 14 days ago or more.
#[must_use]
pub fn lift(
    first: &HashMap<String, NaiveDate>,
    gone: &HashMap<String, bool>,
    adopted: &[(String, String, NaiveDate)],
    today: NaiveDate,
) -> Vec<Lift> {
    let eligible: Vec<&String> = first
        .iter()
        .filter(|(_, d)| (today - **d).num_days() >= EARLY_DAYS)
        .map(|(u, _)| u)
        .collect();
    let mut early: HashMap<&str, std::collections::HashSet<&str>> = HashMap::new();
    for (u, f, d) in adopted {
        if let Some(f0) = first.get(u)
            && (*d - *f0).num_days() < EARLY_DAYS
        {
            early.entry(f.as_str()).or_default().insert(u.as_str());
        }
    }
    let mut out: Vec<Lift> = early
        .into_iter()
        .filter_map(|(feat, users)| {
            let (mut a, mut ac, mut n, mut nc) = (0i64, 0i64, 0i64, 0i64);
            for u in &eligible {
                let g = gone.get(*u).copied().unwrap_or(false);
                if users.contains(u.as_str()) {
                    a += 1;
                    ac += i64::from(g);
                } else {
                    n += 1;
                    nc += i64::from(g);
                }
            }
            (a >= MIN_SIDE && n >= MIN_SIDE).then(|| Lift {
                feature: feat.to_owned(),
                adopters: a,
                non_adopters: n,
                churn_adopters: ac as f64 / a as f64,
                churn_non: nc as f64 / n as f64,
            })
        })
        .collect();
    out.sort_by(|x, y| {
        (y.churn_non - y.churn_adopters)
            .total_cmp(&(x.churn_non - x.churn_adopters))
            .then(x.feature.cmp(&y.feature))
    });
    out.truncate(10);
    out
}

/// The top reasons from the report's parts, ranked by weight (pure).
#[must_use]
pub fn reasons(r: &ChurnReport) -> Vec<Reason> {
    let pct = |v: f64| format!("{:.0}%", v * 100.0);
    let mut out = Vec::new();
    for l in &r.lift {
        let gap = l.churn_non - l.churn_adopters;
        if gap >= 0.05 {
            out.push(Reason {
                kind: "feature",
                title: format!("Users who don't try `{}` early leave more", l.feature),
                evidence: format!(
                    "{} of {} non-adopters churned vs {} of {} who used it in their first 14 days (correlation)",
                    pct(l.churn_non),
                    l.non_adopters,
                    pct(l.churn_adopters),
                    l.adopters
                ),
                weight: gap,
            });
        }
    }
    for (kind, c, k) in &r.friction {
        if c - k >= 0.05 {
            out.push(Reason {
                kind: "friction",
                title: format!("{kind} on their last day"),
                evidence: format!(
                    "{} of churned users vs {} of retained users",
                    pct(*c),
                    pct(*k)
                ),
                weight: c - k,
            });
        }
    }
    for (place, n, share, base) in &r.last_place {
        if *n >= MIN_SIDE && share - base >= 0.1 {
            out.push(Reason {
                kind: "place",
                title: format!("Many leave after `{place}`"),
                evidence: format!(
                    "last place of {} of churned users vs {} of retained users",
                    pct(*share),
                    pct(*base)
                ),
                weight: share - base,
            });
        }
    }
    let all: i64 = r.by_release.iter().map(|x| x.1).sum();
    let mean = if all > 0 {
        r.by_release.iter().map(|x| x.2 * x.1 as f64).sum::<f64>() / all as f64
    } else {
        0.0
    };
    for (rel, n, share) in &r.by_release {
        if *n >= MIN_SIDE && share - mean >= 0.1 {
            out.push(Reason {
                kind: "release",
                title: format!("Users who arrived on `{rel}` leave more"),
                evidence: format!("{} of {n} churned, vs {} overall", pct(*share), pct(mean)),
                weight: share - mean,
            });
        }
    }
    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out.truncate(5);
    out
}

fn day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// Kaplan–Meier survival from (lifetime in days, churned?) pairs, at the
/// given days.
#[must_use]
pub fn survival(lives: &[(i64, bool)], at: &[i64]) -> Vec<(i64, f64)> {
    let mut times: Vec<i64> = lives.iter().filter(|l| l.1).map(|l| l.0).collect();
    times.sort_unstable();
    times.dedup();
    let mut s = 1.0;
    let mut curve: Vec<(i64, f64)> = vec![(0, 1.0)];
    for t in times {
        let at_risk = lives.iter().filter(|l| l.0 >= t).count() as f64;
        let died = lives.iter().filter(|l| l.1 && l.0 == t).count() as f64;
        if at_risk > 0.0 {
            s *= 1.0 - died / at_risk;
        }
        curve.push((t, s));
    }
    at.iter()
        .map(|&d| {
            let v = curve
                .iter()
                .rev()
                .find(|(t, _)| *t <= d)
                .map_or(1.0, |c| c.1);
            (d, v)
        })
        .collect()
}

/// The churn numbers of the board's Retention tile (#616): the churn
/// rate and the median lifetime, read with the same users query and the
/// same rules as [`churn`], so the board and the Churn page agree.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChurnLine {
    /// The churn window used (days).
    pub window: i64,
    /// Users active at the start of the last window.
    pub active_at_start: i64,
    /// Of them, churned since.
    pub churned_recent: i64,
    /// Median lifetime in days (Kaplan–Meier), when the curve crosses
    /// one half.
    pub median_days: Option<i64>,
}

impl ChurnLine {
    /// The churn rate (docs/kpis.md): churned ÷ active at the start; none
    /// when nobody was active at the start.
    #[must_use]
    pub fn rate(&self) -> Option<f64> {
        (self.active_at_start > 0).then(|| self.churned_recent as f64 / self.active_at_start as f64)
    }
}

/// One row per identified user over the window `w`: `distinct_id`,
/// `role`, `first` and `last` (interactions only), and the active days in
/// the last churn window `c` (`recent`) and in the one before (`before`).
fn users_sql(src: &str, w: &str, ident: &str, c: i64) -> String {
    format!(
        "SELECT distinct_id, max(role) AS role, CAST(min(timestamp) AS VARCHAR) AS first, \
         CAST(max(timestamp) AS VARCHAR) AS last, \
         count(DISTINCT CASE WHEN timestamp >= now() - INTERVAL '{c} days' THEN date END) AS recent, \
         count(DISTINCT CASE WHEN timestamp < now() - INTERVAL '{c} days' \
               AND timestamp >= now() - INTERVAL '{c2} days' THEN date END) AS before \
         FROM {src} WHERE {w} AND {ident} AND {INTERACTION} GROUP BY distinct_id",
        c2 = 2 * c
    )
}

/// The users who sent the map's `churn_event` over the window `w`.
async fn cancelled(
    q: &QueryEngine,
    project: &str,
    src: &str,
    w: &str,
    map: Option<&FeatureMap>,
) -> Result<HashSet<String>> {
    let Some(ev) = map.and_then(|m| m.churn_event.as_deref()) else {
        return Ok(HashSet::new());
    };
    let sql = format!(
        "SELECT DISTINCT distinct_id FROM {src} WHERE {w} AND event = {}",
        crate::lit(ev)?
    );
    Ok(q.sql(project, &sql)
        .await?
        .rows
        .iter()
        .filter_map(|r| text(r, "distinct_id"))
        .collect())
}

/// A user of [`users_sql`]: their first and last active days, the days
/// since, whether they cancelled, and whether they churned.
struct Life {
    id: String,
    first: NaiveDate,
    last: NaiveDate,
    idle: i64,
    explicit: bool,
    gone: bool,
}

/// The user of a [`users_sql`] row, `today`, with the churn window `c`.
fn life(
    r: &Map<String, Value>,
    today: NaiveDate,
    c: i64,
    cancelled: &HashSet<String>,
) -> Option<Life> {
    let (id, first, last) = (
        text(r, "distinct_id")?,
        text(r, "first").as_deref().and_then(day)?,
        text(r, "last").as_deref().and_then(day)?,
    );
    let idle = (today - last).num_days();
    let explicit = cancelled.contains(&id);
    Some(Life {
        id,
        first,
        last,
        idle,
        explicit,
        gone: idle > c || explicit,
    })
}

/// Active in the window before the last one, which starts at `start`:
/// counted at its start.
fn at_start(first: NaiveDate, last: NaiveDate, start: NaiveDate, c: i64) -> bool {
    first < start && last >= start - chrono::Duration::days(c)
}

/// The first day the Kaplan–Meier curve of `lives` falls under one half.
fn median_days(lives: &[(i64, bool)]) -> Option<i64> {
    let fine: Vec<i64> = (0..=90).collect();
    survival(lives, &fine)
        .into_iter()
        .find(|(_, s)| *s < 0.5)
        .map(|(d, _)| d)
}

/// The board's [`ChurnLine`] (#616): the users query of [`churn`] and the
/// cancellations only, on the board's engine `q` (its parameters set),
/// source `src` and identified-user condition `ident`.
///
/// The line is kept for [`LINE_FOR`] per data root, project, filters, churn
/// window, churn event and identified-user condition (#748): a 90-day read
/// of every identified user (about 7 s at 4 M events) for two numbers that
/// move by the day was paid again on every recompute of every board.
pub(crate) async fn line(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    src: &str,
    ident: &str,
) -> Result<ChurnLine> {
    let c = map.map_or(14, |m| i64::from(m.churn_days()));
    let key = format!(
        "{}\u{0}{project}\u{0}{f:?}\u{0}{c}\u{0}{:?}\u{0}{src}\u{0}{ident}",
        q.root().display(),
        map.and_then(|m| m.churn_event.as_deref()),
    );
    if let Some(kept) = kept_line(&key) {
        return Ok(kept);
    }
    let out = line_now(q, project, map, f, src, ident, c).await?;
    keep_line(key, &out);
    Ok(out)
}

/// How long a computed churn line is reused.
const LINE_FOR: std::time::Duration = std::time::Duration::from_secs(3600);
/// Lines kept at most (one per board key; the oldest goes first).
const LINES_KEPT: usize = 256;

type LineCache =
    std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, ChurnLine)>>;

fn line_cache() -> &'static LineCache {
    static CACHE: std::sync::OnceLock<LineCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(LineCache::default)
}

/// The line kept for `key`, when recent enough. A poisoned lock reads as
/// nothing kept: the line is computed again.
fn kept_line(key: &str) -> Option<ChurnLine> {
    let cache = line_cache().lock().ok()?;
    let (at, l) = cache.get(key)?;
    (at.elapsed() < LINE_FOR).then(|| l.clone())
}

fn keep_line(key: String, l: &ChurnLine) {
    let Ok(mut cache) = line_cache().lock() else {
        return;
    };
    cache.retain(|_, (at, _)| at.elapsed() < LINE_FOR);
    if cache.len() >= LINES_KEPT
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(k, _)| k.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(key, (std::time::Instant::now(), l.clone()));
}

/// [`line`], computed.
async fn line_now(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
    src: &str,
    ident: &str,
    c: i64,
) -> Result<ChurnLine> {
    let w = f.clause(90)?;
    let users = users_sql(src, &w, ident, c);
    let (u, cancelled) =
        futures::join!(q.sql(project, &users), cancelled(q, project, src, &w, map));
    let cancelled = cancelled?;
    let today = Utc::now().date_naive();
    let start = today - chrono::Duration::days(c);
    let mut out = ChurnLine {
        window: c,
        ..ChurnLine::default()
    };
    let mut lives = Vec::new();
    for r in &u?.rows {
        let Some(l) = life(r, today, c, &cancelled) else {
            continue;
        };
        if at_start(l.first, l.last, start, c) {
            out.active_at_start += 1;
            out.churned_recent += i64::from(l.gone);
        }
        lives.push(((l.last - l.first).num_days(), l.gone));
    }
    out.median_days = median_days(&lives);
    Ok(out)
}

/// The churn report of a project.
pub async fn churn(
    q: &QueryEngine,
    project: &str,
    map: Option<&FeatureMap>,
    f: &Filters,
) -> Result<ChurnReport> {
    let q = &q.with_params(&f.params()?);
    let src = source(map);
    let w = f.clause(90)?;
    let ident = identified(any_person(q, project).await?);
    let c = map.map_or(14, |m| i64::from(m.churn_days()));
    let users_sql = users_sql(&src, &w, ident, c);
    let days_sql = format!(
        "SELECT distinct_id, date, max(release) AS rel, min(feature) AS feat FROM {src} WHERE {w} AND {ident} AND {INTERACTION} \
         GROUP BY distinct_id, date ORDER BY distinct_id, date LIMIT 500000"
    );
    let adopt_sql = format!(
        "SELECT distinct_id, feature, CAST(min(timestamp) AS VARCHAR) AS t FROM {src} \
         WHERE {w} AND {ident} AND feature IS NOT NULL GROUP BY distinct_id, feature LIMIT 500000"
    );
    let friction_sql = format!(
        "WITH l AS (SELECT distinct_id, max(date) AS d FROM {src} WHERE {w} AND {ident} AND {INTERACTION} GROUP BY distinct_id) \
         SELECT e.distinct_id, \
                max(CASE WHEN e.event = '$click' AND e.click_rage THEN 1 ELSE 0 END) AS rage, \
                max(CASE WHEN e.event = '$request_failed' THEN 1 ELSE 0 END) AS failed, \
                max(CASE WHEN e.event = '$vital' AND e.vital_rating = 'poor' THEN 1 ELSE 0 END) AS slow \
         FROM (SELECT distinct_id, event, click_rage, vital_rating, date FROM {src} WHERE {w}) e \
         JOIN l ON e.distinct_id = l.distinct_id AND e.date = l.d GROUP BY e.distinct_id"
    );
    let release_sql = format!(
        "SELECT distinct_id, first_value(release ORDER BY timestamp) AS rel FROM {src} \
         WHERE {w} AND {ident} AND release IS NOT NULL GROUP BY distinct_id"
    );
    let active_rows = crate::tasks::event_rows(&src, f)?;
    let last_sql = format!(
        "WITH l AS (SELECT distinct_id, max(timestamp) AS t FROM {src} WHERE {w} AND {ident} AND {INTERACTION} GROUP BY distinct_id) \
         SELECT e.distinct_id, max(coalesce(e.feature, e.path)) AS place, \
                max(CASE WHEN ev.event = '$error' THEN 1 ELSE 0 END) AS err \
         FROM (SELECT distinct_id, timestamp, date, feature, path FROM {src} WHERE {w}) e \
         JOIN l ON e.distinct_id = l.distinct_id AND e.timestamp = l.t \
         LEFT JOIN (SELECT distinct_id, event, date FROM {src} WHERE {w} AND event = '$error') ev \
           ON ev.distinct_id = e.distinct_id AND ev.date = e.date \
         GROUP BY e.distinct_id"
    );
    let (u, d, l, resurrected) = futures::join!(
        q.sql(project, &users_sql),
        q.sql(project, &days_sql),
        q.sql(project, &last_sql),
        crate::tasks::resurrected(q, project, &active_rows, f, c),
    );
    let (adopt, fr, rel) = futures::join!(
        q.sql(project, &adopt_sql),
        q.sql(project, &friction_sql),
        q.sql(project, &release_sql),
    );
    let cancelled = cancelled(q, project, &src, &w, map).await?;
    let today = Utc::now().date_naive();
    let mut out = ChurnReport {
        window: c,
        resurrected: resurrected.unwrap_or(0),
        ..ChurnReport::default()
    };
    let mut churned: HashMap<String, bool> = HashMap::new();
    let mut firsts: HashMap<String, NaiveDate> = HashMap::new();
    let mut roles: HashMap<String, Vec<(i64, bool)>> = HashMap::new();
    let mut lives: Vec<(i64, bool)> = Vec::new();
    let mut weeks: HashMap<String, i64> = HashMap::new();
    let mut leave: HashMap<i64, i64> = HashMap::new();
    let start = today - chrono::Duration::days(c);
    for r in &u?.rows {
        let Some(Life {
            id,
            first,
            last,
            idle,
            explicit,
            gone,
        }) = life(r, today, c, &cancelled)
        else {
            continue;
        };
        out.users += 1;
        out.cancelled += i64::from(explicit);
        churned.insert(id.clone(), gone);
        firsts.insert(id.clone(), first);
        let life = (last - first).num_days();
        lives.push((life, gone));
        roles
            .entry(text(r, "role").unwrap_or_else(|| "(none)".into()))
            .or_default()
            .push((life, gone));
        if first >= start {
            out.new_recent += 1;
        }
        if at_start(first, last, start, c) {
            out.active_at_start += 1;
            if gone {
                out.churned_recent += 1;
            }
        }
        if gone {
            let monday = last
                - chrono::Duration::days(i64::from(
                    chrono::Datelike::weekday(&last).num_days_from_monday(),
                ));
            *weeks
                .entry(monday.format("%Y-%m-%d").to_string())
                .or_default() += 1;
            *leave.entry((life / 7).min(12)).or_default() += 1;
        } else {
            let recent = int(r, "recent");
            let before = int(r, "before");
            if idle * 2 > c && recent < before {
                out.at_risk.push(AtRisk {
                    distinct_id: id,
                    role: text(r, "role"),
                    last_day: last.format("%Y-%m-%d").to_string(),
                    idle_days: idle,
                    recent_days: recent,
                    before_days: before,
                });
            }
        }
    }
    out.at_risk.sort_by(|a, b| {
        b.idle_days
            .cmp(&a.idle_days)
            .then(a.distinct_id.cmp(&b.distinct_id))
    });
    out.at_risk.truncate(100);
    let mut by_week: Vec<(String, i64)> = weeks.into_iter().collect();
    by_week.sort();
    out.by_week = by_week;
    let mut lw: Vec<(i64, i64)> = leave.into_iter().collect();
    lw.sort_unstable();
    out.leave_week = lw;
    let points: Vec<i64> = [0, 1, 2, 3, 7, 14, 21, 30, 45, 60, 75, 90].to_vec();
    out.survival = survival(&lives, &points);
    out.median_days = median_days(&lives);
    // Gaps between active days → a suggested window; returns after a gap
    // longer than the window in the last 30 days → win-back.
    let mut gaps: Vec<i64> = Vec::new();
    let mut prev: Option<(String, NaiveDate)> = None;
    let mut back: HashMap<String, (NaiveDate, Option<String>, Option<String>)> = HashMap::new();
    let mut active_after: HashMap<String, i64> = HashMap::new();
    let recent = today - chrono::Duration::days(30);
    for r in &d?.rows {
        let (Some(id), Some(dt)) = (
            text(r, "distinct_id"),
            text(r, "date").as_deref().and_then(day),
        ) else {
            continue;
        };
        if let Some((pid, pd)) = &prev
            && *pid == id
        {
            let gap = (dt - *pd).num_days();
            gaps.push(gap);
            if gap > c && dt >= recent {
                back.insert(id.clone(), (dt, text(r, "rel"), text(r, "feat")));
                active_after.insert(id.clone(), 0);
            }
        }
        if let Some(n) = active_after.get_mut(&id) {
            *n += 1;
        }
        prev = Some((id, dt));
    }
    out.winback.users = back.len() as i64;
    let mut wr: HashMap<String, i64> = HashMap::new();
    let mut wf: HashMap<String, i64> = HashMap::new();
    for (rel, feat) in back.values().map(|(_, r, f)| (r, f)) {
        *wr.entry(rel.clone().unwrap_or_else(|| "(none)".into()))
            .or_default() += 1;
        *wf.entry(feat.clone().unwrap_or_else(|| "(page)".into()))
            .or_default() += 1;
    }
    let top = |m: HashMap<String, i64>| {
        let mut v: Vec<(String, i64)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(8);
        v
    };
    out.winback.by_release = top(wr);
    out.winback.by_feature = top(wf);
    out.winback.mean_active_days = if active_after.is_empty() {
        0.0
    } else {
        active_after.values().sum::<i64>() as f64 / active_after.len() as f64
    };
    // Why: early adoption, friction, arrival release, roles.
    let adopted: Vec<(String, String, NaiveDate)> = adopt?
        .rows
        .iter()
        .filter_map(|r| {
            Some((
                text(r, "distinct_id")?,
                text(r, "feature")?,
                text(r, "t").as_deref().and_then(day)?,
            ))
        })
        .collect();
    out.lift = lift(&firsts, &churned, &adopted, today);
    let mut fc = [0i64; 3];
    let mut fk = [0i64; 3];
    let (mut nc, mut nk) = (0i64, 0i64);
    for r in &fr?.rows {
        let Some(g) = text(r, "distinct_id").and_then(|id| churned.get(&id).copied()) else {
            continue;
        };
        let hits = [int(r, "rage"), int(r, "failed"), int(r, "slow")];
        let (cnt, n) = if g {
            (&mut fc, &mut nc)
        } else {
            (&mut fk, &mut nk)
        };
        *n += 1;
        for (i, h) in hits.iter().enumerate() {
            cnt[i] += i64::from(*h > 0);
        }
    }
    if nc >= MIN_SIDE && nk >= MIN_SIDE {
        for (i, kind) in [
            "Rage clicks",
            "Failed requests",
            "Slow pages (poor Web Vitals)",
        ]
        .iter()
        .enumerate()
        {
            out.friction.push((
                (*kind).to_owned(),
                fc[i] as f64 / nc as f64,
                fk[i] as f64 / nk as f64,
            ));
        }
    }
    let mut rels: HashMap<String, (i64, i64)> = HashMap::new();
    for r in &rel?.rows {
        if let (Some(id), Some(rl)) = (text(r, "distinct_id"), text(r, "rel"))
            && let Some(g) = churned.get(&id)
        {
            let e = rels.entry(rl).or_default();
            e.0 += 1;
            e.1 += i64::from(*g);
        }
    }
    let mut br: Vec<(String, i64, f64)> = rels
        .into_iter()
        .map(|(k, (n, g))| (k, n, g as f64 / n.max(1) as f64))
        .collect();
    br.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    br.truncate(12);
    out.by_release = br;
    let mut rr: Vec<(String, i64, f64)> = roles
        .into_iter()
        .map(|(k, v)| {
            let s30 = survival(&v, &[30]).first().map_or(1.0, |x| x.1);
            (k, v.len() as i64, s30)
        })
        .collect();
    rr.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rr.truncate(12);
    out.by_role = rr;
    if gaps.len() >= 50 {
        gaps.sort_unstable();
        let p90 = gaps[(gaps.len() - 1) * 9 / 10];
        out.suggested = [7, 14, 21, 30, 45, 60]
            .into_iter()
            .find(|&s| s >= p90)
            .or(Some(60));
    }
    // Last place and errors at the end, churned vs retained.
    let mut place_c: HashMap<String, i64> = HashMap::new();
    let mut place_r: HashMap<String, i64> = HashMap::new();
    let (mut n_c, mut n_r, mut err_c, mut err_r) = (0i64, 0i64, 0i64, 0i64);
    for r in &l?.rows {
        let Some(id) = text(r, "distinct_id") else {
            continue;
        };
        let Some(&gone) = churned.get(&id) else {
            continue;
        };
        let place = text(r, "place").unwrap_or_else(|| "(none)".into());
        let err = int(r, "err") > 0;
        if gone {
            n_c += 1;
            err_c += i64::from(err);
            *place_c.entry(place).or_default() += 1;
        } else {
            n_r += 1;
            err_r += i64::from(err);
            *place_r.entry(place).or_default() += 1;
        }
    }
    let mut lp: Vec<(String, i64, f64, f64)> = place_c
        .into_iter()
        .map(|(p, n)| {
            let base = place_r.get(&p).copied().unwrap_or(0) as f64 / n_r.max(1) as f64;
            (p, n, n as f64 / n_c.max(1) as f64, base)
        })
        .collect();
    lp.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    lp.truncate(10);
    out.last_place = lp;
    out.errors_at_end = (
        err_c as f64 / n_c.max(1) as f64,
        err_r as f64 / n_r.max(1) as f64,
    );
    if n_c >= MIN_SIDE && n_r >= MIN_SIDE {
        out.friction.insert(
            0,
            (
                "Errors".to_owned(),
                out.errors_at_end.0,
                out.errors_at_end.1,
            ),
        );
    }
    out.reasons = reasons(&out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_adopters_who_stay_rank_their_feature_first() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap_or_default();
        let start = today - chrono::Duration::days(60);
        let mut first = HashMap::new();
        let mut gone = HashMap::new();
        let mut adopted = Vec::new();
        for i in 0..40 {
            let u = format!("u{i}");
            first.insert(u.clone(), start);
            // Half adopt `share` in week 1 and stay; the rest leave.
            let keeps = i < 20;
            gone.insert(u.clone(), !keeps);
            if keeps {
                adopted.push((
                    u.clone(),
                    "share".to_owned(),
                    start + chrono::Duration::days(3),
                ));
            }
            // Everyone uses `edit`, but late adoption does not count.
            adopted.push((
                u.clone(),
                "edit".to_owned(),
                start + chrono::Duration::days(if i % 2 == 0 { 2 } else { 30 }),
            ));
        }
        // A user too young to judge is left out.
        first.insert("new".into(), today - chrono::Duration::days(3));
        let l = lift(&first, &gone, &adopted, today);
        assert_eq!(l[0].feature, "share");
        assert_eq!((l[0].adopters, l[0].non_adopters), (20, 20));
        assert!((l[0].churn_non - 1.0).abs() < 1e-9 && l[0].churn_adopters < 1e-9);
        let edit = l.iter().find(|x| x.feature == "edit").unwrap_or(&l[0]);
        assert!(
            (edit.churn_non - edit.churn_adopters).abs() < 1e-9,
            "no lift for edit"
        );
        let r = ChurnReport {
            lift: l,
            friction: vec![
                ("Rage clicks".into(), 0.4, 0.1),
                ("Errors".into(), 0.1, 0.1),
            ],
            ..ChurnReport::default()
        };
        let rs = reasons(&r);
        assert_eq!(rs[0].kind, "feature");
        assert_eq!(rs[1].kind, "friction");
        assert_eq!(rs.len(), 2);
    }

    #[test]
    fn kaplan_meier_counts_recent_users_correctly() {
        // Two churned at day 5 and 10; two still active (censored) at day 3 and 20.
        let lives = [(5, true), (10, true), (3, false), (20, false)];
        let s = survival(&lives, &[0, 4, 5, 10, 30]);
        assert_eq!(s[0], (0, 1.0));
        assert!((s[1].1 - 1.0).abs() < 1e-9);
        // At day 5: 3 at risk (3 is censored before), 1 dies → 2/3.
        assert!((s[2].1 - 2.0 / 3.0).abs() < 1e-9);
        // At day 10: 2 at risk, 1 dies → 2/3 × 1/2.
        assert!((s[3].1 - 1.0 / 3.0).abs() < 1e-9);
        assert!((s[4].1 - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(survival(&[], &[7]), vec![(7, 1.0)]);
    }
}

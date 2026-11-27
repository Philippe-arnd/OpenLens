//! How good is the feature map, measured on real usage (#240)? A score
//! out of 100 and findings, each with its evidence and a YAML fix an agent
//! can apply: coverage (pages and clicks no feature claims), overlapping
//! routes, dead features, success events that never fire or fire on every
//! use, `data-feature` values the map does not declare, and `requires`
//! that usage contradicts. Plus the per-feature numbers (30 days against
//! the 30 before), roll-ups by area and the adoption ladder. Since #283,
//! also features cut too big or too small and steps out of order
//! (`mapshape`), routes new in the latest release that no feature claims
//! (`drift`) and the relations inferred between features (`relations`).
//!
//! Scoring (documented in docs/agents/analyze.md): 100, minus up to 40
//! for coverage (40 × the share of pages and clicks without a feature),
//! minus 10 per error and 4 per warning, never below 0.

use std::collections::{BTreeMap, HashMap, HashSet};

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, int, source, text};

/// Below this many users, rates are not judged.
const MIN_USERS: i64 = 20;
/// Findings kept (the highest impact first).
const MAX_FINDINGS: usize = 50;

/// One problem with the map, and how to fix it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    /// `coverage`, `overlap`, `dead`, `success_never`, `success_always`,
    /// `undeclared`, `requires`, `too_big`, `too_small`, `step_order`, `drift`.
    pub kind: &'static str,
    /// `error` or `warning`.
    pub level: &'static str,
    /// The feature concerned, if any.
    pub feature: Option<String>,
    /// What is wrong.
    pub message: String,
    /// The numbers behind it.
    pub evidence: String,
    /// A YAML change that fixes it (to adapt, then `features push`).
    pub fix: Option<String>,
    /// Events or users affected: findings are ranked by it.
    pub impact: i64,
}

/// One feature's numbers.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeatureHealth {
    /// Key.
    pub key: String,
    /// Name.
    pub name: String,
    /// Area, if declared.
    pub area: Option<String>,
    /// Events, last 30 days.
    pub uses: i64,
    /// Users, last 30 days.
    pub users: i64,
    /// Users who reached its success event, last 30 days.
    pub success_users: i64,
    /// Events, the 30 days before.
    pub prev_uses: i64,
    /// Users, the 30 days before.
    pub prev_users: i64,
    /// Users who reached success, the 30 days before.
    pub prev_success_users: i64,
    /// `ok`, `dead`, `no_success`, `low_sample` or `issues`.
    pub status: &'static str,
}

impl FeatureHealth {
    /// Share of users who reached success (None without a success event or users).
    #[must_use]
    pub fn success_rate(&self) -> Option<f64> {
        (self.users > 0).then(|| self.success_users as f64 / self.users as f64)
    }
}

/// An area's roll-up.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AreaHealth {
    /// Name (`(none)` for features without an area).
    pub area: String,
    /// Features in it.
    pub features: usize,
    /// Events, last 30 days.
    pub uses: i64,
    /// Distinct users of any of its features.
    pub users: i64,
}

/// One rung of the adoption ladder: of the users of `from` (the
/// prerequisite), how many went on to `to` (#240).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Rung {
    /// The feature required first.
    pub from: String,
    /// The feature that requires it.
    pub to: String,
    /// Users of `from`, last 30 days.
    pub from_users: i64,
    /// Of them, users of `to`.
    pub reached: i64,
    /// Users of `to` who never used `from` (the declared order is wrong
    /// for them).
    pub skipped: i64,
}

/// The map's health.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapHealth {
    /// 0–100.
    pub score: u8,
    /// Share of pages and clicks (30 days) that resolve to a feature.
    pub coverage: f64,
    /// Pages and clicks counted for coverage.
    pub interactions: i64,
    /// Findings, highest impact first (at most 50).
    pub findings: Vec<Finding>,
    /// Every feature, in map order.
    pub features: Vec<FeatureHealth>,
    /// Roll-ups by area (only when some feature has one).
    pub areas: Vec<AreaHealth>,
    /// The adoption ladder, from `requires`.
    pub ladder: Vec<Rung>,
    /// Relations inferred from sessions (#283), most shared sessions first.
    pub relations: Vec<crate::relations::Relation>,
    /// Days of the 60 read that count nobody (#489, D-070), oldest first.
    pub unavailable_days: Vec<String>,
}

/// Turn a path into a route pattern: ids (digits, long hex, uuids) become `:id`.
#[must_use]
pub fn route_for(path: &str) -> String {
    let segs: Vec<String> = path
        .trim_end_matches('/')
        .split('/')
        .map(|s| {
            let id = !s.is_empty()
                && (s.bytes().all(|b| b.is_ascii_digit())
                    || (s.len() >= 16 && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')));
            if id { ":id".to_owned() } else { s.to_owned() }
        })
        .collect();
    let r = segs.join("/");
    if r.is_empty() { "/".into() } else { r }
}

/// A key for a new feature from a route: `/billing/:id/invoices` → `billing-invoices`.
pub(crate) fn key_for(route: &str) -> String {
    let k: Vec<String> = route
        .split('/')
        .filter(|s| !s.is_empty() && !s.starts_with(':') && *s != "*")
        .map(|s| {
            s.to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect()
        })
        .collect();
    let k = k.join("-");
    let k = k.trim_matches('-');
    if k.is_empty() {
        "home".into()
    } else {
        k.chars().take(64).collect()
    }
}

/// What the queries return, folded in [`assess`].
#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// (feature resolved or None, current period, events, users).
    pub by_feature: Vec<(Option<String>, bool, i64, i64)>,
    /// (feature, current period, users who reached its success event).
    pub success: Vec<(String, bool, i64)>,
    /// Pages and clicks, last 30 days: (path, events, feature resolved).
    pub paths: Vec<(String, i64, bool)>,
    /// Raw `feature` values sent by the app (`data-feature`), with events.
    pub tagged: Vec<(String, i64)>,
    /// (user, feature) pairs, last 30 days, for features in a `requires`.
    pub pairs: Vec<(String, String)>,
    /// What each session did with the features (#283).
    pub sessions: crate::mapsessions::Sessions,
    /// The relations' counts (#483); `None`: taken from `sessions`.
    pub relations: Option<crate::relations::Counts>,
    /// The current release's routes (#283, D-070).
    pub drift: crate::drift::Observed,
}

fn finding(
    kind: &'static str,
    level: &'static str,
    feature: Option<&str>,
    message: String,
    evidence: String,
    fix: Option<String>,
    impact: i64,
) -> Finding {
    Finding {
        kind,
        level,
        feature: feature.map(str::to_owned),
        message,
        evidence,
        fix,
        impact,
    }
}

/// Whether a feature's success event probably fires on entry rather than
/// when the user gets value: every one of enough users reached it. Map
/// health reports it as a `success_always` finding; the board shows no
/// task time for it, since the time from entry to success is then
/// meaningless (#612).
#[must_use]
pub fn success_on_entry(users: i64, success_users: i64) -> bool {
    users >= MIN_USERS && success_users >= users
}

/// Fold measured usage into the health report (pure, so it is tested
/// without data files).
#[must_use]
pub fn assess(m: &FeatureMap, u: &Usage) -> MapHealth {
    let mut out = MapHealth::default();
    let mut findings = Vec::new();
    // Per feature.
    let mut rows: Vec<FeatureHealth> = m
        .features
        .iter()
        .map(|f| FeatureHealth {
            key: f.key.clone(),
            name: f.name.clone(),
            area: f.area.clone(),
            ..FeatureHealth::default()
        })
        .collect();
    let pos: HashMap<&str, usize> = m
        .features
        .iter()
        .enumerate()
        .map(|(i, f)| (f.key.as_str(), i))
        .collect();
    for (feat, cur, n, users) in &u.by_feature {
        let Some(i) = feat.as_deref().and_then(|k| pos.get(k)) else {
            continue;
        };
        let r = &mut rows[*i];
        if *cur {
            r.uses += n;
            r.users += users;
        } else {
            r.prev_uses += n;
            r.prev_users += users;
        }
    }
    for (feat, cur, users) in &u.success {
        let Some(r) = pos.get(feat.as_str()).map(|i| &mut rows[*i]) else {
            continue;
        };
        if *cur {
            r.success_users += users;
        } else {
            r.prev_success_users += users;
        }
    }
    let any_data = u.by_feature.iter().any(|(_, cur, n, _)| *cur && *n > 0);
    for (f, r) in m.features.iter().zip(rows.iter_mut()) {
        r.status = "ok";
        if !any_data {
            continue;
        }
        if r.uses == 0 {
            r.status = "dead";
            let had = if r.prev_uses > 0 {
                format!("{} events the 30 days before", r.prev_uses)
            } else {
                "none the 30 days before either".to_owned()
            };
            findings.push(finding(
                "dead",
                "warning",
                Some(&f.key),
                format!("`{}` was not used in 30 days", f.key),
                format!("0 events in 30 days, {had}"),
                Some(format!(
                    "# Check the wiring (routes {:?}, data-feature=\"{}\"), or remove the feature if it is gone:\n- key: {}   # delete this entry",
                    f.entry.routes, f.key, f.key
                )),
                r.prev_uses.max(1),
            ));
            continue;
        }
        let Some(ev) = &f.success else {
            r.status = "no_success";
            continue;
        };
        if r.users < MIN_USERS {
            r.status = "low_sample";
        }
        if r.success_users == 0 {
            r.status = "issues";
            findings.push(finding(
                "success_never",
                "warning",
                Some(&f.key),
                format!("the success event `{ev}` of `{}` never fired in 30 days", f.key),
                format!("{} users used the feature, 0 reached `{ev}`", r.users),
                Some(format!(
                    "# Track it where the user gets value: openlens.track(\"{ev}\")\n# or point success at an event the app sends:\n- key: {}\n  success: <event name>",
                    f.key
                )),
                r.users,
            ));
        } else if success_on_entry(r.users, r.success_users) {
            r.status = "issues";
            findings.push(finding(
                "success_always",
                "warning",
                Some(&f.key),
                format!(
                    "every user of `{}` reaches `{ev}`: it probably fires on entry, not on value",
                    f.key
                ),
                format!("{} of {} users reached it", r.success_users, r.users),
                Some(format!(
                    "# Pick the event that means the job is done, later than opening the feature:\n- key: {}\n  success: <later event>",
                    f.key
                )),
                r.users,
            ));
        }
    }
    // Coverage and unmapped pages.
    let total: i64 = u.paths.iter().map(|(_, n, _)| n).sum();
    let mapped: i64 = u
        .paths
        .iter()
        .filter(|(_, _, f)| *f)
        .map(|(_, n, _)| n)
        .sum();
    out.interactions = total;
    out.coverage = if total > 0 {
        mapped as f64 / total as f64
    } else {
        1.0
    };
    let mut unmapped: BTreeMap<String, i64> = BTreeMap::new();
    for (path, n, f) in &u.paths {
        if !f {
            *unmapped.entry(route_for(path)).or_default() += n;
        }
    }
    let drift = crate::drift::findings(&u.drift);
    // A new route is reported as drift, the more specific finding.
    unmapped.retain(|r, _| !drift.iter().any(|(d, _)| d == r));
    findings.extend(drift.into_iter().map(|(_, f)| f));
    out.relations = match &u.relations {
        Some(c) => crate::relations::from_counts(m, c),
        None => crate::relations::infer(m, &u.sessions),
    };
    findings.extend(crate::mapshape::findings(m, &u.sessions, &out.relations));
    let mut unmapped: Vec<(String, i64)> = unmapped.into_iter().collect();
    unmapped.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (route, n) in unmapped.iter().take(10) {
        if total == 0 || (*n as f64 / total as f64) < 0.01 {
            continue;
        }
        let key = key_for(route);
        findings.push(finding(
            "coverage",
            if (*n as f64 / total as f64) >= 0.1 { "error" } else { "warning" },
            None,
            format!("`{route}` is used but belongs to no feature"),
            format!(
                "{n} pages and clicks in 30 days ({:.0}% of all)",
                100.0 * *n as f64 / total as f64
            ),
            Some(format!(
                "# Add the route to the feature it belongs to, or declare it:\n- key: {key}\n  name: {}\n  entry:\n    routes: [\"{route}\"]",
                key.replace('-', " ")
            )),
            *n,
        ));
    }
    // Overlaps: a path several features claim (the first one wins).
    let mut overlaps: BTreeMap<(String, String), (i64, String)> = BTreeMap::new();
    for (path, n, _) in &u.paths {
        let fs = m.features_for_path(path);
        if fs.len() > 1 {
            for other in &fs[1..] {
                let e = overlaps
                    .entry((fs[0].to_owned(), (*other).to_owned()))
                    .or_insert((0, path.clone()));
                e.0 += n;
            }
        }
    }
    for ((a, b), (n, path)) in overlaps {
        findings.push(finding(
            "overlap",
            "warning",
            Some(&b),
            format!("routes of `{a}` and `{b}` both match `{path}`: it counts for `{a}` only"),
            format!("{n} pages and clicks in 30 days on paths both claim"),
            Some(format!(
                "# Make the routes disjoint (`*` matches everything below), or put `{b}` first:\n- key: {a}\n  entry:\n    routes: [<narrower routes>]"
            )),
            n,
        ));
    }
    // data-feature values the map does not declare.
    for (k, n) in &u.tagged {
        if m.current_key(k).is_none() && openlens_featuremap::valid_key(k) {
            findings.push(finding(
                "undeclared",
                "warning",
                Some(k),
                format!("the app tags `{k}` (data-feature) but the map has no such feature"),
                format!("{n} events in 30 days"),
                Some(format!(
                    "- key: {k}\n  name: {}\n  success: <event that means value>\n# or, if it was renamed: renamed_from: [{k}] on the new feature",
                    k.replace(['-', '_'], " ")
                )),
                *n,
            ));
        }
    }
    // Ladder and `requires` contradicted by usage.
    let mut users_of: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (user, f) in &u.pairs {
        users_of
            .entry(f.as_str())
            .or_default()
            .insert(user.as_str());
    }
    let empty = HashSet::new();
    for f in &m.features {
        let to = users_of.get(f.key.as_str()).unwrap_or(&empty);
        for r in &f.requires {
            let from = users_of.get(r.as_str()).unwrap_or(&empty);
            let reached = from.intersection(to).count() as i64;
            let skipped = to.difference(from).count() as i64;
            let rung = Rung {
                from: r.clone(),
                to: f.key.clone(),
                from_users: from.len() as i64,
                reached,
                skipped,
            };
            if to.len() as i64 >= MIN_USERS && skipped * 2 > to.len() as i64 {
                findings.push(finding(
                    "requires",
                    "warning",
                    Some(&f.key),
                    format!(
                        "most users of `{}` never used `{r}`, which it requires",
                        f.key
                    ),
                    format!("{skipped} of {} users of `{}` skipped `{r}` (30 days)", to.len(), f.key),
                    Some(format!(
                        "# Usage says `{r}` is not a step before `{}`:\n- key: {}\n  requires: []   # drop `{r}`",
                        f.key, f.key
                    )),
                    skipped,
                ));
            }
            out.ladder.push(rung);
        }
    }
    // Areas.
    if m.features.iter().any(|f| f.area.is_some()) {
        let mut areas: BTreeMap<String, (usize, i64, HashSet<&str>)> = BTreeMap::new();
        for (f, r) in m.features.iter().zip(&rows) {
            let a = areas
                .entry(f.area.clone().unwrap_or_else(|| "(none)".into()))
                .or_default();
            a.0 += 1;
            a.1 += r.uses;
            if let Some(us) = users_of.get(f.key.as_str()) {
                a.2.extend(us.iter().copied());
            }
        }
        out.areas = areas
            .into_iter()
            .map(|(area, (features, uses, us))| AreaHealth {
                area,
                features,
                uses,
                users: us.len() as i64,
            })
            .collect();
        // Without pairs (no `requires`), distinct users fall back to the
        // largest feature of the area (a lower bound).
        for a in &mut out.areas {
            if a.users == 0 {
                a.users = m
                    .features
                    .iter()
                    .zip(&rows)
                    .filter(|(f, _)| f.area.as_deref().unwrap_or("(none)") == a.area)
                    .map(|(_, r)| r.users)
                    .max()
                    .unwrap_or(0);
            }
        }
    }
    for f in &findings {
        if let Some(k) = &f.feature
            && let Some(i) = pos.get(k.as_str())
            && rows[*i].status == "ok"
        {
            rows[*i].status = "issues";
        }
    }
    findings.sort_by(|a, b| {
        (b.level == "error")
            .cmp(&(a.level == "error"))
            .then(b.impact.cmp(&a.impact))
    });
    findings.truncate(MAX_FINDINGS);
    let errors = findings.iter().filter(|f| f.level == "error").count() as f64;
    let warnings = findings.iter().filter(|f| f.level == "warning").count() as f64;
    let score = 100.0 - 40.0 * (1.0 - out.coverage) - 10.0 * errors - 4.0 * warnings;
    out.score = score.clamp(0.0, 100.0).round() as u8;
    out.findings = findings;
    out.features = rows;
    out
}

/// Measure the map on the last 60 days and assess it, every day from the
/// events (see [`map_health_in`] to read the rollups).
pub async fn map_health(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    f: &Filters,
) -> Result<MapHealth> {
    map_health_in(q, project, m, None, f).await
}

/// [`map_health`] for the map version `version`: the closed days rolled
/// up with it are read from the rollups (#483, D-054).
pub async fn map_health_in(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    version: Option<i64>,
    f: &Filters,
) -> Result<MapHealth> {
    let q = &q.with_params(&f.params()?);
    let days = crate::mapdays::MapDays::load(q, project, version).await?;
    measure(q, project, m, &days, f).await
}

/// Measure and assess, with `q` already bound to the filters' values.
async fn measure(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    days: &crate::mapdays::MapDays,
    f: &Filters,
) -> Result<MapHealth> {
    let src = source(Some(m));
    let w30 = f.clause(30)?;
    let paths = format!(
        "SELECT path, feature IS NOT NULL AS mapped, count(*) AS n FROM {src} \
         WHERE {w30} AND event IN ('$pageview', '$click') AND path IS NOT NULL \
         GROUP BY path, feature IS NOT NULL ORDER BY n DESC LIMIT 2000"
    );
    let tagged = format!(
        "SELECT feature, count(*) AS n FROM events WHERE {w30} AND feature IS NOT NULL \
         GROUP BY feature ORDER BY n DESC LIMIT 200"
    );
    let laddered: HashSet<&str> = m
        .features
        .iter()
        .filter(|x| !x.requires.is_empty())
        .flat_map(|x| {
            x.requires
                .iter()
                .map(String::as_str)
                .chain([x.key.as_str()])
        })
        .collect();
    let with_area = m.features.iter().any(|x| x.area.is_some());
    let pair_keys: Vec<String> = m
        .features
        .iter()
        .filter(|x| with_area || laddered.contains(x.key.as_str()))
        .filter_map(|x| crate::lit(&x.key).ok())
        .collect();
    let mut u = Usage::default();
    let (b, c, people) = futures::join!(
        q.sql(project, &paths),
        q.sql(project, &tagged),
        days.usage(q, project, m, f, &pair_keys, &mut u)
    );
    people?;
    for r in &b?.rows {
        if let Some(p) = text(r, "path") {
            let mapped = r
                .get("mapped")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            u.paths.push((p, int(r, "n"), mapped));
        }
    }
    for r in &c?.rows {
        if let Some(k) = text(r, "feature") {
            u.tagged.push((k, int(r, "n")));
        }
    }
    let (s, d, rel, gone) = futures::join!(
        crate::mapsessions::load(q, project, m, f),
        crate::drift::load(q, project, m, f),
        days.relations(q, project, m, f),
        days.unavailable(q, project)
    );
    (u.sessions, u.drift, u.relations) = (s?, d?, Some(rel?));
    let mut h = assess(m, &u);
    h.unavailable_days = gone?;
    Ok(h)
}

/// One feature in depth (#240): its definition, health, numbers by role,
/// top pages and where it sits on the ladder. Bounded, for agents.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeatureContext {
    /// The feature as declared.
    pub definition: Option<openlens_featuremap::Feature>,
    /// Its numbers (30 days vs the 30 before).
    pub health: Option<FeatureHealth>,
    /// Findings about it.
    pub findings: Vec<Finding>,
    /// (canonical role, users, success users), last 30 days.
    pub roles: Vec<(String, i64, i64)>,
    /// (path, events), top 10, last 30 days.
    pub pages: Vec<(String, i64)>,
    /// Rungs where it is the prerequisite or the next step.
    pub ladder: Vec<Rung>,
    /// Features that require it.
    pub required_by: Vec<String>,
    /// Other features in its area.
    pub area_peers: Vec<String>,
    /// Use per release, newest first (#411), last 90 days, each counted
    /// with the map version in force (#283).
    pub releases: Vec<FeatureRelease>,
    /// The releases whose code change touched it (#576), newest first:
    /// `(release, changed files matching its code paths)`. Empty when no
    /// release was matched (no git host link, or no `paths` declared).
    pub touched_by: Vec<(String, i64)>,
    /// Inferred relations it is part of (#283).
    pub relations: Vec<crate::relations::Relation>,
    /// Errors in the sessions that used it (#411), most users first, last 30 days.
    pub errors: Vec<FeatureError>,
    /// Latest sessions that used it (#411; the page keeps those with a replay).
    pub sessions: Vec<FeatureSession>,
    /// Flags active while it was used (#411, from `$flags`), last 30 days.
    pub flags: Vec<FeatureFlag>,
}

pub use crate::inforce::FeatureRelease;

/// An error message seen in sessions that used a feature.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeatureError {
    /// One message.
    pub message: String,
    /// Occurrences.
    pub count: i64,
    /// Users hit.
    pub users: i64,
    /// The session of the latest one.
    pub last_session: Option<String>,
}

/// A session that used a feature.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeatureSession {
    /// Session id.
    pub session_id: String,
    /// Who.
    pub distinct_id: String,
    /// When it last used the feature.
    pub at: String,
    /// It reached the success event.
    pub succeeded: bool,
}

/// A flag active while a feature was used.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeatureFlag {
    /// Flag key.
    pub flag: String,
    /// Users of the feature with it on.
    pub users: i64,
    /// Of whom reached the success event.
    pub success_users: i64,
}

/// The releases that touched `key` (#576), under it or a former key.
fn touched_by(
    history: &crate::inforce::History,
    key: &str,
    def: Option<&openlens_featuremap::Feature>,
) -> Vec<(String, i64)> {
    let names: Vec<&str> = std::iter::once(key)
        .chain(
            def.into_iter()
                .flat_map(|d| d.renamed_from.iter().map(String::as_str)),
        )
        .collect();
    history
        .touched
        .iter()
        .filter_map(|(release, fs)| {
            let f = fs.iter().find(|f| names.contains(&f.key.as_str()))?;
            Some((release.clone(), f.files))
        })
        .collect()
}

/// [`FeatureContext`] for `key` (its current key or a former one).
/// `history` holds the stored map versions and the recorded releases, for
/// the releases (#283, D-070); record what they derived with
/// [`crate::inforce::record`].
pub async fn feature_context(
    q: &QueryEngine,
    project: &str,
    m: &FeatureMap,
    history: &crate::inforce::History,
    f: &Filters,
    key: &str,
) -> Result<Option<FeatureContext>> {
    let q = &q.with_params(&f.params()?);
    let Some(key) = m.current_key(key).map(str::to_owned) else {
        return Ok(None);
    };
    let def = m.features.iter().find(|x| x.key == key).cloned();
    // The map's stored version, when `m` is one: its rolled-up days are
    // read from the rollups (#483, D-054).
    let version = history
        .versions
        .iter()
        .filter(|h| h.map == *m)
        .map(|h| h.version)
        .max();
    let days = crate::mapdays::MapDays::load(q, project, version).await?;
    let health = measure(q, project, m, &days, f).await?;
    let src = source(Some(m));
    let w30 = f.clause(30)?;
    let k = crate::lit(&key)?;
    let succ = def
        .as_ref()
        .and_then(|d| d.success.as_deref())
        .and_then(|s| crate::lit(s).ok());
    let roles = days.roles_sql(m, f, &key, succ.is_some())?;
    let pages = format!(
        "SELECT path, count(*) AS n FROM {src} WHERE {w30} AND feature = {k} AND path IS NOT NULL \
         GROUP BY path ORDER BY n DESC LIMIT 10"
    );
    let ok = succ.as_deref().unwrap_or("NULL");
    let errors = format!(
        "SELECT coalesce(error_message, '(no message)') AS msg, \
         count(*) AS n, count(DISTINCT distinct_id) AS u, last_value(session_id ORDER BY timestamp) AS last_session \
         FROM {src} WHERE {w30} AND event = '$error' AND session_id IN \
         (SELECT DISTINCT session_id FROM {src} WHERE {w30} AND feature = {k}) \
         GROUP BY 1 ORDER BY u DESC, n DESC LIMIT 10"
    );
    let sessions = format!(
        "SELECT session_id, min(distinct_id) AS who, CAST(max(timestamp) AS VARCHAR) AS at, \
         max(CASE WHEN event = {ok} THEN 1 ELSE 0 END) AS ok \
         FROM {src} WHERE {w30} AND feature = {k} AND session_id IS NOT NULL \
         GROUP BY session_id ORDER BY at DESC LIMIT 50"
    );
    let flags = format!(
        "WITH y AS (SELECT distinct_id, event, unnest(string_to_array(flags, ',')) AS flag \
           FROM {src} WHERE {w30} AND feature = {k} AND flags IS NOT NULL) \
         SELECT flag, count(DISTINCT distinct_id) AS u, \
         count(DISTINCT CASE WHEN event = {ok} THEN distinct_id END) AS s \
         FROM y WHERE flag <> '' GROUP BY flag ORDER BY u DESC LIMIT 10"
    );
    let (r, p, rel, err, ses, fl) = futures::join!(
        q.sql(project, &roles),
        q.sql(project, &pages),
        crate::inforce::releases(q, project, m, history, f, &key),
        q.sql(project, &errors),
        q.sql(project, &sessions),
        q.sql(project, &flags)
    );
    let area = def.as_ref().and_then(|d| d.area.clone());
    Ok(Some(FeatureContext {
        health: health.features.iter().find(|x| x.key == key).cloned(),
        findings: health
            .findings
            .iter()
            .filter(|x| x.feature.as_deref() == Some(key.as_str()))
            .cloned()
            .collect(),
        roles: r?
            .rows
            .iter()
            .filter_map(|x| Some((text(x, "role")?, int(x, "u"), int(x, "s"))))
            .collect(),
        pages: p?
            .rows
            .iter()
            .filter_map(|x| Some((text(x, "path")?, int(x, "n"))))
            .collect(),
        ladder: health
            .ladder
            .iter()
            .filter(|x| x.from == key || x.to == key)
            .cloned()
            .collect(),
        required_by: m
            .features
            .iter()
            .filter(|x| x.requires.contains(&key))
            .map(|x| x.key.clone())
            .collect(),
        area_peers: m
            .features
            .iter()
            .filter(|x| area.is_some() && x.area == area && x.key != key)
            .map(|x| x.key.clone())
            .collect(),
        releases: rel?,
        touched_by: touched_by(history, &key, def.as_ref()),
        relations: health
            .relations
            .iter()
            .filter(|x| x.first == key || x.then == key)
            .cloned()
            .collect(),
        errors: err?
            .rows
            .iter()
            .map(|x| FeatureError {
                // A JSON string body: undo its escapes.
                message: text(x, "msg")
                    .map(|m| serde_json::from_str(&format!("\"{m}\"")).unwrap_or(m))
                    .unwrap_or_default(),
                count: int(x, "n"),
                users: int(x, "u"),
                last_session: text(x, "last_session"),
            })
            .collect(),
        sessions: ses?
            .rows
            .iter()
            .filter_map(|x| {
                Some(FeatureSession {
                    session_id: text(x, "session_id")?,
                    distinct_id: text(x, "who").unwrap_or_default(),
                    at: text(x, "at").unwrap_or_default(),
                    succeeded: int(x, "ok") > 0,
                })
            })
            .collect(),
        flags: fl?
            .rows
            .iter()
            .filter_map(|x| {
                Some(FeatureFlag {
                    flag: text(x, "flag")?,
                    users: int(x, "u"),
                    success_users: int(x, "s"),
                })
            })
            .collect(),
        definition: def,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::field_reassign_with_default)]
    use super::*;

    const MAP: &str = "features:
  - key: docs
    name: Docs
    area: Reading
    success: doc_read
    entry: {routes: ['/docs/*']}
  - key: edit
    name: Edit
    area: Writing
    success: doc_saved
    entry: {routes: ['/docs/:id/edit']}
  - key: share
    name: Share
    area: Writing
    success: shared
    requires: [edit]
    entry: {routes: ['/share']}
  - key: old
    name: Old thing
    success: old_done
    entry: {routes: ['/old']}
";

    fn usage() -> Usage {
        let mut u = Usage::default();
        u.by_feature = vec![
            (Some("docs".into()), true, 500, 40),
            (Some("edit".into()), true, 300, 30),
            (Some("share".into()), true, 100, 25),
            (Some("old".into()), false, 12, 3),
            (None, true, 200, 20),
        ];
        u.success = vec![("docs".into(), true, 40), ("share".into(), true, 10)];
        u.paths = vec![
            ("/docs/1".into(), 400, true),
            ("/docs/1/edit".into(), 250, true),
            ("/billing/42".into(), 150, false),
            ("/billing/43".into(), 50, false),
            ("/share".into(), 150, true),
        ];
        u.tagged = vec![("export".into(), 30), ("edit".into(), 10)];
        for i in 0..25 {
            u.pairs.push((format!("u{i}"), "share".into()));
            if i < 5 {
                u.pairs.push((format!("u{i}"), "edit".into()));
            }
        }
        u
    }

    #[test]
    fn planted_defects_are_all_found_with_fixes() {
        let m = openlens_featuremap::parse(MAP).map.unwrap();
        let h = assess(&m, &usage());
        let kinds: Vec<(&str, Option<&str>)> = h
            .findings
            .iter()
            .map(|f| (f.kind, f.feature.as_deref()))
            .collect();
        for want in [
            ("coverage", None),
            ("overlap", Some("edit")),
            ("dead", Some("old")),
            ("success_never", Some("edit")),
            ("success_always", Some("docs")),
            ("undeclared", Some("export")),
            ("requires", Some("share")),
        ] {
            assert!(kinds.contains(&want), "{want:?} missing from {kinds:?}");
        }
        assert!(h.findings.iter().all(|f| f.fix.is_some()));
        // `/billing/:id` is 20% of all: an error, ranked first.
        assert_eq!(h.findings[0].kind, "coverage");
        assert!(
            h.findings[0]
                .fix
                .as_deref()
                .unwrap()
                .contains("/billing/:id")
        );
        assert!((h.coverage - 0.8).abs() < 1e-9);
        assert!(h.score < 60, "{}", h.score);
        let share = &h.ladder[0];
        assert_eq!((share.from_users, share.reached, share.skipped), (5, 5, 20));
        let writing = h.areas.iter().find(|a| a.area == "Writing").unwrap();
        assert_eq!(
            (writing.features, writing.uses, writing.users),
            (2, 400, 25)
        );
        let old = h.features.iter().find(|f| f.key == "old").unwrap();
        assert_eq!((old.status, old.prev_uses), ("dead", 12));
    }

    #[test]
    fn a_clean_map_scores_100() {
        let m = openlens_featuremap::parse(
            "features:\n  - key: a\n    name: A\n    success: done\n    entry: {routes: ['/a']}\n",
        )
        .map
        .unwrap();
        let mut u = Usage::default();
        u.by_feature = vec![(Some("a".into()), true, 100, 30)];
        u.success = vec![("a".into(), true, 12)];
        u.paths = vec![("/a".into(), 100, true)];
        let h = assess(&m, &u);
        assert_eq!(h.score, 100, "{:?}", h.findings);
        assert!(h.areas.is_empty());
        assert_eq!(h.features[0].success_rate(), Some(0.4));
    }

    #[test]
    fn routes_and_keys_are_suggested_from_paths() {
        assert_eq!(route_for("/billing/42/invoices/"), "/billing/:id/invoices");
        assert_eq!(
            route_for("/d/0f8fad5b-d9cb-469f-a165-70867728950e"),
            "/d/:id"
        );
        assert_eq!(route_for("/"), "/");
        assert_eq!(key_for("/billing/:id/invoices"), "billing-invoices");
        assert_eq!(key_for("/"), "home");
    }
}

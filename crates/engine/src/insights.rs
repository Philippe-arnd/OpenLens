//! Insight cards for the Focus board (docs/method.md, step 3).
//!
//! Each card is computed by one SQL pass over the (map-resolved) events,
//! carries the cohort sizes behind its numbers, a suggested next step, and
//! a score used to rank the board. Cards below the minimum cohort sizes are
//! not shown: no insight is better than a misleading one.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{Filters, Result, int, text};
use openlens_query::QueryEngine;

/// Smallest group size an insight may rest on.
pub const MIN_GROUP: i64 = 8;

/// One insight card.
#[derive(Debug, Clone, Serialize)]
pub struct Insight {
    /// Kind: stick_driver, value_moment, churn_point, friction, role_blind_spot, release_regression, slow_pages.
    pub kind: &'static str,
    /// Headline in plain language.
    pub title: String,
    /// The numbers behind it.
    pub evidence: String,
    /// What to do about it.
    pub next_step: String,
    /// Ranking score (higher first).
    pub score: f64,
    /// Feature, role or release the card is about.
    pub subject: String,
}

/// Why the board has no (or few) insights yet.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Readiness {
    /// Users old enough to judge retention (first seen before the churn window).
    pub judged_users: i64,
    /// Needed for retention insights.
    pub needed: i64,
}

fn pct(r: f64) -> String {
    format!("{:.0}%", r * 100.0)
}

fn rows(
    r: std::result::Result<openlens_query::QueryResult, openlens_query::QueryError>,
) -> Result<Vec<Map<String, Value>>> {
    Ok(r?.rows)
}

/// Compute all insights. `src` is the event source (raw or map-resolved),
/// `w90` the filter clause over the analysis window, `churn` the window in days.
/// Events of the last 90 days and the cohort judged for retention:
/// identified users first seen inside the window but before the churn
/// window, `retained` = seen within it. "First seen" is the first
/// `$identify` ever, so a returning user is not taken for a new one (their
/// "first week" would be wrong); without identify, users already active on
/// the window's first day are left out for the same reason.
pub(crate) fn cohort_base(src: &str, w: &str, ident: &str, churn: i64) -> String {
    format!(
        "WITH ev AS (SELECT distinct_id, timestamp, feature, event, path, release, session_id FROM {src} WHERE {w} AND {ident}), \
         users AS (SELECT ev.distinct_id, least(min(ev.timestamp), min(p.first_seen)) AS first_seen, max(ev.timestamp) AS last_seen \
                   FROM ev LEFT JOIN persons p ON ev.distinct_id = p.person GROUP BY ev.distinct_id), \
         cohort AS (SELECT distinct_id, first_seen, last_seen, \
                    CASE WHEN last_seen >= now() - INTERVAL '{churn} days' THEN 1 ELSE 0 END AS retained \
                    FROM users WHERE first_seen <= now() - INTERVAL '{churn} days' \
                    AND first_seen > now() - INTERVAL '89 days')"
    )
}

pub(crate) async fn compute(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    ident: &str,
    churn: i64,
    successes: &[(String, String)],
) -> Result<(Vec<Insight>, Readiness)> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause(90)?;
    let base = cohort_base(src, &w, ident, churn);
    let totals_sql = format!("{base} SELECT count(*) AS n, sum(retained) AS kept FROM cohort");
    let early_sql = format!(
        "{base}, early AS (SELECT e.distinct_id, e.feature, count(*) AS uses FROM ev e JOIN cohort c ON e.distinct_id = c.distinct_id \
           WHERE e.feature IS NOT NULL AND e.timestamp <= c.first_seen + INTERVAL '7 days' GROUP BY e.distinct_id, e.feature) \
         SELECT early.feature AS feature, least(early.uses, 10) AS bucket, count(*) AS users, sum(c.retained) AS kept \
         FROM early JOIN cohort c ON early.distinct_id = c.distinct_id GROUP BY early.feature, least(early.uses, 10)"
    );
    let churn_sql = format!(
        "{base}, churned AS (SELECT distinct_id, last_seen FROM cohort WHERE retained = 0) \
         SELECT coalesce(e.feature, e.path) AS place, count(DISTINCT e.distinct_id) AS users \
         FROM ev e JOIN churned ch ON e.distinct_id = ch.distinct_id AND e.timestamp = ch.last_seen \
         GROUP BY coalesce(e.feature, e.path) ORDER BY users DESC LIMIT 5"
    );
    let reach_sql = format!(
        "{base} SELECT coalesce(feature, path) AS place, count(DISTINCT distinct_id) AS users FROM ev GROUP BY coalesce(feature, path)"
    );
    let w_roles = Filters {
        role: None,
        ..f.clone()
    }
    .clause(30)?;
    let roles_sql = format!(
        "SELECT role, feature, count(DISTINCT distinct_id) AS users FROM {src} WHERE {w_roles} \
         AND role IS NOT NULL AND feature IS NOT NULL GROUP BY role, feature"
    );
    let role_totals_sql = format!(
        "SELECT role, count(DISTINCT distinct_id) AS users FROM {src} WHERE {w_roles} \
         AND role IS NOT NULL GROUP BY role"
    );
    let release_sql = format!(
        "SELECT release, count(DISTINCT session_id) AS sessions, \
         count(DISTINCT CASE WHEN event = '$error' THEN session_id END) AS bad, min(timestamp) AS first \
         FROM {src} WHERE {} AND release IS NOT NULL GROUP BY release ORDER BY first DESC LIMIT 2",
        Filters {
            release: None,
            ..f.clone()
        }
        .clause(90)?
    );
    let friction_sql = if successes.is_empty() {
        None
    } else {
        let cond = successes
            .iter()
            .map(|(k, e)| {
                format!(
                    "(feature = {} AND event = {})",
                    openlens_sqlgen::quote(k),
                    openlens_sqlgen::quote(e)
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        Some(format!(
            "SELECT feature, count(DISTINCT distinct_id) AS started, \
             count(DISTINCT CASE WHEN {cond} THEN distinct_id END) AS succeeded \
             FROM {src} WHERE {} AND feature IS NOT NULL GROUP BY feature",
            f.clause(30)?
        ))
    };

    let friction_fut = async {
        match &friction_sql {
            Some(s) => Some(q.sql(project, s).await),
            None => None,
        }
    };
    let (totals, early, churned, reach, roles, role_totals, releases, friction) = futures::join!(
        q.sql(project, &totals_sql),
        q.sql(project, &early_sql),
        q.sql(project, &churn_sql),
        q.sql(project, &reach_sql),
        q.sql(project, &roles_sql),
        q.sql(project, &role_totals_sql),
        q.sql(project, &release_sql),
        friction_fut,
    );

    let mut cards = Vec::new();
    let t = rows(totals)?;
    let n_all = t.first().map_or(0, |r| int(r, "n"));
    let kept_all = t.first().map_or(0, |r| int(r, "kept"));
    let readiness = Readiness {
        judged_users: n_all,
        needed: 2 * MIN_GROUP,
    };

    // Stick drivers and the value moment.
    if n_all >= 2 * MIN_GROUP {
        let early = rows(early)?;
        let mut features: Vec<String> = early.iter().filter_map(|r| text(r, "feature")).collect();
        features.sort();
        features.dedup();
        for feat in features {
            let at_least = |k: i64| -> (i64, i64) {
                early
                    .iter()
                    .filter(|r| {
                        text(r, "feature").as_deref() == Some(feat.as_str())
                            && int(r, "bucket") >= k
                    })
                    .fold((0, 0), |(u, kept), r| {
                        (u + int(r, "users"), kept + int(r, "kept"))
                    })
            };
            let (with, kept_with) = at_least(1);
            let (without, kept_without) = (n_all - with, kept_all - kept_with);
            if with < MIN_GROUP || without < MIN_GROUP {
                continue;
            }
            let (ra, rb) = (
                kept_with as f64 / with as f64,
                kept_without as f64 / without as f64,
            );
            if ra > rb + 0.1 {
                let lift = if rb > 0.0 {
                    format!("{:.1}×", ra / rb)
                } else {
                    "far".into()
                };
                cards.push(Insight {
                    kind: "stick_driver",
                    title: format!("Users who use {feat} in their first week stay {lift} more"),
                    evidence: format!(
                        "{} stay with it ({with} users) vs {} without ({without} users)",
                        pct(ra),
                        pct(rb)
                    ),
                    next_step: format!(
                        "Bring new users to {feat} during onboarding, and make it easy to find."
                    ),
                    score: (ra - rb) * (with.min(without) as f64).sqrt() * 2.0,
                    subject: feat.clone(),
                });
                // Value moment: the repetition threshold with the best gap.
                let mut best: Option<(i64, f64, f64, i64)> = None;
                for k in [2, 3, 5] {
                    let (wk, kk) = at_least(k);
                    let (ok, okk) = (n_all - wk, kept_all - kk);
                    if wk < MIN_GROUP || ok < MIN_GROUP {
                        continue;
                    }
                    let (a, b) = (kk as f64 / wk as f64, okk as f64 / ok as f64);
                    if best.is_none_or(|(_, ba, bb, _)| a - b > ba - bb) {
                        best = Some((k, a, b, wk));
                    }
                }
                if let Some((k, a, b, wk)) = best
                    && a - b > ra - rb
                {
                    cards.push(Insight {
                        kind: "value_moment",
                        title: format!("Value moment: {feat} {k}+ times in week 1"),
                        evidence: format!(
                            "{} of those {wk} users stay, vs {} of everyone else",
                            pct(a),
                            pct(b)
                        ),
                        next_step: format!(
                            "Design the first week so new users reach {feat} {k} times."
                        ),
                        score: (a - b) * (wk as f64).sqrt() * 2.2,
                        subject: feat.clone(),
                    });
                }
            }
        }
    }

    // Churn points: where churned users were last seen, against how many users visit it at all.
    let churned = rows(churned)?;
    let reach = rows(reach)?;
    let n_churned = n_all - kept_all;
    let n_users: i64 = reach
        .iter()
        .map(|r| int(r, "users"))
        .max()
        .unwrap_or(0)
        .max(1);
    if n_churned >= MIN_GROUP
        && let Some(top) = churned.first()
    {
        let place = text(top, "place").unwrap_or_default();
        let share = int(top, "users") as f64 / n_churned as f64;
        let visits = reach
            .iter()
            .find(|r| text(r, "place").as_deref() == Some(place.as_str()))
            .map_or(0, |r| int(r, "users"));
        let base_share = visits as f64 / n_users as f64;
        if share >= 0.25 && share > base_share * 1.2 && int(top, "users") >= MIN_GROUP {
            cards.push(Insight {
                kind: "churn_point",
                title: format!("{} of churned users were last seen in {place}", pct(share)),
                evidence: format!(
                    "{} of {n_churned} churned users; only {} of all users ever go there",
                    int(top, "users"),
                    pct(base_share)
                ),
                next_step: format!(
                    "Look at what happens in {place}: errors, confusing steps, or a pricing wall."
                ),
                score: share * (n_churned as f64).sqrt() * 1.8,
                subject: place,
            });
        }
    }

    // Role blind spots: a feature most roles adopt, one role never touches.
    let roles = rows(roles)?;
    let role_users: Vec<(String, i64)> = rows(role_totals)?
        .iter()
        .filter_map(|r| Some((text(r, "role")?, int(r, "users"))))
        .collect();
    let total_role_users: i64 = role_users.iter().map(|(_, n)| n).sum();
    let mut feats: Vec<String> = roles.iter().filter_map(|r| text(r, "feature")).collect();
    feats.sort();
    feats.dedup();
    for feat in &feats {
        let used = |role: &str| {
            roles
                .iter()
                .find(|r| {
                    text(r, "role").as_deref() == Some(role)
                        && text(r, "feature").as_deref() == Some(feat.as_str())
                })
                .map_or(0, |r| int(r, "users"))
        };
        let overall: i64 = role_users.iter().map(|(r, _)| used(r)).sum();
        for (role, n) in &role_users {
            if *n < MIN_GROUP || role.starts_with("unmatched:") {
                continue;
            }
            let others = total_role_users - n;
            let others_used = overall - used(role);
            if others < MIN_GROUP {
                continue;
            }
            let (mine, theirs) = (
                used(role) as f64 / *n as f64,
                others_used as f64 / others as f64,
            );
            if theirs >= 0.3 && mine <= theirs * 0.2 {
                cards.push(Insight {
                    kind: "role_blind_spot",
                    title: format!("{role}s almost never use {feat}"),
                    evidence: format!("{} of {n} {role}s vs {} of other users", pct(mine), pct(theirs)),
                    next_step: format!("Do {role}s need {feat}? If yes, surface it in their screens; if no, hide it for them."),
                    score: (theirs - mine) * (*n as f64).sqrt() * 1.2,
                    subject: format!("{role}:{feat}"),
                });
            }
        }
    }

    // Release regression.
    let rel = rows(releases)?;
    if let [new, old] = rel.as_slice() {
        let rate = |r: &Map<String, Value>| int(r, "bad") as f64 / int(r, "sessions").max(1) as f64;
        let (rn, ro) = (rate(new), rate(old));
        if int(new, "sessions") >= 2 * MIN_GROUP && rn > ro * 1.5 && rn - ro > 0.03 {
            let name = text(new, "release").unwrap_or_default();
            cards.push(Insight {
                kind: "release_regression",
                title: format!(
                    "Release {name}: sessions with errors went from {} to {}",
                    pct(ro),
                    pct(rn)
                ),
                evidence: format!(
                    "{} sessions on {name}, {} on {}",
                    int(new, "sessions"),
                    int(old, "sessions"),
                    text(old, "release").unwrap_or_default()
                ),
                next_step: format!(
                    "Check the errors introduced by {name} (Events page, filter by release)."
                ),
                score: (rn - ro) * (int(new, "sessions") as f64).sqrt() * 2.5,
                subject: name,
            });
        }
    }

    // Friction: started a feature, rarely reached its success event.
    if let Some(fr) = friction {
        for r in fr?.rows {
            let (feat, started, done) = (
                text(&r, "feature").unwrap_or_default(),
                int(&r, "started"),
                int(&r, "succeeded"),
            );
            if !successes.iter().any(|(k, _)| *k == feat) || started < MIN_GROUP {
                continue;
            }
            let rate = done as f64 / started as f64;
            if rate < 0.5 {
                cards.push(Insight {
                    kind: "friction",
                    title: format!("Only {} of people who start {feat} finish it", pct(rate)),
                    evidence: format!(
                        "{done} of {started} users reached its success event in 30 days"
                    ),
                    next_step: format!(
                        "Walk through {feat} as a new user: find the step where people give up."
                    ),
                    score: (1.0 - rate) * (started as f64).sqrt(),
                    subject: feat,
                });
            }
        }
    }

    cards.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok((cards, readiness))
}

/// Slow pages, from the vitals already on the board. The next step is the
/// vital's first action (#408), after its dominant cause when known.
pub(crate) fn slow_pages(vitals: &[crate::Vital], why: &[crate::VitalReport]) -> Option<Insight> {
    let v = vitals
        .iter()
        .find(|v| v.rating == "poor" && v.samples >= MIN_GROUP)?;
    let report = why.iter().find(|r| r.name == v.name);
    let first = report
        .and_then(crate::VitalReport::first_action)
        .or_else(|| {
            crate::vitals_guide(&v.name)
                .and_then(|g| g.causes.first())
                .and_then(|c| c.actions.first().copied())
        })
        .unwrap_or("Trim images, scripts and blocking requests.");
    // The guilty element and its page when the SDK named one (#467), else
    // the worst page.
    let worst = match report.and_then(|r| r.elements.first()) {
        Some(c) => format!(" Start with {} on {}.", c.element, c.path),
        None => report
            .and_then(|r| r.worst_pages.first())
            .map_or_else(String::new, |p| format!(" Start with {}.", p.path)),
    };
    Some(Insight {
        kind: "slow_pages",
        title: format!(
            "Pages feel slow: {} p75 is {}",
            v.name,
            crate::vitals::show(&v.name, v.p75)
        ),
        evidence: format!("{} measurements in the last 7 days, rated poor", v.samples),
        next_step: format!("{first}{worst}"),
        score: (v.samples as f64).sqrt() * 0.5,
        subject: v.name.clone(),
    })
}

/// A frequent path into a success event.
#[derive(Debug, Clone, Serialize)]
pub struct Journey {
    /// The success event the path leads to.
    pub success: String,
    /// The feature this success belongs to.
    pub feature: String,
    /// Up to 3 places (feature, else page path) before the first success, oldest first.
    pub path: Vec<String>,
    /// Users who took this path.
    pub users: i64,
    /// Share of the users who reached the success.
    pub share: f64,
}

/// Top 3 paths into the first success of each feature (at most 4 features):
/// the last three distinct places a user visited in the day before reaching
/// it for the first time.
pub(crate) async fn journeys(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    successes: &[(String, String)],
) -> Result<Vec<Journey>> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause(30)?;
    let mut out = Vec::new();
    let runs = successes.iter().take(4).map(|(feature, ev)| {
        let ev_lit = openlens_sqlgen::quote(ev);
        let sql = format!(
            "WITH ev AS (SELECT distinct_id, timestamp, event, coalesce(feature, path) AS place FROM {src} WHERE {w}), \
             firsts AS (SELECT distinct_id, min(timestamp) AS t FROM ev WHERE event = {ev_lit} GROUP BY distinct_id), \
             before AS (SELECT e.distinct_id, e.timestamp, e.place FROM ev e JOIN firsts f ON e.distinct_id = f.distinct_id \
                        WHERE e.timestamp < f.t AND e.timestamp >= f.t - INTERVAL '1 day' AND e.place IS NOT NULL \
                        AND e.event <> {ev_lit}), \
             dedup AS (SELECT distinct_id, timestamp, place, \
                       lag(place) OVER (PARTITION BY distinct_id ORDER BY timestamp) AS prev FROM before), \
             ranked AS (SELECT distinct_id, place, \
                        row_number() OVER (PARTITION BY distinct_id ORDER BY timestamp DESC) AS rn \
                        FROM dedup WHERE prev IS NULL OR prev <> place) \
             SELECT p3.place AS a, p2.place AS b, p1.place AS c, count(*) AS users, \
                    (SELECT count(*) FROM firsts) AS reached \
             FROM ranked p1 \
             LEFT JOIN ranked p2 ON p1.distinct_id = p2.distinct_id AND p2.rn = 2 \
             LEFT JOIN ranked p3 ON p1.distinct_id = p3.distinct_id AND p3.rn = 3 \
             WHERE p1.rn = 1 GROUP BY p3.place, p2.place, p1.place ORDER BY users DESC LIMIT 3"
        );
        async move { (feature.clone(), ev.clone(), q.sql(project, &sql).await) }
    });
    for (feature, ev, res) in futures::future::join_all(runs).await {
        for r in res?.rows {
            let users = int(&r, "users");
            let reached = int(&r, "reached").max(1);
            if users < 3 {
                continue;
            }
            let path: Vec<String> = ["a", "b", "c"].iter().filter_map(|k| text(&r, k)).collect();
            out.push(Journey {
                success: ev.clone(),
                feature: feature.clone(),
                path,
                users,
                share: users as f64 / reached as f64,
            });
        }
    }
    Ok(out)
}

/// Session KPIs of one side of a flag.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CohortStats {
    /// Distinct users.
    pub users: i64,
    /// Sessions.
    pub sessions: i64,
    /// Mean duration of non-bounce sessions, seconds.
    pub avg_secs: f64,
    /// Feature uses per session.
    pub tasks_per_session: f64,
    /// Share of sessions with an error.
    pub error_rate: f64,
    /// Standard deviation of feature uses per session.
    pub tasks_sd: f64,
}

/// Users with a flag on, against everyone else (last 30 days).
#[derive(Debug, Clone, Serialize)]
pub struct FlagCohort {
    /// Flag key.
    pub flag: String,
    /// Sessions where the flag was on.
    pub with: CohortStats,
    /// All other sessions.
    pub without: CohortStats,
    /// p-value of the difference in sessions with errors (`None`: too few sessions).
    pub p_errors: Option<f64>,
    /// p-value of the difference in tasks per session.
    pub p_tasks: Option<f64>,
}

fn valid_flag(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Compare the sessions of each flag seen lately (at most 5) with the rest.
pub(crate) async fn flag_cohorts(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
) -> Result<Vec<FlagCohort>> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause(30)?;
    let seen = q
        .sql(
            project,
            &format!("SELECT DISTINCT flags FROM {src} WHERE {w} AND flags IS NOT NULL LIMIT 200"),
        )
        .await?;
    let mut keys: Vec<String> = seen
        .rows
        .iter()
        .filter_map(|r| text(r, "flags"))
        .flat_map(|s| s.split(',').map(str::to_owned).collect::<Vec<_>>())
        .filter(|k| valid_flag(k))
        .collect();
    keys.sort();
    keys.dedup();
    keys.truncate(5);
    let runs = keys.iter().map(|key| {
        // `key` is restricted to [a-z0-9_-], so it is safe inside the literal.
        let sql = format!(
            "SELECT has, count(DISTINCT uid) AS users, count(*) AS sessions, \
             avg(CASE WHEN n > 1 THEN secs END) AS avg_secs, avg(CAST(tasks AS DOUBLE)) AS tps, stddev(CAST(tasks AS DOUBLE)) AS tps_sd, \
             avg(CASE WHEN errors > 0 THEN 1.0 ELSE 0.0 END) AS err \
             FROM (SELECT session_id, min(distinct_id) AS uid, \
                   max(CASE WHEN strpos(concat(',', coalesce(flags, ''), ','), ',{key},') > 0 THEN 1 ELSE 0 END) AS has, \
                   count(*) AS n, date_part('epoch', max(timestamp)) - date_part('epoch', min(timestamp)) AS secs, \
                   sum(CASE WHEN event = '$error' THEN 1 ELSE 0 END) AS errors, \
                   sum(CASE WHEN feature IS NOT NULL THEN 1 ELSE 0 END) AS tasks \
                   FROM {src} WHERE {w} AND session_id IS NOT NULL GROUP BY session_id) s GROUP BY has"
        );
        async move { (key.clone(), q.sql(project, &sql).await) }
    });
    let mut out = Vec::new();
    for (key, res) in futures::future::join_all(runs).await {
        let mut c = FlagCohort {
            flag: key,
            with: CohortStats::default(),
            without: CohortStats::default(),
            p_errors: None,
            p_tasks: None,
        };
        for r in res?.rows {
            let stats = CohortStats {
                users: int(&r, "users"),
                sessions: int(&r, "sessions"),
                avg_secs: r.get("avg_secs").and_then(Value::as_f64).unwrap_or(0.0),
                tasks_per_session: r.get("tps").and_then(Value::as_f64).unwrap_or(0.0),
                error_rate: r.get("err").and_then(Value::as_f64).unwrap_or(0.0),
                tasks_sd: r.get("tps_sd").and_then(Value::as_f64).unwrap_or(0.0),
            };
            if int(&r, "has") == 1 {
                c.with = stats
            } else {
                c.without = stats
            }
        }
        let (w, o) = (&c.with, &c.without);
        c.p_errors =
            crate::stats::two_proportions(w.error_rate, w.sessions, o.error_rate, o.sessions);
        c.p_tasks = crate::stats::two_means(
            w.tasks_per_session,
            w.tasks_sd,
            w.sessions,
            o.tasks_per_session,
            o.tasks_sd,
            o.sessions,
        );
        out.push(c);
    }
    Ok(out)
}

/// Cards for flags whose cohort clearly differs from everyone else.
pub(crate) fn flag_cards(cohorts: &[FlagCohort]) -> Vec<Insight> {
    let mut cards = Vec::new();
    for c in cohorts {
        let (w, o) = (&c.with, &c.without);
        if w.users < MIN_GROUP || o.users < MIN_GROUP {
            continue;
        }
        let sig = |p: Option<f64>| p.is_some_and(|p| p < crate::stats::ALPHA);
        if w.error_rate > o.error_rate * 1.5
            && w.error_rate - o.error_rate > 0.03
            && sig(c.p_errors)
        {
            cards.push(Insight {
                kind: "flag_regression",
                title: format!(
                    "Flag {}: sessions with errors {} vs {} without it",
                    c.flag,
                    pct(w.error_rate),
                    pct(o.error_rate)
                ),
                evidence: format!(
                    "{} users / {} sessions with the flag, {} / {} without (p = {:.3})",
                    w.users,
                    w.sessions,
                    o.users,
                    o.sessions,
                    c.p_errors.unwrap_or(1.0)
                ),
                next_step: format!(
                    "Hold the {} rollout and check its errors (Events, filter $error).",
                    c.flag
                ),
                score: (w.error_rate - o.error_rate) * (w.sessions as f64).sqrt() * 2.5,
                subject: c.flag.clone(),
            });
        } else if o.tasks_per_session > 0.0
            && w.tasks_per_session > o.tasks_per_session * 1.2
            && sig(c.p_tasks)
        {
            cards.push(Insight {
                kind: "flag_win",
                title: format!(
                    "Flag {}: {:.0}% more tasks per session",
                    c.flag,
                    (w.tasks_per_session / o.tasks_per_session - 1.0) * 100.0
                ),
                evidence: format!(
                    "{:.2} vs {:.2} tasks per session ({} vs {} users, p = {:.3})",
                    w.tasks_per_session,
                    o.tasks_per_session,
                    w.users,
                    o.users,
                    c.p_tasks.unwrap_or(1.0)
                ),
                next_step: format!("Widen the {} rollout and keep watching errors.", c.flag),
                score: (w.tasks_per_session / o.tasks_per_session - 1.0) * (w.users as f64).sqrt(),
                subject: c.flag.clone(),
            });
        }
    }
    cards
}

/// One step of a funnel.
#[derive(Debug, Clone, Serialize)]
pub struct FunnelStep {
    /// Event name.
    pub event: String,
    /// Users who reached this step in order.
    pub users: i64,
    /// Share of the first step's users.
    pub conversion: f64,
    /// Median seconds from the previous step (0 for the first).
    pub median_secs: f64,
}

/// A feature's declared steps as a funnel (last 30 days, each step within
/// a day of the first, in order).
#[derive(Debug, Clone, Serialize)]
pub struct Funnel {
    /// Feature key.
    pub feature: String,
    /// The steps.
    pub steps: Vec<FunnelStep>,
}

/// Funnels for up to 4 features that declare 2–8 steps (success appended
/// when it is not the last step already).
pub(crate) async fn funnels(
    q: &QueryEngine,
    project: &str,
    src: &str,
    f: &Filters,
    defs: &[(String, Vec<String>)],
) -> Result<Vec<Funnel>> {
    let q = &q.with_params(&f.params()?);
    let w = f.clause(30)?;
    let runs = defs.iter().take(4).filter(|(_, s)| (2..=8).contains(&s.len())).map(|(feature, steps)| {
        let lit = |e: &str| openlens_sqlgen::quote(e);
        // Each step carries the first step's time (t0) so every later step
        // must happen within a day of starting.
        // Only the steps' events are read (#502): no step looks at others.
        let names = steps.iter().map(|s| lit(s)).collect::<Vec<_>>().join(", ");
        let mut ctes = vec![format!(
            "ev AS (SELECT distinct_id, timestamp, event FROM {src} WHERE {w} AND event IN ({names})), \
             s0 AS (SELECT distinct_id, min(timestamp) AS t, min(timestamp) AS t0 FROM ev WHERE event = {} GROUP BY distinct_id)",
            lit(&steps[0])
        )];
        for (i, step) in steps.iter().enumerate().skip(1) {
            ctes.push(format!(
                "s{i} AS (SELECT e.distinct_id, min(e.timestamp) AS t, min(p.t0) AS t0 FROM ev e \
                 JOIN s{p} p ON e.distinct_id = p.distinct_id \
                 WHERE e.event = {} AND e.timestamp >= p.t AND e.timestamp <= p.t0 + INTERVAL '1 day' \
                 GROUP BY e.distinct_id)",
                lit(step),
                p = i - 1
            ));
        }
        let selects: Vec<String> = (0..steps.len())
            .map(|i| {
                if i == 0 {
                    "SELECT 0 AS step, count(*) AS users, CAST(0.0 AS DOUBLE) AS med FROM s0".to_owned()
                } else {
                    format!(
                        "SELECT {i} AS step, count(*) AS users, \
                         approx_median(date_part('epoch', s{i}.t) - date_part('epoch', s{p}.t)) AS med \
                         FROM s{i} JOIN s{p} ON s{i}.distinct_id = s{p}.distinct_id",
                        p = i - 1
                    )
                }
            })
            .collect();
        let sql = format!("WITH {} {} ORDER BY step", ctes.join(", "), selects.join(" UNION ALL "));
        async move { (feature.clone(), steps.clone(), q.sql(project, &sql).await) }
    });
    let mut out = Vec::new();
    for (feature, steps, res) in futures::future::join_all(runs).await {
        let rows = res?.rows;
        let first = rows.first().map_or(0, |r| int(r, "users")).max(1);
        let steps = steps
            .iter()
            .enumerate()
            .map(|(i, ev)| {
                let r = rows.iter().find(|r| int(r, "step") == i as i64);
                let users = r.map_or(0, |r| int(r, "users"));
                FunnelStep {
                    event: ev.clone(),
                    users,
                    conversion: users as f64 / first as f64,
                    median_secs: r
                        .and_then(|r| r.get("med"))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                }
            })
            .collect();
        out.push(Funnel { feature, steps });
    }
    Ok(out)
}

/// A card for the biggest drop-off in a funnel with enough users.
pub(crate) fn funnel_cards(funnels: &[Funnel]) -> Vec<Insight> {
    let mut cards = Vec::new();
    for fun in funnels {
        let Some(first) = fun.steps.first() else {
            continue;
        };
        if first.users < MIN_GROUP {
            continue;
        }
        let worst = fun
            .steps
            .windows(2)
            .map(|w| {
                (
                    w[1].event.clone(),
                    w[0].event.clone(),
                    if w[0].users > 0 {
                        1.0 - w[1].users as f64 / w[0].users as f64
                    } else {
                        0.0
                    },
                    w[0].users,
                )
            })
            .max_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((to, from, drop, base)) = worst
            && drop >= 0.4
        {
            cards.push(Insight {
                kind: "funnel_drop",
                title: format!("{}: {} of users stop between {from} and {to}", fun.feature, pct(drop)),
                evidence: format!("{base} reached {from}; {} of the {} who started finished", pct(fun.steps.last().map_or(0.0, |s| s.conversion)), first.users),
                next_step: format!("Look at the {from} → {to} step of {}: what is missing, slow or confusing there?", fun.feature),
                score: drop * (base as f64).sqrt() * 1.4,
                subject: format!("{}:{to}", fun.feature),
            });
        }
    }
    cards
}

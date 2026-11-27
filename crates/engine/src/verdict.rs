//! The verdict of a release (#522, part 5 of #237, D-067): once a release
//! has enough traffic, or is a day old, it is judged **healthy**,
//! **regressed** or **inconclusive** against the release before it in the
//! same environment.
//!
//! [`compare`] is the one release-versus-release comparison: the Releases
//! page, `releases.list`, `releases.report`, the digest and the board's
//! *release regression* card (which the hourly alerts turn into an
//! incident, #227) all read it. The card uses its error rule,
//! [`errors_regressed`], unchanged.
//!
//! What is compared ([`crate::verdict_read`] reads it):
//! - **KPIs**: sessions with an error (the card's rule), bounce rate and
//!   the share of sessions with a completed task (a two-proportion test at
//!   [`stats::ALPHA`] and at least [`MIN_MOVE`]); sessions per user, shown
//!   only (traffic follows the rollout, not the release).
//! - **Errors**: error groups first seen in the release, hitting at least
//!   [`NEW_ERROR_USERS`] users (as the hourly alert counts a new error).
//! - **Web Vitals**: LCP, INP and CLS at p75, worse when the rating drops
//!   (good, needs improvement, poor).
//! - **Features**: the success rate of each feature with a success
//!   event, worse when it drops by [`MIN_FEATURE_DROP`] or more,
//!   significantly. The features the release's code change touched
//!   ([`Side::touched`], #576, D-095) are judged and listed first, before
//!   every other check, with the same thresholds; one that cannot be
//!   compared says so.
//! - **Goals and guardrails** (production): a guardrail breached by the
//!   release and not by the one before is a regression; a goal's KPI moving
//!   toward or away from its target is shown, not judged.
//! - **Server routes** (#594, D-083): a route's 5xx share and p95, when it
//!   has enough requests on both releases ([`crate::routes::checks`]).
//! - **Runtime** (#610, D-087): process restarts per day, judged only when
//!   they go up significantly (a crash loop, [`crate::runtime::check`]);
//!   the largest RSS, its growth and the event-loop delay p99 are shown in
//!   [`Report::runtime`], never judged.
//! - **Outgoing dependencies** (#608, D-090): a host that fails more or
//!   got slower is listed in [`Report::dependencies`], shown, never
//!   judged: a dependency's trouble is usually not the release's fault.

use serde::Serialize;

use crate::goals::{Goal, Guardrail, kpi_info};
use crate::insights::MIN_GROUP;
use crate::stats::{self, ALPHA, MIN_SESSIONS};
use crate::vitals;

/// Hours after which a release is judged whatever its traffic.
pub const DUE_HOURS: i64 = 24;
/// Users a release needs (with [`MIN_SESSIONS`] sessions) to be judged
/// before [`DUE_HOURS`].
pub const MIN_USERS: i64 = MIN_GROUP;
/// Users a new error group must hit to count (the hourly alert's rule).
pub const NEW_ERROR_USERS: i64 = 2;
/// Smallest move of a share (bounce, tasks) that counts as worse.
pub const MIN_MOVE: f64 = 0.05;
/// Smallest drop of a feature's success rate that counts as worse.
pub const MIN_FEATURE_DROP: f64 = 0.10;
/// The vitals judged.
pub const VITALS: [&str; 3] = ["LCP", "INP", "CLS"];

/// What a release is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Nothing got worse than the release before, with enough traffic on
    /// both.
    Healthy,
    /// Something got worse: the reasons say what.
    Regressed,
    /// Too little traffic, or no earlier release to compare with.
    Inconclusive,
}

impl Verdict {
    /// As shown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Regressed => "regressed",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// One Web Vital of a release.
#[derive(Debug, Clone, Default, Serialize)]
pub struct VitalP75 {
    /// LCP, INP or CLS.
    pub name: String,
    /// p75.
    pub p75: f64,
    /// Measurements.
    pub samples: i64,
}

/// One feature with a success event, in one release.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeatureUse {
    /// Feature key.
    pub feature: String,
    /// Sessions that used it.
    pub sessions: i64,
    /// Of those, sessions that reached its success event.
    pub succeeded: i64,
}

/// An error group first seen in a release.
#[derive(Debug, Clone, Default, Serialize)]
pub struct NewError {
    /// Fingerprint (normalised message @ source file), as on the Errors page.
    pub fingerprint: String,
    /// One real message.
    pub sample: String,
    /// Distinct users hit in the release.
    pub users: i64,
    /// Occurrences in the release.
    pub count: i64,
    /// Raised on the server (`$kind: server`, #593).
    #[serde(default)]
    pub server: bool,
}

/// The numbers of one release in one environment, over the last 90 days.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Side {
    /// Release name.
    pub release: String,
    /// Distinct users (the team's own left out).
    pub users: i64,
    /// Sessions.
    pub sessions: i64,
    /// Sessions with one interaction at most.
    pub bounced: i64,
    /// Sessions with an error.
    pub error_sessions: i64,
    /// Sessions with a completed task.
    pub task_sessions: i64,
    /// Completed tasks.
    pub tasks: i64,
    /// Mean length of the sessions that are not bounces, seconds.
    pub avg_secs: f64,
    /// LCP, INP and CLS at p75.
    pub vitals: Vec<VitalP75>,
    /// Features with a success event.
    pub features: Vec<FeatureUse>,
    /// Error groups first seen in this release.
    pub new_errors: Vec<NewError>,
    /// Server routes (#594): requests, 5xx and durations per route.
    #[serde(default)]
    pub routes: Vec<crate::routes::RouteStat>,
    /// Runtime health (#610): none when the release sent no runtime stats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<crate::runtime::RuntimeStat>,
    /// Outgoing dependencies (#608): calls, failures and durations per
    /// external host; empty when the release sent no client stats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<crate::dependencies::DependencyStat>,
    /// The features its code change touched (#576, D-095), in map order:
    /// judged and listed first. Empty when not known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched: Vec<String>,
}

fn share(n: i64, of: i64) -> f64 {
    n as f64 / of.max(1) as f64
}

impl Side {
    /// Share of sessions with an error.
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        share(self.error_sessions, self.sessions)
    }
    /// Share of sessions that bounced.
    #[must_use]
    pub fn bounce_rate(&self) -> f64 {
        share(self.bounced, self.sessions)
    }
    /// Share of sessions with a completed task.
    #[must_use]
    pub fn task_rate(&self) -> f64 {
        share(self.task_sessions, self.sessions)
    }
    /// Completed tasks per session.
    #[must_use]
    pub fn tasks_per_session(&self) -> f64 {
        share(self.tasks, self.sessions)
    }
    /// Enough sessions to test a share.
    fn enough(&self) -> bool {
        self.sessions >= MIN_SESSIONS
    }
    fn vital(&self, name: &str) -> Option<&VitalP75> {
        self.vitals.iter().find(|v| v.name == name)
    }
    fn feature(&self, key: &str) -> Option<&FeatureUse> {
        self.features.iter().find(|f| f.feature == key)
    }

    /// A goal KPI's value on this release, when the release gives one,
    /// and whether it rests on enough measurements.
    fn kpi(&self, kpi: &str) -> Option<(f64, bool)> {
        let vital = |n: &str| self.vital(n).map(|v| (v.p75, v.samples >= MIN_SESSIONS));
        match kpi {
            "error_rate" => Some((self.error_rate(), self.enough())),
            "bounce_rate" => Some((self.bounce_rate(), self.enough())),
            "avg_secs" => Some((self.avg_secs, self.enough())),
            "tasks_per_session" => Some((self.tasks_per_session(), self.enough())),
            "lcp_p75" => vital("LCP"),
            "inp_p75" => vital("INP"),
            "cls_p75" => vital("CLS"),
            k => {
                let f = self.feature(k.strip_prefix("success:")?)?;
                Some((share(f.succeeded, f.sessions), f.sessions >= MIN_SESSIONS))
            }
        }
    }
}

/// How one number moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Worse than the release before.
    Worse,
    /// Better.
    Better,
    /// No change that counts.
    Same,
    /// Too few measurements on one side to say.
    TooFew,
    /// Shown, not judged.
    Info,
}

/// One number compared.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// `kpi`, `errors`, `vital`, `feature`, `guardrail`, `goal`, `route` or
    /// `runtime`.
    pub kind: &'static str,
    /// The KPI key, vital or feature.
    pub key: String,
    /// As shown.
    pub label: String,
    /// On the release before.
    pub previous: Option<f64>,
    /// On this release.
    pub current: f64,
    /// How it moved.
    pub outcome: Outcome,
    /// Whether a `worse` outcome makes the release regressed.
    pub judged: bool,
    /// In plain words.
    pub note: String,
    /// A slower route's slowest request in this release (#607, D-089),
    /// with its session when known: the structured part of its reason, for
    /// a page to open the replay at that time. Never changes the outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exemplar: Option<crate::routes::Exemplar>,
}

/// A release compared with the one before it.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Release name.
    pub release: String,
    /// Environment.
    pub environment: String,
    /// The release before it in the environment (by first event), if any.
    pub previous_release: Option<String>,
    /// The verdict; none until the release has enough traffic or is a day
    /// old.
    pub verdict: Option<Verdict>,
    /// Why, in plain words, the worst first.
    pub reasons: Vec<String>,
    /// Hours since its first event.
    pub age_hours: i64,
    /// Its numbers.
    pub current: Side,
    /// The numbers of the release before.
    pub previous: Option<Side>,
    /// Every number compared.
    pub checks: Vec<Check>,
    /// Runtime health shown next to the release before (#610): RSS and its
    /// growth, event-loop delay p99, restarts per day. Absent when the
    /// release sent no runtime stats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<crate::runtime::RuntimeShown>,
    /// Outgoing dependencies that fail more or got slower than on the
    /// release before (#608, D-090). Shown, never judged: they are not
    /// checks and never change the verdict. Absent when none did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<crate::dependencies::DependencyChange>,
    /// The release's notes from the git host (#574, D-093): its merged
    /// pull requests grouped, or why they are not there. Shown, never
    /// judged. Filled by `releases.report`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<crate::release_git::ReleaseNotes>,
}

/// The board's *release regression* rule (#227), shared by the verdict:
/// the newer release has at least 16 sessions, and its share of sessions
/// with an error is more than 1.5 times the older one's and 3 points
/// above it.
#[must_use]
pub fn errors_regressed(new_sessions: i64, new_bad: i64, old_sessions: i64, old_bad: i64) -> bool {
    let (rn, ro) = (share(new_bad, new_sessions), share(old_bad, old_sessions));
    new_sessions >= 2 * MIN_GROUP && rn > ro * 1.5 && rn - ro > 0.03
}

fn pct(r: f64) -> String {
    format!("{:.1}%", r * 100.0)
}

/// A share compared with a two-proportion test: worse when it moved the
/// wrong way by at least `min` and significantly.
fn share_check(
    key: &str,
    label: &str,
    (c, cn): (f64, i64),
    (p, pn): (f64, i64),
    up: bool,
    min: f64,
) -> Check {
    let outcome = match stats::two_proportions(c, cn, p, pn) {
        None => Outcome::TooFew,
        Some(pv) if pv < ALPHA && (c - p).abs() >= min => {
            if (c > p) == up {
                Outcome::Better
            } else {
                Outcome::Worse
            }
        }
        Some(_) => Outcome::Same,
    };
    Check {
        exemplar: None,
        kind: "kpi",
        key: key.to_owned(),
        label: label.to_owned(),
        previous: Some(p),
        current: c,
        outcome,
        judged: true,
        note: format!("{label}: {} → {}", pct(p), pct(c)),
    }
}

fn rating_rank(name: &str, v: f64) -> u8 {
    match vitals::rating(name, v) {
        "good" => 0,
        "needs-improvement" => 1,
        _ => 2,
    }
}

/// Compare `current` with `previous` (the release before it in the same
/// environment), `age_hours` after the release's first event. `goals`
/// apply in production only (the caller passes none elsewhere).
#[must_use]
pub fn compare(
    environment: &str,
    current: &Side,
    previous: Option<&Side>,
    age_hours: i64,
    goals: &[Goal],
) -> Report {
    let c = current;
    let mut checks = Vec::new();
    if let Some(p) = previous {
        let worse = errors_regressed(c.sessions, c.error_sessions, p.sessions, p.error_sessions);
        let better = stats::two_proportions(c.error_rate(), c.sessions, p.error_rate(), p.sessions)
            .is_some_and(|pv| pv < ALPHA && c.error_rate() < p.error_rate());
        checks.push(Check {
            exemplar: None,
            kind: "kpi",
            key: "error_rate".into(),
            label: "Sessions with errors".into(),
            previous: Some(p.error_rate()),
            current: c.error_rate(),
            outcome: if worse {
                Outcome::Worse
            } else if better {
                Outcome::Better
            } else if !c.enough() || !p.enough() {
                Outcome::TooFew
            } else {
                Outcome::Same
            },
            judged: true,
            note: format!(
                "Sessions with errors: {} → {}",
                pct(p.error_rate()),
                pct(c.error_rate())
            ),
        });
        checks.push(share_check(
            "bounce_rate",
            "Bounce rate",
            (c.bounce_rate(), c.sessions),
            (p.bounce_rate(), p.sessions),
            false,
            MIN_MOVE,
        ));
        checks.push(share_check(
            "task_rate",
            "Sessions with a completed task",
            (c.task_rate(), c.sessions),
            (p.task_rate(), p.sessions),
            true,
            MIN_MOVE,
        ));
        let per_user = |s: &Side| share(s.sessions, s.users);
        checks.push(Check {
            exemplar: None,
            kind: "kpi",
            key: "sessions_per_user".into(),
            label: "Sessions per user".into(),
            previous: Some(per_user(p)),
            current: per_user(c),
            outcome: Outcome::Info,
            judged: false,
            note: format!(
                "{} sessions by {} users (before: {} by {})",
                c.sessions, c.users, p.sessions, p.users
            ),
        });
        for name in VITALS {
            let (Some(cv), Some(pv)) = (c.vital(name), p.vital(name)) else {
                continue;
            };
            let enough = cv.samples >= MIN_SESSIONS && pv.samples >= MIN_SESSIONS;
            let (rc, rp) = (rating_rank(name, cv.p75), rating_rank(name, pv.p75));
            checks.push(Check {
                exemplar: None,
                kind: "vital",
                key: name.to_owned(),
                label: format!("{name} p75"),
                previous: Some(pv.p75),
                current: cv.p75,
                outcome: if !enough {
                    Outcome::TooFew
                } else if rc > rp {
                    Outcome::Worse
                } else if rc < rp {
                    Outcome::Better
                } else {
                    Outcome::Same
                },
                judged: true,
                note: format!(
                    "{name} p75: {} → {} ({})",
                    vitals::show(name, pv.p75),
                    vitals::show(name, cv.p75),
                    vitals::rating(name, cv.p75)
                ),
            });
        }
        // The features the release touched lead the comparison (#576).
        let (touched, rest) = feature_checks(c, p);
        checks.extend(rest);
        let mut first = touched;
        first.append(&mut checks);
        checks = first;
        for g in goals {
            goal_checks(g, c, p, &mut checks);
        }
        checks.extend(crate::routes::checks(&c.routes, &p.routes));
        if let (Some(cr), Some(pr)) = (&c.runtime, &p.runtime) {
            checks.push(crate::runtime::check(cr, pr));
        }
    }
    let new_errors: Vec<&NewError> = c
        .new_errors
        .iter()
        .filter(|e| e.users >= NEW_ERROR_USERS)
        .collect();
    for e in &new_errors {
        checks.push(Check {
            exemplar: None,
            kind: "errors",
            key: e.fingerprint.clone(),
            label: "New error".into(),
            previous: None,
            current: e.users as f64,
            outcome: Outcome::Worse,
            judged: true,
            note: format!(
                "New {}error: {} ({} users, {} times)",
                if e.server { "server " } else { "" },
                e.sample,
                e.users,
                e.count
            ),
        });
    }
    let (verdict, reasons) = decide(c, previous, age_hours, &checks);
    Report {
        release: c.release.clone(),
        environment: environment.to_owned(),
        previous_release: previous.map(|p| p.release.clone()),
        verdict,
        reasons,
        age_hours,
        current: c.clone(),
        previous: previous.cloned(),
        checks,
        runtime: c
            .runtime
            .as_ref()
            .map(|cr| crate::runtime::shown(cr, previous.and_then(|p| p.runtime.as_ref()))),
        dependencies: previous
            .map(|p| crate::dependencies::shown(&c.dependencies, &p.dependencies))
            .unwrap_or_default(),
        notes: None,
    }
}

/// The features compared on two releases: the success rate of each one
/// with a success event used on both, as `(touched, the rest)`. The
/// features the release touched (#576) are in their order, and one that
/// cannot be compared says so, shown and not judged. The thresholds are
/// the same for every feature.
fn feature_checks(c: &Side, p: &Side) -> (Vec<Check>, Vec<Check>) {
    let one = |f: &FeatureUse, touched: bool| {
        let pf = p.feature(&f.feature)?;
        let (rc, rp) = (
            share(f.succeeded, f.sessions),
            share(pf.succeeded, pf.sessions),
        );
        let label = if touched {
            format!("Success rate of {} (changed in this release)", f.feature)
        } else {
            format!("Success rate of {}", f.feature)
        };
        let mut ch = share_check(
            &f.feature,
            &label,
            (rc, f.sessions),
            (rp, pf.sessions),
            true,
            MIN_FEATURE_DROP,
        );
        ch.kind = "feature";
        Some(ch)
    };
    let mut out = Vec::new();
    for key in &c.touched {
        match c.feature(key).and_then(|f| one(f, true)) {
            Some(ch) => out.push(ch),
            None => out.push(Check {
                exemplar: None,
                kind: "feature",
                key: key.clone(),
                label: format!("{key} (changed in this release)"),
                previous: None,
                current: 0.0,
                outcome: Outcome::Info,
                judged: false,
                note: format!(
                    "{key} was changed in this release but cannot be compared: it has no success \
                     event, or no sessions on {} and {}.",
                    p.release, c.release
                ),
            }),
        }
    }
    let rest = c
        .features
        .iter()
        .filter(|f| !c.touched.contains(&f.feature))
        .filter_map(|f| one(f, false))
        .collect();
    (out, rest)
}

/// A goal's guardrails (judged) and its KPI (shown) on two releases.
fn goal_checks(g: &Goal, c: &Side, p: &Side, checks: &mut Vec<Check>) {
    let guard = |gr: &Guardrail, checks: &mut Vec<Check>| {
        let (Some((cv, ce)), Some((pv, pe))) = (c.kpi(&gr.kpi), p.kpi(&gr.kpi)) else {
            return;
        };
        let (label, _, _) = kpi_info(&gr.kpi);
        let outcome = if !ce || !pe {
            Outcome::TooFew
        } else if gr.breached(cv) && !gr.breached(pv) {
            Outcome::Worse
        } else if !gr.breached(cv) && gr.breached(pv) {
            Outcome::Better
        } else {
            Outcome::Same
        };
        checks.push(Check {
            exemplar: None,
            kind: "guardrail",
            key: gr.kpi.clone(),
            label: label.clone(),
            previous: Some(pv),
            current: cv,
            outcome,
            judged: true,
            note: format!(
                "Guardrail {label}: {pv:.3} → {cv:.3} (limit {:.3})",
                gr.limit
            ),
        });
    };
    for gr in &g.guardrails {
        guard(gr, checks);
    }
    let (Some((cv, ce)), Some((pv, pe))) = (c.kpi(&g.kpi), p.kpi(&g.kpi)) else {
        return;
    };
    let (label, _, _) = kpi_info(&g.kpi);
    let gap = |v: f64| (g.target - v).abs();
    checks.push(Check {
        exemplar: None,
        kind: "goal",
        key: g.kpi.clone(),
        label: label.clone(),
        previous: Some(pv),
        current: cv,
        outcome: if !ce || !pe {
            Outcome::TooFew
        } else if gap(cv) < gap(pv) {
            Outcome::Better
        } else if gap(cv) > gap(pv) {
            Outcome::Worse
        } else {
            Outcome::Same
        },
        judged: false,
        note: format!("Goal {label}: {pv:.3} → {cv:.3} (target {:.3})", g.target),
    });
}

/// The verdict and its reasons from the checks.
fn decide(
    c: &Side,
    previous: Option<&Side>,
    age_hours: i64,
    checks: &[Check],
) -> (Option<Verdict>, Vec<String>) {
    let sampled = c.sessions >= MIN_SESSIONS && c.users >= MIN_USERS;
    if !sampled && age_hours < DUE_HOURS {
        return (
            None,
            vec![format!(
                "Waiting for {MIN_SESSIONS} sessions and {MIN_USERS} users, or {DUE_HOURS} h: \
                 {} sessions, {} users, {age_hours} h so far.",
                c.sessions, c.users
            )],
        );
    }
    let worse: Vec<String> = checks
        .iter()
        .filter(|x| x.judged && x.outcome == Outcome::Worse)
        .map(|x| x.note.clone())
        .collect();
    if !worse.is_empty() {
        return (Some(Verdict::Regressed), worse);
    }
    let Some(p) = previous else {
        return (
            Some(Verdict::Inconclusive),
            vec!["No earlier release in this environment to compare with.".into()],
        );
    };
    if !sampled || !p.enough() {
        return (
            Some(Verdict::Inconclusive),
            vec![format!(
                "Too few users to judge: {} sessions by {} users on {}, {} sessions on {} \
                 ({MIN_SESSIONS} sessions and {MIN_USERS} users needed).",
                c.sessions, c.users, c.release, p.sessions, p.release
            )],
        );
    }
    let mut reasons = vec![format!(
        "Nothing got worse than {}: errors, bounce, tasks, Web Vitals, features and guardrails.",
        p.release
    )];
    reasons.extend(
        checks
            .iter()
            .filter(|x| x.judged && x.outcome == Outcome::Better)
            .map(|x| format!("Better: {}", x.note)),
    );
    (Some(Verdict::Healthy), reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(release: &str, users: i64, sessions: i64, bad: i64) -> Side {
        Side {
            release: release.into(),
            users,
            sessions,
            bounced: sessions / 4,
            error_sessions: bad,
            task_sessions: sessions / 2,
            tasks: sessions,
            avg_secs: 60.0,
            ..Side::default()
        }
    }

    #[test]
    fn a_release_as_good_as_the_one_before_is_healthy() {
        let r = compare(
            "production",
            &side("v2", 120, 400, 8),
            Some(&side("v1", 150, 500, 10)),
            30,
            &[],
        );
        assert_eq!(r.verdict, Some(Verdict::Healthy), "{:?}", r.reasons);
        assert!(r.checks.iter().all(|c| c.outcome != Outcome::Worse));
    }

    #[test]
    fn an_error_spike_or_a_new_error_makes_it_regressed() {
        let prev = side("v1", 150, 500, 10);
        let spike = compare(
            "production",
            &side("v2", 120, 400, 80),
            Some(&prev),
            30,
            &[],
        );
        assert_eq!(spike.verdict, Some(Verdict::Regressed));
        assert!(spike.reasons.iter().any(|r| r.contains("errors")));
        // The board's card says the same.
        assert!(errors_regressed(400, 80, 500, 10));
        // A new error group hitting two users, the rates unchanged.
        let mut fresh = side("v2", 120, 400, 8);
        fresh.new_errors.push(NewError {
            fingerprint: "TypeError: x @ app.js".into(),
            sample: "TypeError: x".into(),
            users: 3,
            count: 5,
            server: false,
        });
        let r = compare("production", &fresh, Some(&prev), 30, &[]);
        assert_eq!(r.verdict, Some(Verdict::Regressed));
    }

    #[test]
    fn a_release_that_breaks_a_key_server_route_is_regressed_and_one_without_stats_is_not_judged_on_them()
     {
        let route = |errors: i64| crate::routes::RouteStat {
            method: "POST".into(),
            route: "/api/auth/sign-in".into(),
            requests: 300,
            errors,
            buckets: {
                let mut b = vec![0; crate::routes::BUCKETS];
                b[3] = 300;
                b
            },
            max_ms: 40,
            key_of: Some("login".into()),
            ..crate::routes::RouteStat::default()
        };
        let mut prev = side("v1", 120, 400, 8);
        prev.routes = vec![route(0)];
        let mut broken = side("v2", 120, 400, 8);
        broken.routes = vec![route(300)];
        let r = compare("production", &broken, Some(&prev), 30, &[]);
        assert_eq!(r.verdict, Some(Verdict::Regressed));
        assert!(r.reasons.iter().any(|x| x.contains("key action of login")));
        // The same release with no server stats: the backend says nothing.
        let quiet = side("v2", 120, 400, 8);
        let r = compare("production", &quiet, Some(&prev), 30, &[]);
        assert!(r.checks.iter().all(|c| c.kind != "route"));
    }

    #[test]
    fn a_crash_looping_release_is_regressed_and_memory_growth_is_only_shown() {
        let runtime = |restarts: i64, rss: i64| crate::runtime::RuntimeStat {
            starts: restarts + 2,
            restarts,
            hours: 48.0,
            rss_max_bytes: Some(rss),
            ..crate::runtime::RuntimeStat::default()
        };
        let mut prev = side("v1", 120, 400, 8);
        prev.runtime = Some(runtime(1, 200 << 20));
        let mut looping = side("v2", 120, 400, 8);
        looping.runtime = Some(runtime(150, 200 << 20));
        let r = compare("production", &looping, Some(&prev), 30, &[]);
        assert_eq!(r.verdict, Some(Verdict::Regressed), "{:?}", r.reasons);
        assert!(r.reasons.iter().any(|x| x.contains("restarts")));
        // Three times the memory, restarts as before: still healthy, the
        // growth shown.
        let mut leaking = side("v2", 120, 400, 8);
        leaking.runtime = Some(runtime(1, 600 << 20));
        let r = compare("production", &leaking, Some(&prev), 30, &[]);
        assert_eq!(r.verdict, Some(Verdict::Healthy), "{:?}", r.reasons);
        assert!(
            r.runtime
                .as_ref()
                .and_then(|x| x.rss_growth)
                .is_some_and(|g| g > 0.0)
        );
        // No runtime stats: no runtime check and no runtime fields.
        let quiet = side("v2", 120, 400, 8);
        let r = compare("production", &quiet, Some(&prev), 30, &[]);
        assert!(r.runtime.is_none() && r.checks.iter().all(|c| c.kind != "runtime"));
        let json = serde_json::to_value(&r).unwrap_or_default();
        assert!(json.get("runtime").is_none() && json["current"].get("runtime").is_none());
    }

    #[test]
    fn a_dependency_that_fails_or_slows_down_is_shown_and_never_changes_the_outcome() {
        let host = |errors: i64, bucket: usize| crate::dependencies::DependencyStat {
            host: "api.stripe.com".into(),
            calls: 500,
            errors,
            buckets: {
                let mut b = vec![0; crate::routes::BUCKETS];
                b[bucket] = 500;
                b
            },
            max_ms: 20_000,
            ..crate::dependencies::DependencyStat::default()
        };
        let mut prev = side("v1", 150, 500, 10);
        prev.dependencies = vec![host(0, 5)];
        let quiet = side("v2", 120, 400, 8);
        let baseline = compare("production", &quiet, Some(&prev), 30, &[]);
        let mut broken = quiet.clone();
        broken.dependencies = vec![host(400, 12)];
        let r = compare("production", &broken, Some(&prev), 30, &[]);
        assert_eq!(r.verdict, baseline.verdict, "{:?}", r.reasons);
        assert_eq!(r.checks.len(), baseline.checks.len(), "never a check");
        let shown = r.dependencies.first().expect("the regression is shown");
        assert!(shown.fails_more && shown.slower);
        // A regressed release stays regressed for its own reasons, and a
        // dependency does not add one.
        let mut bad = side("v2", 120, 400, 80);
        let regressed = compare("production", &bad, Some(&prev), 30, &[]);
        bad.dependencies = vec![host(400, 12)];
        let r = compare("production", &bad, Some(&prev), 30, &[]);
        assert_eq!(r.verdict, regressed.verdict);
        assert_eq!(r.reasons, regressed.reasons);
        assert!(baseline.dependencies.is_empty());
    }

    #[test]
    fn too_few_users_after_a_day_is_inconclusive_and_before_a_day_waits() {
        let prev = side("v1", 150, 500, 10);
        let few = side("v2", 3, 6, 0);
        let r = compare("production", &few, Some(&prev), 30, &[]);
        assert_eq!(r.verdict, Some(Verdict::Inconclusive));
        let r = compare("production", &few, Some(&prev), 2, &[]);
        assert_eq!(r.verdict, None, "not due yet");
        // No earlier release: nothing to compare with.
        let r = compare("production", &side("v1", 150, 500, 10), None, 30, &[]);
        assert_eq!(r.verdict, Some(Verdict::Inconclusive));
    }

    #[test]
    fn a_guardrail_the_release_breaches_is_a_regression() {
        let goal = Goal {
            id: "g".into(),
            kpi: "avg_secs".into(),
            target: 120.0,
            up: true,
            deadline: "2027-01-01".into(),
            owner: String::new(),
            baseline: 60.0,
            created: "2026-11-01".into(),
            guardrails: vec![Guardrail {
                kpi: "bounce_rate".into(),
                limit: 0.30,
                up: false,
            }],
            releases: None,
        };
        let prev = side("v1", 150, 500, 10);
        let mut cur = side("v2", 120, 400, 8);
        cur.bounced = 130;
        let r = compare("production", &cur, Some(&prev), 30, &[goal]);
        assert_eq!(r.verdict, Some(Verdict::Regressed));
        assert!(r.checks.iter().any(|c| c.kind == "goal" && !c.judged));
    }
}

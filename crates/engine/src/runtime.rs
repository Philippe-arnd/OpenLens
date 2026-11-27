//! Runtime health per release (#610, D-087): the `$runtime_stats` the
//! Node SDK sends, one per process start (`process.start`) and one per
//! stats window per process (event-loop delay p50 and p99, the largest
//! RSS and heap used), read per release. A window is 1, 5 or 10 minutes
//! (#628, D-091): each point says its own in `$window_s`.
//!
//! - **Restarts per day:** the process starts of a release after its
//!   rollout ([`ROLLOUT_MINUTES`] after its first runtime event: the
//!   deploy's own starts are not restarts), over the hours it ran.
//! - **Shown, not judged:** the largest RSS (and its growth against the
//!   release before) and the typical event-loop delay p99 (the median of
//!   the windows' p99). Memory follows uptime and load, so a single
//!   threshold would be noise.
//! - **In the verdict** ([`check`]): a release is worse only when its
//!   restarts per day go up significantly (a crash loop): at least
//!   [`MIN_HOURS`] on both releases, [`MIN_RESTARTS`] restarts on one of
//!   them, [`RESTARTS_RATIO`] times the rate before **and**
//!   [`RESTARTS_MIN_MOVE`] more per day, at [`stats::ALPHA`]. A release with
//!   no runtime stats keeps the verdict it had, and has no runtime fields.
//!
//! The stats are read from `raw_events`: the `events` view leaves them
//! out, so no user KPI ever counts them.

use openlens_query::QueryEngine;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::verdict::{Check, Outcome};
use crate::{EngineError, Result, float, int, stats, text};

/// Starts in the first minutes of a release are its deploy (one per
/// instance), not restarts.
pub const ROLLOUT_MINUTES: i64 = 30;
/// Hours a release must have run, after its rollout, on both sides to be
/// judged on restarts.
pub const MIN_HOURS: f64 = 6.0;
/// Restarts one of the two releases needs before the rates are compared.
pub const MIN_RESTARTS: i64 = 10;
/// Ratio of restarts per day from which a release counts as worse…
pub const RESTARTS_RATIO: f64 = 2.0;
/// …when it also restarts this many more times per day.
pub const RESTARTS_MIN_MOVE: f64 = 5.0;
/// Releases read at most.
pub const MAX_RELEASES: usize = 500;

/// The runtime numbers of one release.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RuntimeStat {
    /// Process starts, the deploy's included.
    pub starts: i64,
    /// Process starts after the rollout: the restarts.
    pub restarts: i64,
    /// Hours from the release's first runtime event to the end of its
    /// last window.
    pub hours: f64,
    /// Largest RSS seen (`process.memory.usage`), bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_max_bytes: Option<i64>,
    /// Largest heap used seen (`v8js.memory.heap.used`), bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heap_used_max_bytes: Option<i64>,
    /// Median of the windows' event-loop delay p50, ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_delay_p50_ms: Option<f64>,
    /// Median of the windows' event-loop delay p99, ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_delay_p99_ms: Option<f64>,
}

impl RuntimeStat {
    /// Hours the release ran after its rollout.
    #[must_use]
    pub fn settled_hours(&self) -> f64 {
        (self.hours - ROLLOUT_MINUTES as f64 / 60.0).max(0.0)
    }
    /// Restarts per day after the rollout; none before the release has
    /// run an hour past it.
    #[must_use]
    pub fn restarts_per_day(&self) -> Option<f64> {
        let h = self.settled_hours();
        (h >= 1.0).then(|| self.restarts as f64 * 24.0 / h)
    }
}

/// What is shown of a release's runtime next to the one before it.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RuntimeShown {
    /// Restarts per day after the rollout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restarts_per_day: Option<f64>,
    /// The same, on the release before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_restarts_per_day: Option<f64>,
    /// Largest RSS, bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_max_bytes: Option<i64>,
    /// Largest RSS on the release before, bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_rss_max_bytes: Option<i64>,
    /// Growth of the largest RSS against the release before (0.25 = 25 %
    /// more). Shown, never judged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_growth: Option<f64>,
    /// Typical event-loop delay p99, ms. Shown, never judged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_delay_p99_ms: Option<f64>,
    /// The same, on the release before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_loop_delay_p99_ms: Option<f64>,
}

/// `current`'s runtime as shown, next to `previous`'s when there is one.
#[must_use]
pub fn shown(current: &RuntimeStat, previous: Option<&RuntimeStat>) -> RuntimeShown {
    let prss = previous.and_then(|p| p.rss_max_bytes);
    let rss_growth = match (current.rss_max_bytes, prss) {
        (Some(c), Some(p)) if p > 0 => Some(c as f64 / p as f64 - 1.0),
        _ => None,
    };
    RuntimeShown {
        restarts_per_day: current.restarts_per_day(),
        previous_restarts_per_day: previous.and_then(RuntimeStat::restarts_per_day),
        rss_max_bytes: current.rss_max_bytes,
        previous_rss_max_bytes: prss,
        rss_growth,
        loop_delay_p99_ms: current.loop_delay_p99_ms,
        previous_loop_delay_p99_ms: previous.and_then(|p| p.loop_delay_p99_ms),
    }
}

/// Two event counts over two exposures (`n1` in `t1`, `n2` in `t2`):
/// two-sided p-value that their rates are equal. Given the total, `n1` is
/// binomial with the share `t1` has of the time under the null; normal
/// approximation, through [`stats::p_from_z`].
fn two_rates(n1: i64, t1: f64, n2: i64, t2: f64) -> Option<f64> {
    let n = (n1 + n2) as f64;
    if n <= 0.0 || t1 <= 0.0 || t2 <= 0.0 {
        return None;
    }
    let share = t1 / (t1 + t2);
    let se = (n * share * (1.0 - share)).sqrt();
    if se <= 0.0 || !se.is_finite() {
        return None;
    }
    Some(stats::p_from_z((n1 as f64 - n * share) / se))
}

/// The verdict's runtime check: restarts per day against the release
/// before. The only runtime number judged.
#[must_use]
pub fn check(current: &RuntimeStat, previous: &RuntimeStat) -> Check {
    let (hc, hp) = (current.settled_hours(), previous.settled_hours());
    let (rc, rp) = (
        current.restarts_per_day().unwrap_or(0.0),
        previous.restarts_per_day().unwrap_or(0.0),
    );
    let outcome = if hc < MIN_HOURS || hp < MIN_HOURS {
        Outcome::TooFew
    } else if current.restarts < MIN_RESTARTS && previous.restarts < MIN_RESTARTS {
        Outcome::Same
    } else {
        match two_rates(current.restarts, hc, previous.restarts, hp) {
            Some(pv) if pv < stats::ALPHA => {
                if rc >= rp * RESTARTS_RATIO && rc - rp >= RESTARTS_MIN_MOVE {
                    Outcome::Worse
                } else if rp >= rc * RESTARTS_RATIO && rp - rc >= RESTARTS_MIN_MOVE {
                    Outcome::Better
                } else {
                    Outcome::Same
                }
            }
            Some(_) => Outcome::Same,
            None => Outcome::TooFew,
        }
    };
    Check {
        exemplar: None,
        kind: "runtime",
        key: "restarts_per_day".into(),
        label: "Process restarts per day".into(),
        previous: Some(rp),
        current: rc,
        outcome,
        judged: true,
        note: format!(
            "Process restarts per day: {rp:.1} → {rc:.1} ({} restarts in {hc:.0} h)",
            current.restarts
        ),
    }
}

/// One release's runtime, with when it first reported.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseRuntime {
    /// Release name.
    pub release: String,
    /// First runtime event, UTC.
    pub first_seen: String,
    /// Its numbers.
    pub stat: RuntimeStat,
}

/// The SQL reading the runtime of each release where `w` holds.
fn runtime_sql(w: &str) -> String {
    let attr = |k: &str, pat: &str| format!("regexp_match(r.properties, '\"{k}\":{pat}')[1]");
    let bytes = |k: &str| format!("CAST({} AS BIGINT)", attr(k, "([0-9]{1,18})"));
    let secs = |k: &str| {
        format!(
            "CAST({} AS DOUBLE)",
            attr(k, "([0-9]{1,12}(?:\\.[0-9]{1,9})?(?:[eE][-+]?[0-9]{1,3})?)")
        )
    };
    format!(
        "WITH r AS (SELECT release, timestamp, properties FROM raw_events \
           WHERE event = '{event}' AND {w}), \
         f AS (SELECT release, min(timestamp) AS t0 FROM r GROUP BY release) \
         SELECT r.release AS release, CAST(min(f.t0) AS VARCHAR) AS first_seen, \
         max(date_part('epoch', r.timestamp) + {win}) - date_part('epoch', min(f.t0)) AS span_s, \
         sum(CASE WHEN strpos(r.properties, '\"process.start\":true') > 0 THEN 1 ELSE 0 END) AS starts, \
         sum(CASE WHEN strpos(r.properties, '\"process.start\":true') > 0 \
             AND r.timestamp >= f.t0 + INTERVAL '{ROLLOUT_MINUTES} minutes' THEN 1 ELSE 0 END) AS restarts, \
         max({rss}) AS rss, max({heap}) AS heap, \
         median({p50}) AS p50_s, median({p99}) AS p99_s \
         FROM r JOIN f ON r.release = f.release \
         GROUP BY r.release ORDER BY min(f.t0) LIMIT {MAX_RELEASES}",
        event = openlens_event::names::RUNTIME_STATS,
        rss = bytes("process\\.memory\\.usage"),
        heap = bytes("v8js\\.memory\\.heap\\.used"),
        p50 = secs("nodejs\\.eventloop\\.delay\\.p50"),
        p99 = secs("nodejs\\.eventloop\\.delay\\.p99"),
        // A window is stamped with its start: it ends `$window_s` later
        // (0 for a process start), whatever the window's length (#628).
        win = crate::capacity::window::window_s_sql("r.properties"),
    )
}

fn opt_int(r: &Map<String, Value>, k: &str) -> Option<i64> {
    r.get(k).and_then(Value::as_i64).filter(|v| *v > 0)
}

fn opt_ms(r: &Map<String, Value>, k: &str) -> Option<f64> {
    r.get(k)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|s| (s * 1000.0 * 1000.0).round() / 1000.0)
}

fn row_runtime(r: &Map<String, Value>) -> Option<ReleaseRuntime> {
    Some(ReleaseRuntime {
        release: text(r, "release")?,
        first_seen: text(r, "first_seen").unwrap_or_default(),
        stat: RuntimeStat {
            starts: int(r, "starts"),
            restarts: int(r, "restarts"),
            hours: float(r, "span_s").max(0.0) / 3600.0,
            rss_max_bytes: opt_int(r, "rss"),
            heap_used_max_bytes: opt_int(r, "heap"),
            loop_delay_p50_ms: opt_ms(r, "p50_s"),
            loop_delay_p99_ms: opt_ms(r, "p99_s"),
        },
    })
}

/// The runtime of each release of `environment` since `since` (a
/// `YYYY-MM-DD` day), oldest first.
///
/// # Errors
/// A bad environment or a failed query.
pub async fn by_release(
    q: &QueryEngine,
    project: &str,
    environment: &str,
    since: &str,
) -> Result<Vec<ReleaseRuntime>> {
    let mut params = openlens_sqlgen::Params::new();
    let env = params
        .set("env", environment)
        .map_err(|_| EngineError::InvalidFilter("environment"))?;
    let w = format!(
        "date >= '{since}' AND environment = {env} AND release IS NOT NULL AND release <> ''"
    );
    let r = q
        .with_params(&params)
        .sql(project, &runtime_sql(&w))
        .await?;
    Ok(r.rows.iter().filter_map(row_runtime).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(restarts: i64, hours: f64, rss: i64) -> RuntimeStat {
        RuntimeStat {
            starts: restarts + 2,
            restarts,
            hours,
            rss_max_bytes: Some(rss),
            heap_used_max_bytes: Some(rss / 2),
            loop_delay_p50_ms: Some(1.0),
            loop_delay_p99_ms: Some(20.0),
        }
    }

    #[test]
    fn a_crash_loop_is_worse_and_memory_growth_alone_never_is() {
        let before = stat(2, 72.0, 200 << 20);
        let looping = stat(120, 48.0, 200 << 20);
        let c = check(&looping, &before);
        assert_eq!(c.outcome, Outcome::Worse, "{}", c.note);
        assert!(c.judged && c.current > c.previous.unwrap_or(f64::MAX));
        // Four times the memory, the same restarts: shown, not worse.
        let leaking = stat(2, 72.0, 800 << 20);
        assert_ne!(check(&leaking, &before).outcome, Outcome::Worse);
        let s = shown(&leaking, Some(&before));
        assert!(s.rss_growth.is_some_and(|g| g > 1.0));
        // The previous release looping and this one not: better.
        assert_eq!(check(&before, &looping).outcome, Outcome::Better);
    }

    #[test]
    fn a_young_release_or_a_few_restarts_are_not_judged_worse() {
        let before = stat(1, 72.0, 1);
        // Two hours old: too early, however many restarts.
        assert_eq!(check(&stat(40, 2.0, 1), &before).outcome, Outcome::TooFew);
        // A handful of restarts on both: nothing to see.
        assert_eq!(check(&stat(6, 24.0, 1), &before).outcome, Outcome::Same);
        // More restarts but not significantly so many more per day.
        let busy = stat(30, 72.0, 1);
        assert_ne!(
            check(&stat(36, 72.0, 1), &busy).outcome,
            Outcome::Worse,
            "a small rise is noise"
        );
    }

    #[test]
    fn a_release_without_a_previous_runtime_shows_no_growth() {
        let s = shown(&stat(0, 10.0, 1 << 20), None);
        assert!(s.rss_growth.is_none() && s.previous_rss_max_bytes.is_none());
        assert!(s.restarts_per_day.is_some());
    }
}

//! Job check-ins as commands (#609, D-086): a cron or background job tells
//! OpenLens it ran (`jobs.check_in`, from `POST /check-in/…` or a server
//! SDK's `checkIn`), and OpenLens learns how often it runs and how long it
//! takes. The minute pass ([`alert_pass`]) opens an incident, through the
//! alert channels, when a job did not run (no check-in for 1.5 times its
//! usual gap), failed (its last check-in said `error`) or got slower (its
//! last run took twice its usual time or more), and closes it with the
//! next good check-in. A sibling of the uptime probe: the probe asks the
//! app, a job reports itself.
//!
//! - **Schedule:** the median gap between the kept check-ins, once there
//!   are [`MIN_GAPS`] gaps; or the gap the job declared (`expectEvery`).
//!   No cron expression to type.
//! - **Incidents:** `job:<slug>:missed|failed|slower`, announced once (no
//!   hourly reminder), closed by the next check-in that clears them.
//! - **Availability:** off by default; with the project's one switch
//!   ([`SetAvailability`]), the Uptime page's 24-hour and 7-day shares
//!   count each check-in as a check and each missed run as a failed one.

use chrono::DateTime;
use openlens_meta::jobs::{Job, JobRun};
use openlens_meta::{Meta, MetaError, Project};
use serde::{Deserialize, Serialize};

use super::alerts::{Seen, track_at};
use super::{Access, Caller, Command, Error, Host};

/// The longest slug.
pub const MAX_SLUG: usize = 64;
/// Jobs a project may have (D-086): a new slug past it is refused.
pub const MAX_JOBS: i64 = 200;
/// Gaps between check-ins needed before a schedule is learned.
pub const MIN_GAPS: usize = 3;
/// Earlier durations needed before a run can be judged slower.
pub const MIN_DURATIONS: usize = 3;
/// A job is missed after this many times its usual gap with no check-in.
pub const LATE: f64 = 1.5;
/// A run is slower at this many times its usual duration or more.
pub const SLOWER: f64 = 2.0;
/// The shortest and longest gap a job may declare (`expectEvery`), in
/// seconds: one check-in per 10 seconds is the most the endpoint takes,
/// and a job that runs less than monthly is not watched by a heartbeat.
pub const EXPECT_MIN: i64 = 10;
/// See [`EXPECT_MIN`]: 31 days.
pub const EXPECT_MAX: i64 = 31 * 86_400;
/// The longest duration a check-in may report: 7 days, in milliseconds.
pub const MAX_MS: i64 = 7 * 86_400 * 1000;
/// The project setting of the availability switch.
const AVAILABILITY: &str = "jobs_availability";

fn storage(e: MetaError) -> Error {
    match e {
        MetaError::Invalid(m) => Error::Invalid(m.to_owned()),
        e => Error::Storage(e.to_string()),
    }
}

/// A job slug: 1 to [`MAX_SLUG`] of `a-z`, `0-9`, `_` and `-`.
///
/// # Errors
/// Anything else.
pub fn valid_slug(slug: &str) -> Result<(), Error> {
    if slug.is_empty()
        || slug.len() > MAX_SLUG
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(Error::Invalid(
            "slug: 1 to 64 lowercase letters, digits, '_' or '-'".into(),
        ));
    }
    Ok(())
}

fn median(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v.get(mid).copied()
    } else {
        Some((v.get(mid - 1)? + v.get(mid)?) / 2)
    }
}

/// The learned gap of a job, in seconds: the median gap between its
/// check-ins (oldest first), once there are [`MIN_GAPS`] of them. The
/// median keeps one long pause or one manual run from moving it.
#[must_use]
pub fn learned_gap(runs: &[JobRun]) -> Option<i64> {
    let gaps: Vec<i64> = runs
        .windows(2)
        .filter_map(|w| match w {
            [a, b] => Some(b.at - a.at),
            _ => None,
        })
        .filter(|g| *g > 0)
        .collect();
    if gaps.len() < MIN_GAPS {
        return None;
    }
    median(gaps)
}

/// The usual duration of these runs, in milliseconds: the median of those
/// that reported one, once [`MIN_DURATIONS`] did.
#[must_use]
pub fn usual_ms(runs: &[JobRun]) -> Option<i64> {
    let ms: Vec<i64> = runs.iter().filter_map(|r| r.ms).collect();
    if ms.len() < MIN_DURATIONS {
        return None;
    }
    median(ms)
}

/// A job as shown and judged.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobState {
    /// Its slug.
    pub slug: String,
    /// `learning` (no schedule yet, last run fine), `ok`, `missed`,
    /// `failed` or `slower` (the first of missed, failed, slower that
    /// applies).
    pub state: &'static str,
    /// The last check-in (Unix seconds).
    pub last_at: Option<i64>,
    /// The last check-in said it succeeded.
    pub last_ok: Option<bool>,
    /// The last run's duration, in milliseconds.
    pub last_ms: Option<i64>,
    /// The gap expected between check-ins, in seconds: declared or
    /// learned; `None` while learning.
    pub every: Option<i64>,
    /// `every` was declared by the job (`expectEvery`), not learned.
    pub declared: bool,
    /// When it counts as missed without a new check-in (Unix seconds).
    pub due_by: Option<i64>,
    /// The usual duration (median of the runs before the last), in ms.
    pub usual_ms: Option<i64>,
    /// Check-ins kept (at most 50).
    pub runs: usize,
    /// Check-ins still needed before the schedule is learned.
    pub to_learn: usize,
    /// The job is missed now.
    pub missed: bool,
    /// The last run failed.
    pub failed: bool,
    /// The last run took [`SLOWER`] times its usual duration or more.
    pub slower: bool,
}

fn utc(t: i64) -> String {
    DateTime::from_timestamp(t, 0)
        .map_or_else(String::new, |d| d.format("%Y-%m-%d %H:%M").to_string())
}

/// A gap in words (`45 s`, `15 min`, `1 h 30 min`, `2 days`).
#[must_use]
pub fn every_words(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs >= 86_400 && secs % 3600 == 0 {
        let h = secs / 3600;
        match (h / 24, h % 24) {
            (1, 0) => "1 day".to_owned(),
            (d, 0) => format!("{d} days"),
            (d, h) => format!("{d} d {h} h"),
        }
    } else {
        crate::maintenance::length(secs)
    }
}

/// A job's state at `now`.
#[must_use]
pub fn state_of(job: &Job, now: i64) -> JobState {
    let last = job.runs.last();
    let every = job.expect_every.or_else(|| learned_gap(&job.runs));
    let due_by = every
        .zip(last)
        .map(|(e, l)| l.at + (e as f64 * LATE).round() as i64);
    let before = job.runs.len().saturating_sub(1);
    let usual = usual_ms(job.runs.get(..before).unwrap_or_default());
    let missed = due_by.is_some_and(|d| now > d);
    let failed = last.is_some_and(|l| !l.ok);
    let slower = last
        .and_then(|l| l.ms)
        .zip(usual)
        .is_some_and(|(ms, u)| u > 0 && ms as f64 >= SLOWER * u as f64);
    let state = if missed {
        "missed"
    } else if failed {
        "failed"
    } else if slower {
        "slower"
    } else if every.is_none() {
        "learning"
    } else {
        "ok"
    };
    JobState {
        slug: job.slug.clone(),
        state,
        last_at: last.map(|l| l.at),
        last_ok: last.map(|l| l.ok),
        last_ms: last.and_then(|l| l.ms),
        every,
        declared: job.expect_every.is_some(),
        due_by,
        usual_ms: usual,
        runs: job.runs.len(),
        to_learn: if every.is_some() {
            0
        } else {
            (MIN_GAPS + 1).saturating_sub(job.runs.len())
        },
        missed,
        failed,
        slower,
    }
}

/// The three incident checks of a job: `(rule, failing, cause)`.
#[must_use]
pub fn checks(s: &JobState) -> [(String, bool, String); 3] {
    let slug = &s.slug;
    let last = s.last_at.map(utc).unwrap_or_default();
    let every = s.every.map(every_words).unwrap_or_default();
    let ratio = s
        .last_ms
        .zip(s.usual_ms)
        .filter(|(_, u)| *u > 0)
        .map_or(0.0, |(m, u)| m as f64 / u as f64);
    [
        (
            format!("job:{slug}:missed"),
            s.missed,
            format!("{slug}: no check-in since {last} UTC (usually every {every})"),
        ),
        (
            format!("job:{slug}:failed"),
            s.failed,
            format!("{slug}: the run of {last} UTC reported an error"),
        ),
        (
            format!("job:{slug}:slower"),
            s.slower,
            format!(
                "{slug}: the run of {last} UTC took {} ms, {ratio:.1} times its usual {} ms",
                s.last_ms.unwrap_or(0),
                s.usual_ms.unwrap_or(0)
            ),
        ),
    ]
}

/// Whether the project counts its jobs in availability (off by default).
#[must_use]
pub fn in_availability(meta: &Meta, project: &str) -> bool {
    meta.project_setting(project, AVAILABILITY)
        .ok()
        .flatten()
        .is_some_and(|v| v == "on")
}

/// What the jobs add to availability since `since` (Unix seconds):
/// `(good, total)`, each check-in one check (good when it said ok) and
/// each missed run one failed check.
#[must_use]
pub fn availability_counts(meta: &Meta, project: &str, since: i64) -> (usize, usize) {
    let (mut good, mut total) = (0, 0);
    for j in meta.jobs(project).unwrap_or_default() {
        for r in j.runs.iter().filter(|r| r.at >= since) {
            total += 1;
            good += usize::from(r.ok);
        }
    }
    total += meta
        .incidents(project, since)
        .unwrap_or_default()
        .iter()
        .filter(|i| i.1 >= since && i.0.starts_with("job:") && i.0.ends_with(":missed"))
        .count();
    (good, total)
}

/// Close the open incidents of a job without a message (a removed job).
fn close_quietly(meta: &Meta, project: &str, slug: &str, now: i64) {
    for kind in ["missed", "failed", "slower"] {
        if let Ok(Some((id, _, _))) = meta.open_incident(project, &format!("job:{slug}:{kind}")) {
            let _ = meta.end_incident(id, now);
        }
    }
}

/// The minute pass: each job's three checks become incidents (opened,
/// announced once, closed by the check-in that clears them); the
/// incidents of a job that was removed are closed without a message.
pub async fn alert_pass<H: Host>(host: &H, p: &Project) {
    alert_pass_at(host, p, chrono::Utc::now().timestamp()).await;
}

/// [`alert_pass`] at `now` (Unix seconds).
pub async fn alert_pass_at<H: Host>(host: &H, p: &Project, now: i64) {
    let meta = host.meta();
    let Ok(jobs) = meta.jobs(&p.id) else {
        return;
    };
    for job in &jobs {
        let s = state_of(job, now);
        for (rule, failing, cause) in checks(&s) {
            let seen = Seen {
                rule: &rule,
                failing,
                cause: &cause,
                wait: 0,
            };
            Box::pin(track_at(host, &p.id, &p.name, &seen, now)).await;
        }
    }
    let since = now - 90 * 86_400;
    for (rule, _, end, _, _) in meta.incidents(&p.id, since).unwrap_or_default() {
        let Some(slug) = rule
            .strip_prefix("job:")
            .and_then(|r| r.rsplit_once(':'))
            .map(|(s, _)| s)
        else {
            continue;
        };
        if end.is_none() && !jobs.iter().any(|j| j.slug == slug) {
            close_quietly(meta, &p.id, slug, now);
        }
    }
}

/// The input of `jobs.check_in`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckInInput {
    /// Project id (the endpoint takes it from the key).
    #[serde(default)]
    pub project: String,
    /// The job's slug.
    pub slug: String,
    /// `ok` (default) or `error`.
    #[serde(default)]
    pub status: Option<String>,
    /// How long the run took, in milliseconds.
    #[serde(default)]
    pub ms: Option<i64>,
    /// The gap expected between runs, in seconds ([`EXPECT_MIN`] to
    /// [`EXPECT_MAX`]): it replaces the learned one; 0 goes back to
    /// learning.
    #[serde(default, alias = "expectEvery")]
    pub expect_every: Option<i64>,
}

/// What `jobs.check_in` recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckedIn {
    /// The job.
    pub slug: String,
    /// It said it succeeded.
    pub ok: bool,
    /// When (Unix seconds).
    pub at: i64,
}

/// Check and record a check-in at `at` (Unix seconds).
///
/// # Errors
/// A bad slug, status, duration or gap, a new job past [`MAX_JOBS`], or
/// the store.
pub fn record(
    meta: &Meta,
    project: &str,
    input: &CheckInInput,
    at: i64,
) -> Result<CheckedIn, Error> {
    valid_slug(&input.slug)?;
    let ok = match input.status.as_deref() {
        None | Some("ok") => true,
        Some("error") => false,
        Some(_) => return Err(Error::Invalid("status: ok or error".into())),
    };
    if input.ms.is_some_and(|m| !(0..=MAX_MS).contains(&m)) {
        return Err(Error::Invalid(
            "ms: the run's duration in milliseconds, 0 to 7 days".into(),
        ));
    }
    if input
        .expect_every
        .is_some_and(|e| e != 0 && !(EXPECT_MIN..=EXPECT_MAX).contains(&e))
    {
        return Err(Error::Invalid(
            "expectEvery: seconds between runs, 10 to 31 days (0 to learn it again)".into(),
        ));
    }
    let run = JobRun {
        at,
        ok,
        ms: input.ms,
    };
    meta.record_job_run(project, &input.slug, &run, input.expect_every, MAX_JOBS)
        .map_err(storage)?;
    Ok(CheckedIn {
        slug: input.slug.clone(),
        ok,
        at,
    })
}

/// `jobs.check_in`: a job says it ran, with its status and duration. The
/// adapters limit it to one per job every 10 seconds.
pub struct CheckIn;

impl Command for CheckIn {
    const NAME: &'static str = "jobs.check_in";
    const ACCESS: Access = Access::Write;
    type Input = CheckInInput;
    type Output = CheckedIn;

    async fn run<H: Host>(host: &H, _: &Caller, input: CheckInInput) -> Result<CheckedIn, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        record(host.meta(), &p.id, &input, chrono::Utc::now().timestamp())
    }
}

/// A project, for the commands that only name it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInput {
    /// Project id.
    pub project: String,
}

/// The jobs of a project.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Jobs {
    /// The project counts its jobs in availability.
    pub in_availability: bool,
    /// Jobs it may have.
    pub max: i64,
    /// Every job, by slug.
    pub jobs: Vec<JobState>,
}

/// The jobs of a project at `now`.
///
/// # Errors
/// The store.
pub fn list(meta: &Meta, project: &str, now: i64) -> Result<Jobs, Error> {
    let jobs = meta.jobs(project).map_err(storage)?;
    Ok(Jobs {
        in_availability: in_availability(meta, project),
        max: MAX_JOBS,
        jobs: jobs.iter().map(|j| state_of(j, now)).collect(),
    })
}

/// `jobs.list`: every job with its last check-in, its schedule (declared
/// or learned), its usual duration and its state.
pub struct List;

impl Command for List {
    const NAME: &'static str = "jobs.list";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Jobs;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProjectInput) -> Result<Jobs, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        list(host.meta(), &p.id, chrono::Utc::now().timestamp())
    }
}

/// The input of `jobs.remove`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveInput {
    /// Project id.
    pub project: String,
    /// The job's slug.
    pub slug: String,
}

/// `jobs.remove`: forget a job and its runs, and close its incidents
/// without a message. A new check-in starts it again, learning. The
/// adapters write the audit entry.
pub struct Remove;

impl Command for Remove {
    const NAME: &'static str = "jobs.remove";
    const ACCESS: Access = Access::Write;
    type Input = RemoveInput;
    type Output = ();

    async fn run<H: Host>(host: &H, _: &Caller, input: RemoveInput) -> Result<(), Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        valid_slug(&input.slug)?;
        let meta = host.meta();
        if !meta.remove_job(&p.id, &input.slug).map_err(storage)? {
            return Err(Error::Invalid("no such job".into()));
        }
        close_quietly(meta, &p.id, &input.slug, chrono::Utc::now().timestamp());
        Ok(())
    }
}

/// The input of `jobs.set_availability`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityInput {
    /// Project id.
    pub project: String,
    /// Count the jobs in availability.
    pub on: bool,
}

/// `jobs.set_availability` (D-086): the project's one switch that counts
/// its jobs in the Uptime page's availability (off by default). The
/// adapters write the audit entry.
pub struct SetAvailability;

impl Command for SetAvailability {
    const NAME: &'static str = "jobs.set_availability";
    const ACCESS: Access = Access::Write;
    type Input = AvailabilityInput;
    type Output = bool;

    async fn run<H: Host>(host: &H, _: &Caller, input: AvailabilityInput) -> Result<bool, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        host.meta()
            .set_project_setting(&p.id, AVAILABILITY, if input.on { "on" } else { "" })
            .map_err(storage)?;
        Ok(input.on)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_bounded_to_a_safe_alphabet() {
        assert!(valid_slug("nightly-export_2").is_ok());
        for bad in ["", "Nightly", "a:b", "a/b", "a b", "é", &"a".repeat(65)] {
            assert!(valid_slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_median_of_an_even_count_is_between_the_middle_two() {
        let m = median(vec![10, 40, 20, 30]).unwrap_or(0);
        assert!(m > 20 && m < 30);
    }
}

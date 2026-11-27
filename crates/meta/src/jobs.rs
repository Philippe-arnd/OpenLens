//! Job check-ins (#609, D-086): the cron and background jobs that tell
//! OpenLens they ran, and their last runs. A sibling of the uptime
//! probes: the probe asks the app, a job reports itself.
//!
//! Bounded: at most `max_jobs` slugs per project (the caller's cap, D-086)
//! and the last [`KEEP_RUNS`] runs per slug; older runs are dropped as new
//! ones arrive.

use super::*;

/// The tables, created next to the main schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS jobs (
  project TEXT NOT NULL,
  slug TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expect_every INTEGER,
  PRIMARY KEY (project, slug)
);
CREATE TABLE IF NOT EXISTS job_runs (
  project TEXT NOT NULL,
  slug TEXT NOT NULL,
  at INTEGER NOT NULL,
  ok INTEGER NOT NULL,
  ms INTEGER
);
CREATE INDEX IF NOT EXISTS job_runs_slug ON job_runs (project, slug, at);
";

/// Runs kept per job: enough for a median gap and a median duration.
pub const KEEP_RUNS: i64 = 50;

/// One check-in of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct JobRun {
    /// When it checked in (Unix seconds).
    pub at: i64,
    /// It said it succeeded.
    pub ok: bool,
    /// How long it said it ran, in milliseconds.
    pub ms: Option<i64>,
}

/// A job and its kept runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Its slug (`[a-z0-9_-]`, at most 64).
    pub slug: String,
    /// First check-in (Unix seconds).
    pub created_at: i64,
    /// The gap the job declared (`expectEvery`, seconds), when it did.
    pub expect_every: Option<i64>,
    /// The kept runs, oldest first.
    pub runs: Vec<JobRun>,
}

impl Meta {
    /// Record a check-in of `slug`: a new slug is refused once the project
    /// has `max_jobs` (`Invalid`). `expect_every` `Some(0)` clears the
    /// declared gap, `Some(n)` sets it, `None` keeps it. Runs past
    /// [`KEEP_RUNS`] are dropped.
    pub fn record_job_run(
        &self,
        project: &str,
        slug: &str,
        run: &JobRun,
        expect_every: Option<i64>,
        max_jobs: i64,
    ) -> Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let known: bool = tx
                .prepare("SELECT 1 FROM jobs WHERE project = ?1 AND slug = ?2")?
                .exists(params![project, slug])?;
            if !known {
                let n: i64 = tx.query_row(
                    "SELECT count(*) FROM jobs WHERE project = ?1",
                    [project],
                    |r| r.get(0),
                )?;
                if n >= max_jobs {
                    return Err(MetaError::Invalid(
                        "too many jobs in this project: remove one on the Uptime page first",
                    ));
                }
                tx.execute(
                    "INSERT INTO jobs (project, slug, created_at) VALUES (?1, ?2, ?3)",
                    params![project, slug, run.at],
                )?;
            }
            if let Some(every) = expect_every {
                tx.execute(
                    "UPDATE jobs SET expect_every = ?3 WHERE project = ?1 AND slug = ?2",
                    params![project, slug, (every > 0).then_some(every)],
                )?;
            }
            tx.execute(
                "INSERT INTO job_runs (project, slug, at, ok, ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![project, slug, run.at, run.ok, run.ms],
            )?;
            tx.execute(
                "DELETE FROM job_runs WHERE project = ?1 AND slug = ?2 AND rowid NOT IN \
                 (SELECT rowid FROM job_runs WHERE project = ?1 AND slug = ?2 ORDER BY at DESC, rowid DESC LIMIT ?3)",
                params![project, slug, KEEP_RUNS],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Every job of a project, by slug, with its kept runs (oldest first).
    pub fn jobs(&self, project: &str) -> Result<Vec<Job>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT slug, created_at, expect_every FROM jobs WHERE project = ?1 ORDER BY slug",
            )?;
            let mut jobs = st
                .query_map([project], |r| {
                    Ok(Job {
                        slug: r.get(0)?,
                        created_at: r.get(1)?,
                        expect_every: r.get(2)?,
                        runs: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut st = c.prepare(
                "SELECT slug, at, ok, ms FROM job_runs WHERE project = ?1 ORDER BY slug, at, rowid",
            )?;
            let runs = st
                .query_map([project], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        JobRun {
                            at: r.get(1)?,
                            ok: r.get(2)?,
                            ms: r.get(3)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let at: HashMap<String, usize> = jobs
                .iter()
                .enumerate()
                .map(|(i, j)| (j.slug.clone(), i))
                .collect();
            for (slug, run) in runs {
                if let Some(j) = at.get(&slug).and_then(|i| jobs.get_mut(*i)) {
                    j.runs.push(run);
                }
            }
            Ok(jobs)
        })
    }

    /// Forget a job and its runs. False when there was none.
    pub fn remove_job(&self, project: &str, slug: &str) -> Result<bool> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM job_runs WHERE project = ?1 AND slug = ?2",
                params![project, slug],
            )?;
            let n = tx.execute(
                "DELETE FROM jobs WHERE project = ?1 AND slug = ?2",
                params![project, slug],
            )?;
            tx.commit()?;
            Ok(n > 0)
        })
    }
}

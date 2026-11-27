//! What the git host said about each recorded release (#574, D-092): the
//! commits of its range, the merged pull requests they belong to, the
//! files they changed and the matching tag, read once by a background
//! pass and kept here, so the Releases page, `release_report` and the
//! feature attribution (#576) read them without calling GitHub again.
//!
//! One row per (project, environment, release) in `release_reads`, with
//! the range it was read for and how the reads went; its commits, pull
//! requests and files in three child tables. The caller bounds what it
//! stores (D-092). No author is kept: not their name, login or email.
//! A row goes when its release has no deploy left (deploys are cut at
//! [`crate::deploys::MAX_DEPLOYS`] per project).

use super::*;

/// The tables, created next to the main schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS release_reads (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  from_sha TEXT NOT NULL,
  sha TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  tried_at INTEGER,
  error TEXT,
  fetched_at INTEGER,
  commits_total INTEGER NOT NULL DEFAULT 0,
  prs_total INTEGER NOT NULL DEFAULT 0,
  files_total INTEGER NOT NULL DEFAULT 0,
  first_commit_at INTEGER,
  tag TEXT,
  tag_url TEXT,
  PRIMARY KEY (project, environment, release)
);
CREATE TABLE IF NOT EXISTS release_read_commits (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  seq INTEGER NOT NULL,
  sha TEXT NOT NULL,
  at INTEGER
);
CREATE INDEX IF NOT EXISTS release_read_commits_rel ON release_read_commits (project, environment, release);
CREATE TABLE IF NOT EXISTS release_read_prs (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  number INTEGER NOT NULL,
  title TEXT NOT NULL,
  merged_at INTEGER,
  url TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS release_read_prs_rel ON release_read_prs (project, environment, release);
CREATE TABLE IF NOT EXISTS release_read_files (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  path TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS release_read_files_rel ON release_read_files (project, environment, release);
";

/// One commit of a release's range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    /// Full commit id.
    pub sha: String,
    /// When it was written (its author time, Unix seconds), if given.
    pub at: Option<i64>,
}

/// One merged pull request of a release's range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPr {
    /// Its number in the repository.
    pub number: i64,
    /// Its title.
    pub title: String,
    /// When it was merged (Unix seconds), if given.
    pub merged_at: Option<i64>,
    /// Its page.
    pub url: String,
}

/// What was read for a release, already bounded by the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitRead {
    /// The commits kept, oldest first.
    pub commits: Vec<GitCommit>,
    /// Commits in the range, kept or not.
    pub commits_total: i64,
    /// The merged pull requests kept, in merge order.
    pub prs: Vec<GitPr>,
    /// Merged pull requests found, kept or not.
    pub prs_total: i64,
    /// The changed file paths kept.
    pub files: Vec<String>,
    /// Changed files listed, kept or not.
    pub files_total: i64,
    /// The oldest author time of the range's commits (Unix seconds): the
    /// start of the release's lead time.
    pub first_commit_at: Option<i64>,
    /// The tag (or GitHub release) named like the release, if any.
    pub tag: Option<String>,
    /// The GitHub release's page, when there is one.
    pub tag_url: Option<String>,
}

/// Where the read of a release stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRead {
    /// The first commit of the range read (exclusive).
    pub from: String,
    /// The last commit of the range read.
    pub to: String,
    /// Failed reads since the last success.
    pub attempts: i64,
    /// Last try (Unix seconds).
    pub tried_at: Option<i64>,
    /// Why the last try failed.
    pub error: Option<String>,
    /// When it was read (Unix seconds); `None` until a read succeeds.
    pub fetched_at: Option<i64>,
    /// What was read, once read.
    pub read: Option<GitRead>,
}

/// A recorded release and its commit range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRange {
    /// Environment.
    pub environment: String,
    /// Release name.
    pub release: String,
    /// The first commit of the range (exclusive).
    pub from: String,
    /// The commit deployed.
    pub to: String,
}

/// The latest recorded deploy of each release that names a commit and
/// the start of its range (as `release_range` reads one release).
const LATEST_RANGES: &str = "SELECT d.environment, d.release, d.from_sha, d.sha, d.received_at, d.at \
     FROM deploys d WHERE d.project = ?1 AND d.release IS NOT NULL AND d.sha IS NOT NULL \
     AND d.from_sha IS NOT NULL AND d.rowid = (SELECT d2.rowid FROM deploys d2 \
     WHERE d2.project = d.project AND d2.environment = d.environment AND d2.release = d.release \
     AND d2.sha IS NOT NULL ORDER BY d2.at DESC, d2.rowid DESC LIMIT 1)";

impl Meta {
    /// Up to `limit` recorded releases of `project` with a commit and the
    /// start of its range, newest first: their latest deploy naming a
    /// commit.
    pub fn release_ranges(&self, project: &str, limit: i64) -> Result<Vec<ReleaseRange>> {
        self.read(|c| {
            let sql = format!(
                "SELECT r.environment, r.release, r.from_sha, r.sha FROM ({LATEST_RANGES}) r \
                 ORDER BY r.at DESC LIMIT ?2"
            );
            let mut st = c.prepare(&sql)?;
            let rows = st.query_map(params![project, limit], range_of)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Up to `limit` recorded releases of `project`, newest first, whose
    /// range has a commit and a start and is not read yet: never tried,
    /// recorded with another range, or failed fewer than `max_attempts`
    /// times. A release that gave up is tried again once it is recorded
    /// again.
    pub fn release_reads_pending(
        &self,
        project: &str,
        max_attempts: i64,
        limit: i64,
    ) -> Result<Vec<ReleaseRange>> {
        self.read(|c| {
            let sql = format!(
                "SELECT r.environment, r.release, r.from_sha, r.sha FROM ({LATEST_RANGES}) r \
                 WHERE NOT EXISTS (SELECT 1 FROM release_reads g WHERE g.project = ?1 \
                 AND g.environment = r.environment AND g.release = r.release \
                 AND g.from_sha = r.from_sha AND g.sha = r.sha \
                 AND (g.fetched_at IS NOT NULL \
                      OR (g.attempts >= ?2 AND coalesce(g.tried_at, 0) >= r.received_at))) \
                 ORDER BY r.at DESC LIMIT ?3"
            );
            let mut st = c.prepare(&sql)?;
            let rows = st.query_map(params![project, max_attempts, limit], range_of)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Keep what was read for a release's range `from..to`, replacing what
    /// was kept before. Rows of releases with no deploy left go too.
    pub fn store_release_read(
        &self,
        project: &str,
        p: &ReleaseRange,
        read: &GitRead,
        now: i64,
    ) -> Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let key = (project, p.environment.as_str(), p.release.as_str());
            for t in [
                "release_read_commits",
                "release_read_prs",
                "release_read_files",
            ] {
                tx.execute(
                    &format!(
                        "DELETE FROM {t} WHERE project = ?1 AND environment = ?2 AND release = ?3"
                    ),
                    key,
                )?;
            }
            tx.execute(
                "INSERT INTO release_reads (project, environment, release, from_sha, sha, attempts, \
                 tried_at, error, fetched_at, commits_total, prs_total, files_total, \
                 first_commit_at, tag, tag_url) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, NULL, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                 ON CONFLICT (project, environment, release) DO UPDATE SET \
                 from_sha = excluded.from_sha, sha = excluded.sha, attempts = 0, \
                 tried_at = excluded.tried_at, error = NULL, fetched_at = excluded.fetched_at, \
                 commits_total = excluded.commits_total, prs_total = excluded.prs_total, \
                 files_total = excluded.files_total, first_commit_at = excluded.first_commit_at, \
                 tag = excluded.tag, tag_url = excluded.tag_url",
                params![
                    project,
                    p.environment,
                    p.release,
                    p.from,
                    p.to,
                    now,
                    read.commits_total,
                    read.prs_total,
                    read.files_total,
                    read.first_commit_at,
                    read.tag,
                    read.tag_url
                ],
            )?;
            for (i, x) in read.commits.iter().enumerate() {
                tx.execute(
                    "INSERT INTO release_read_commits (project, environment, release, seq, sha, at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![project, p.environment, p.release, i as i64, x.sha, x.at],
                )?;
            }
            for x in &read.prs {
                tx.execute(
                    "INSERT INTO release_read_prs (project, environment, release, number, title, merged_at, url) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![project, p.environment, p.release, x.number, x.title, x.merged_at, x.url],
                )?;
            }
            for f in &read.files {
                tx.execute(
                    "INSERT INTO release_read_files (project, environment, release, path) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![project, p.environment, p.release, f],
                )?;
            }
            for t in [
                "release_reads",
                "release_read_commits",
                "release_read_prs",
                "release_read_files",
            ] {
                tx.execute(
                    &format!(
                        "DELETE FROM {t} WHERE project = ?1 AND NOT EXISTS (SELECT 1 FROM deploys d \
                         WHERE d.project = {t}.project AND d.environment = {t}.environment \
                         AND d.release = {t}.release)"
                    ),
                    [project],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Note a failed read of a release's range: one more attempt, and why.
    /// What an earlier range read is dropped (it no longer matches).
    pub fn fail_release_read(
        &self,
        project: &str,
        p: &ReleaseRange,
        error: &str,
        now: i64,
    ) -> Result<()> {
        let error: String = error.chars().take(300).collect();
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let same: bool = tx
                .prepare(
                    "SELECT 1 FROM release_reads WHERE project = ?1 AND environment = ?2 \
                     AND release = ?3 AND from_sha = ?4 AND sha = ?5",
                )?
                .exists(params![project, p.environment, p.release, p.from, p.to])?;
            if !same {
                for t in [
                    "release_read_commits",
                    "release_read_prs",
                    "release_read_files",
                ] {
                    tx.execute(
                        &format!(
                            "DELETE FROM {t} WHERE project = ?1 AND environment = ?2 AND release = ?3"
                        ),
                        params![project, p.environment, p.release],
                    )?;
                }
                tx.execute(
                    "DELETE FROM release_reads WHERE project = ?1 AND environment = ?2 AND release = ?3",
                    params![project, p.environment, p.release],
                )?;
            }
            tx.execute(
                "INSERT INTO release_reads (project, environment, release, from_sha, sha, attempts, tried_at, error) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7) \
                 ON CONFLICT (project, environment, release) DO UPDATE SET \
                 attempts = attempts + 1, tried_at = excluded.tried_at, error = excluded.error",
                params![project, p.environment, p.release, p.from, p.to, now, error],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Where the read of a release stands, with what was read once it
    /// succeeded. `None` when it was never tried.
    pub fn release_read(
        &self,
        project: &str,
        environment: &str,
        release: &str,
    ) -> Result<Option<ReleaseRead>> {
        self.read(|c| {
            let key = (project, environment, release);
            let row: Option<(ReleaseRead, Totals)> = c
                .query_row(
                    "SELECT from_sha, sha, attempts, tried_at, error, fetched_at, commits_total, \
                     prs_total, files_total, first_commit_at, tag, tag_url FROM release_reads \
                     WHERE project = ?1 AND environment = ?2 AND release = ?3",
                    key,
                    |r| {
                        Ok((
                            ReleaseRead {
                                from: r.get(0)?,
                                to: r.get(1)?,
                                attempts: r.get(2)?,
                                tried_at: r.get(3)?,
                                error: r.get(4)?,
                                fetched_at: r.get(5)?,
                                read: None,
                            },
                            Totals {
                                commits: r.get(6)?,
                                prs: r.get(7)?,
                                files: r.get(8)?,
                                first: r.get(9)?,
                                tag: r.get(10)?,
                                tag_url: r.get(11)?,
                            },
                        ))
                    },
                )
                .optional()?;
            let Some((mut state, t)) = row else {
                return Ok(None);
            };
            if state.fetched_at.is_none() {
                return Ok(Some(state));
            }
            let commits = c
                .prepare(
                    "SELECT sha, at FROM release_read_commits WHERE project = ?1 \
                     AND environment = ?2 AND release = ?3 ORDER BY seq",
                )?
                .query_map(key, |r| {
                    Ok(GitCommit {
                        sha: r.get(0)?,
                        at: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let prs = c
                .prepare(
                    "SELECT number, title, merged_at, url FROM release_read_prs WHERE project = ?1 \
                     AND environment = ?2 AND release = ?3 ORDER BY merged_at, number",
                )?
                .query_map(key, |r| {
                    Ok(GitPr {
                        number: r.get(0)?,
                        title: r.get(1)?,
                        merged_at: r.get(2)?,
                        url: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let files = files_of(c, project, environment, release)?;
            state.read = Some(GitRead {
                commits,
                commits_total: t.commits,
                prs,
                prs_total: t.prs,
                files,
                files_total: t.files,
                first_commit_at: t.first,
                tag: t.tag,
                tag_url: t.tag_url,
            });
            Ok(Some(state))
        })
    }

    /// The changed file paths read for a release (bounded when stored),
    /// empty until its range is read: what feature attribution (#576)
    /// maps to features.
    pub fn release_read_files(
        &self,
        project: &str,
        environment: &str,
        release: &str,
    ) -> Result<Vec<String>> {
        self.read(|c| files_of(c, project, environment, release))
    }
}

fn range_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReleaseRange> {
    Ok(ReleaseRange {
        environment: r.get(0)?,
        release: r.get(1)?,
        from: r.get(2)?,
        to: r.get(3)?,
    })
}

/// The totals and tag of a read release, as stored.
struct Totals {
    commits: i64,
    prs: i64,
    files: i64,
    first: Option<i64>,
    tag: Option<String>,
    tag_url: Option<String>,
}

fn files_of(
    c: &Connection,
    project: &str,
    environment: &str,
    release: &str,
) -> Result<Vec<String>> {
    Ok(c.prepare(
        "SELECT path FROM release_read_files WHERE project = ?1 AND environment = ?2 \
         AND release = ?3 ORDER BY rowid",
    )?
    .query_map((project, environment, release), |r| r.get(0))?
    .collect::<rusqlite::Result<Vec<_>>>()?)
}

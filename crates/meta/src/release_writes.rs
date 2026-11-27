//! What was written back to the git host for each release (#575, D-096):
//! the commit status, the note in the GitHub release, the issue opened on
//! a regressed verdict and one comment per shipped pull request, each with
//! the verdict it says. The background pass reads this before it writes,
//! so a restart or a second pass never posts twice, and a changed verdict
//! updates what is there instead of adding to it.
//!
//! One row per (project, environment, release) in `release_writes`, and
//! one per pull request commented in `release_write_comments`. Nothing
//! but GitHub's own ids, our verdicts and GitHub's refusals is kept. A row
//! goes when its release has no deploy left, as for `release_reads`.

use super::*;

/// The tables, created next to the main schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS release_writes (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  target TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  tried_at INTEGER,
  error TEXT,
  denied INTEGER NOT NULL DEFAULT 0,
  written_at INTEGER,
  status_verdict TEXT,
  status_sha TEXT,
  note_verdict TEXT,
  issue_number INTEGER,
  issue_url TEXT,
  issue_verdict TEXT,
  PRIMARY KEY (project, environment, release)
);
CREATE TABLE IF NOT EXISTS release_write_comments (
  project TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT NOT NULL,
  pr INTEGER NOT NULL,
  comment_id INTEGER NOT NULL,
  verdict TEXT NOT NULL,
  PRIMARY KEY (project, environment, release, pr)
);
";

/// Where the write-back of one release stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseWrite {
    /// The verdict being written (`healthy` or `regressed`).
    pub target: String,
    /// Passes that failed since the target was set.
    pub attempts: i64,
    /// Last pass that wrote or tried (Unix seconds).
    pub tried_at: Option<i64>,
    /// Why the last pass failed, as GitHub said.
    pub error: Option<String>,
    /// Every failure of the last pass was a refusal (403 or 404): the
    /// token lacks a permission, or cannot see the repository.
    pub denied: bool,
    /// When everything said the target (Unix seconds).
    pub written_at: Option<i64>,
    /// The commit status set: (verdict, commit).
    pub status: Option<(String, String)>,
    /// The verdict written into the GitHub release.
    pub note: Option<String>,
    /// The issue opened on a regressed verdict: (number, page).
    pub issue: Option<(i64, String)>,
    /// The verdict the issue was last told.
    pub issue_verdict: Option<String>,
}

/// The comment on one shipped pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrComment {
    /// The pull request's number.
    pub pr: i64,
    /// The comment's id on GitHub.
    pub id: i64,
    /// The verdict it says.
    pub verdict: String,
}

impl Meta {
    /// Where the write-back of a release stands; `None` when nothing was
    /// tried.
    pub fn release_write(
        &self,
        project: &str,
        environment: &str,
        release: &str,
    ) -> Result<Option<ReleaseWrite>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT target, attempts, tried_at, error, denied, written_at, status_verdict, \
                 status_sha, note_verdict, issue_number, issue_url, issue_verdict \
                 FROM release_writes WHERE project = ?1 AND environment = ?2 AND release = ?3",
                (project, environment, release),
                |r| {
                    let status: (Option<String>, Option<String>) = (r.get(6)?, r.get(7)?);
                    let issue: (Option<i64>, Option<String>) = (r.get(9)?, r.get(10)?);
                    Ok(ReleaseWrite {
                        target: r.get(0)?,
                        attempts: r.get(1)?,
                        tried_at: r.get(2)?,
                        error: r.get(3)?,
                        denied: r.get::<_, i64>(4)? != 0,
                        written_at: r.get(5)?,
                        status: match status {
                            (Some(v), Some(s)) => Some((v, s)),
                            _ => None,
                        },
                        note: r.get(8)?,
                        issue: match issue {
                            (Some(n), u) => Some((n, u.unwrap_or_default())),
                            _ => None,
                        },
                        issue_verdict: r.get(11)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Keep where the write-back of a release stands, replacing the row.
    /// Rows of releases with no deploy left go too.
    pub fn save_release_write(
        &self,
        project: &str,
        environment: &str,
        release: &str,
        w: &ReleaseWrite,
    ) -> Result<()> {
        let error: Option<String> = w.error.as_ref().map(|e| e.chars().take(300).collect());
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO release_writes (project, environment, release, target, attempts, \
                 tried_at, error, denied, written_at, status_verdict, status_sha, note_verdict, \
                 issue_number, issue_url, issue_verdict) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) \
                 ON CONFLICT (project, environment, release) DO UPDATE SET \
                 target = excluded.target, attempts = excluded.attempts, \
                 tried_at = excluded.tried_at, error = excluded.error, denied = excluded.denied, \
                 written_at = excluded.written_at, status_verdict = excluded.status_verdict, \
                 status_sha = excluded.status_sha, note_verdict = excluded.note_verdict, \
                 issue_number = excluded.issue_number, issue_url = excluded.issue_url, \
                 issue_verdict = excluded.issue_verdict",
                params![
                    project,
                    environment,
                    release,
                    w.target,
                    w.attempts,
                    w.tried_at,
                    error,
                    i64::from(w.denied),
                    w.written_at,
                    w.status.as_ref().map(|s| &s.0),
                    w.status.as_ref().map(|s| &s.1),
                    w.note,
                    w.issue.as_ref().map(|i| i.0),
                    w.issue.as_ref().map(|i| &i.1),
                    w.issue_verdict,
                ],
            )?;
            for t in ["release_writes", "release_write_comments"] {
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

    /// The comments written on a release's pull requests.
    pub fn release_write_comments(
        &self,
        project: &str,
        environment: &str,
        release: &str,
    ) -> Result<Vec<PrComment>> {
        self.read(|c| {
            Ok(c.prepare(
                "SELECT pr, comment_id, verdict FROM release_write_comments WHERE project = ?1 \
                 AND environment = ?2 AND release = ?3 ORDER BY pr",
            )?
            .query_map((project, environment, release), |r| {
                Ok(PrComment {
                    pr: r.get(0)?,
                    id: r.get(1)?,
                    verdict: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Keep the comment written (or updated) on one pull request of a
    /// release, right after GitHub accepted it.
    pub fn save_release_write_comment(
        &self,
        project: &str,
        environment: &str,
        release: &str,
        c: &PrComment,
    ) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO release_write_comments (project, environment, release, pr, comment_id, verdict) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT (project, environment, release, pr) DO UPDATE SET \
                 comment_id = excluded.comment_id, verdict = excluded.verdict",
                params![project, environment, release, c.pr, c.id, c.verdict],
            )?;
            Ok(())
        })
    }
}

//! Deploys reported by the deploy hook (#504, D-053), and the hook's
//! per-project secret.
//!
//! The secret is shown once, when it is made; only its SHA-256 is kept,
//! like API tokens and share links. Deploys are a small table, at most
//! [`MAX_DEPLOYS`] per project (oldest first out). A release recorded by
//! its deploy step (`openlens-cli release`, #521, D-066) is a succeeded
//! deploy that also keeps the first commit of its range, and when that
//! commit was made when the deploy step knows it (#633).

use super::*;

/// Deploys kept per project; past it the oldest go.
pub const MAX_DEPLOYS: i64 = 500;

/// Prefix of a deploy hook secret.
pub const HOOK_PREFIX: &str = "dh_";

/// Length of a deploy hook secret, prefix included.
pub const HOOK_LEN: usize = HOOK_PREFIX.len() + 40;

/// One deploy notification, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deploy {
    /// Environment deployed to.
    pub environment: String,
    /// Release name, when the platform gave one.
    pub release: Option<String>,
    /// Commit deployed, when the platform gave one.
    pub commit: Option<String>,
    /// `started`, `succeeded`, `failed` or `rolled_back`.
    pub status: String,
    /// When it happened (Unix seconds).
    pub at: i64,
    /// The format it came in (`github`, `vercel`, `plain`…).
    pub source: String,
}

/// The commit range of a recorded release: `from..commit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Range<'a> {
    /// The commit the range starts after.
    pub from: Option<&'a str>,
    /// When the first commit of the range was made (Unix seconds), as
    /// the deploy step's git checkout says (#633).
    pub first_commit_at: Option<i64>,
}

impl Meta {
    /// Make a new deploy hook secret for `project`, replacing the old one
    /// (which stops working at once). Returns it: it is not kept, only its
    /// hash. `None` when the project does not exist.
    pub fn rotate_deploy_hook(&self, project: &str) -> Result<Option<String>> {
        let secret = format!("{HOOK_PREFIX}{}", token(HOOK_LEN - HOOK_PREFIX.len()));
        let now = Utc::now().timestamp();
        self.with(|c| {
            let exists = c
                .prepare("SELECT 1 FROM projects WHERE id = ?1")?
                .exists([project])?;
            if !exists {
                return Ok(None);
            }
            c.execute(
                "INSERT INTO deploy_hooks (project, hash, created_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (project) DO UPDATE SET hash = excluded.hash, created_at = excluded.created_at",
                params![project, sha256_hex(&secret), now],
            )?;
            Ok(Some(secret))
        })
    }

    /// The project a deploy hook secret belongs to.
    pub fn deploy_hook_project(&self, secret: &str) -> Result<Option<String>> {
        if secret.len() != HOOK_LEN || !secret.starts_with(HOOK_PREFIX) {
            return Ok(None);
        }
        self.read(|c| {
            Ok(c.query_row(
                "SELECT project FROM deploy_hooks WHERE hash = ?1",
                [sha256_hex(secret)],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// When the project's current deploy hook secret was made (Unix
    /// seconds), if it has one.
    pub fn deploy_hook_since(&self, project: &str) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT created_at FROM deploy_hooks WHERE project = ?1",
                [project],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Record a deploy, then cut the project back to [`MAX_DEPLOYS`].
    pub fn record_deploy(&self, project: &str, d: &Deploy, received_at: i64) -> Result<()> {
        self.insert_deploy(project, d, &Range::default(), received_at)
    }

    /// Record a release named by its deploy step (`openlens-cli release`,
    /// #521): a deploy with its commit range.
    pub fn record_release(
        &self,
        project: &str,
        d: &Deploy,
        range: &Range<'_>,
        received_at: i64,
    ) -> Result<()> {
        self.insert_deploy(project, d, range, received_at)
    }

    fn insert_deploy(
        &self,
        project: &str,
        d: &Deploy,
        range: &Range<'_>,
        received_at: i64,
    ) -> Result<()> {
        if d.environment.is_empty() || d.status.is_empty() {
            return Err(MetaError::Invalid("empty environment or status"));
        }
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO deploys (project, environment, release, sha, status, at, source, received_at, from_sha, first_commit_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    project,
                    d.environment,
                    d.release,
                    d.commit,
                    d.status,
                    d.at,
                    d.source,
                    received_at,
                    range.from,
                    range.first_commit_at
                ],
            )?;
            tx.execute(
                "DELETE FROM deploys WHERE project = ?1 AND rowid NOT IN \
                 (SELECT rowid FROM deploys WHERE project = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2)",
                params![project, MAX_DEPLOYS],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// The commit of the environment's latest succeeded deploy that has
    /// one other than `commit`: where the next release's range starts.
    pub fn previous_commit(
        &self,
        project: &str,
        environment: &str,
        commit: Option<&str>,
    ) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT sha FROM deploys WHERE project = ?1 AND environment = ?2 \
                 AND status = 'succeeded' AND sha IS NOT NULL AND sha <> coalesce(?3, '') \
                 ORDER BY at DESC, rowid DESC LIMIT 1",
                params![project, environment, commit],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// The commit range recorded for a release in an environment, as
    /// `(from, to)`: its latest recorded deploy that names a commit.
    pub fn release_range(
        &self,
        project: &str,
        environment: &str,
        release: &str,
    ) -> Result<Option<(Option<String>, String)>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT from_sha, sha FROM deploys WHERE project = ?1 AND environment = ?2 \
                 AND release = ?3 AND sha IS NOT NULL ORDER BY at DESC, rowid DESC LIMIT 1",
                params![project, environment, release],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// When the first commit of each release deployed at or after `since`
    /// (Unix seconds) was made: `(release, first commit time)`, oldest
    /// first, so that the last pair of a release is the one to keep. The
    /// time the deploy step recorded (#633) wins over the one the git
    /// host read gave (#574), for releases recorded without the CLI's
    /// time; within each, the latest record wins.
    pub fn first_commits(&self, project: &str, since: i64) -> Result<Vec<(String, i64)>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT release, t FROM ( \
                   SELECT g.release AS release, g.first_commit_at AS t, 0 AS rank, \
                          coalesce(g.fetched_at, 0) AS at, g.rowid AS rid \
                   FROM release_reads g WHERE g.project = ?1 AND g.first_commit_at IS NOT NULL \
                   AND EXISTS (SELECT 1 FROM deploys d WHERE d.project = ?1 \
                               AND d.release = g.release AND d.at >= ?2) \
                   UNION ALL \
                   SELECT release, first_commit_at, 1, at, rowid FROM deploys \
                   WHERE project = ?1 AND at >= ?2 AND release IS NOT NULL \
                   AND first_commit_at IS NOT NULL \
                 ) ORDER BY rank, at, rid LIMIT ?3",
            )?;
            let rows = st.query_map(params![project, since, 2 * MAX_DEPLOYS], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The project's deploys at or after `since` (Unix seconds), oldest
    /// first (at most [`MAX_DEPLOYS`]).
    pub fn deploys(&self, project: &str, since: i64) -> Result<Vec<Deploy>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT environment, release, sha, status, at, source FROM deploys \
                 WHERE project = ?1 AND at >= ?2 ORDER BY at, rowid LIMIT ?3",
            )?;
            let rows = st.query_map(params![project, since, MAX_DEPLOYS], |r| {
                Ok(Deploy {
                    environment: r.get(0)?,
                    release: r.get(1)?,
                    commit: r.get(2)?,
                    status: r.get(3)?,
                    at: r.get(4)?,
                    source: r.get(5)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

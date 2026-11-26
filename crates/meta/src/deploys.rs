//! Deploys reported by the deploy hook (#504, D-053), and the hook's
//! per-project secret.
//!
//! The secret is shown once, when it is made; only its SHA-256 is kept,
//! like API tokens and share links. Deploys are a small table, at most
//! [`MAX_DEPLOYS`] per project (oldest first out).

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
        if d.environment.is_empty() || d.status.is_empty() {
            return Err(MetaError::Invalid("empty environment or status"));
        }
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO deploys (project, environment, release, sha, status, at, source, received_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    project,
                    d.environment,
                    d.release,
                    d.commit,
                    d.status,
                    d.at,
                    d.source,
                    received_at
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

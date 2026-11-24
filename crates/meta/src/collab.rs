//! Share links, timeline notes, the audit log and replay comments. Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
    /// Create a share link; returns `(link, token)`. At most 50 active
    /// links per project.
    #[allow(clippy::too_many_arguments)]
    pub fn create_share_link(
        &self,
        project: &str,
        scope: &str,
        environment: &str,
        release: Option<&str>,
        embed: Option<&str>,
        expires_at: Option<i64>,
        created_by: &str,
    ) -> Result<(ShareLink, String)> {
        if scope != "board" && scope != "digest" {
            return Err(MetaError::Invalid("scope must be board or digest"));
        }
        let now = Utc::now().timestamp();
        let id = token(10);
        let secret = format!("sh_{}", token(32));
        self.with(|c| {
            let active: i64 = c.query_row(
                "SELECT count(*) FROM share_links WHERE project = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                params![project, now],
                |r| r.get(0),
            )?;
            if active >= 50 {
                return Err(MetaError::Invalid("at most 50 active share links per project; revoke some"));
            }
            c.execute(
                "INSERT INTO share_links (hash, id, project, scope, environment, release, embed, expires_at, created_at, created_by) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![sha256_hex(&secret), id, project, scope, environment, release, embed, expires_at, now, created_by],
            )?;
            let link = c.query_row(
                &format!("SELECT {SHARE_COLS} FROM share_links WHERE id = ?1"),
                [&id],
                share_from_row,
            )?;
            Ok((link, secret))
        })
    }

    /// The live share link a token opens (not expired, not revoked).
    pub fn share_link(&self, token: &str) -> Result<Option<ShareLink>> {
        let now = Utc::now().timestamp();
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {SHARE_COLS} FROM share_links WHERE hash = ?1 AND (expires_at IS NULL OR expires_at > ?2)"),
                params![sha256_hex(token), now],
                share_from_row,
            )
            .optional()?)
        })
    }

    /// A project's share links, newest first (expired ones too).
    pub fn share_links(&self, project: &str) -> Result<Vec<ShareLink>> {
        self.read(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {SHARE_COLS} FROM share_links WHERE project = ?1 ORDER BY created_at DESC"
            ))?;
            let rows = st
                .query_map([project], share_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Revoke a share link (it stops working at once).
    pub fn revoke_share_link(&self, project: &str, id: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM share_links WHERE project = ?1 AND id = ?2",
                params![project, id],
            )? == 1)
        })
    }

    /// Add a note to a project's timeline (#267): a day and 1–500 chars.
    pub fn add_note(
        &self,
        project: &str,
        day: &str,
        author_id: &str,
        author: &str,
        body: &str,
    ) -> Result<i64> {
        let body = body.trim();
        if body.is_empty() || body.chars().count() > 500 {
            return Err(MetaError::Invalid("a note is 1 to 500 characters"));
        }
        if chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_err() {
            return Err(MetaError::Invalid("the day is YYYY-MM-DD"));
        }
        self.with(|c| {
            c.execute(
                "INSERT INTO notes (project, day, author, author_id, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![project, day, author, author_id, body, Utc::now().timestamp()],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Notes from `since` (YYYY-MM-DD) on, newest first:
    /// `(id, day, author, author_id, body)`.
    pub fn notes(&self, project: &str, since: &str) -> Result<Vec<Note>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, day, author, author_id, body FROM notes WHERE project = ?1 AND day >= ?2 ORDER BY day DESC, id DESC LIMIT 500",
            )?;
            let rows = st
                .query_map(params![project, since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Delete a note if `author_id` wrote it or `admin`.
    pub fn delete_note(
        &self,
        project: &str,
        id: i64,
        author_id: &str,
        admin: bool,
    ) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM notes WHERE project = ?1 AND id = ?2 AND (?4 OR author_id = ?3)",
                params![project, id, author_id, admin],
            )? == 1)
        })
    }

    /// A project's audit entries since `since` (Unix seconds), oldest first.
    pub fn project_audit(&self, project: &str, since: i64) -> Result<Vec<AuditEntry>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT at, actor, project, action, detail FROM audit WHERE project = ?1 AND at >= ?2 ORDER BY id LIMIT 1000",
            )?;
            let rows = st
                .query_map(params![project, since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Append to the audit log (#230). It has no delete.
    pub fn audit(
        &self,
        actor: &str,
        project: Option<&str>,
        action: &str,
        detail: &str,
    ) -> Result<()> {
        let detail: String = detail.chars().take(500).collect();
        self.with(|c| {
            c.execute(
                "INSERT INTO audit (at, actor, project, action, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![Utc::now().timestamp(), actor, project, action, detail],
            )?;
            Ok(())
        })
    }

    /// The audit log, newest first: `(at, actor, project, action, detail)`.
    pub fn audit_log(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT at, actor, project, action, detail FROM audit ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = st
                .query_map([i64::try_from(limit.min(1000)).unwrap_or(1000)], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Pin a comment to a moment of a replay. The body is plain text,
    /// 1 to 2000 characters.
    pub fn add_replay_comment(
        &self,
        project: &str,
        session: &str,
        at_ms: i64,
        user_id: &str,
        author: &str,
        body: &str,
    ) -> Result<ReplayComment> {
        let body = body.trim();
        if body.is_empty() || body.chars().count() > 2000 {
            return Err(MetaError::Invalid("a comment is 1 to 2000 characters"));
        }
        if !(0..=7 * 24 * 3600 * 1000).contains(&at_ms) {
            return Err(MetaError::Invalid("the time is outside the recording"));
        }
        let now = Utc::now().to_rfc3339();
        self.with(|c| {
            c.execute(
                "INSERT INTO replay_comments (project, session, at_ms, user_id, author, body, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![project, session, at_ms, user_id, author, body, now],
            )?;
            Ok(ReplayComment {
                id: c.last_insert_rowid(),
                session: session.to_owned(),
                at_ms,
                user_id: user_id.to_owned(),
                author: author.to_owned(),
                body: body.to_owned(),
                created_at: now,
            })
        })
    }

    /// A replay's comments, in playback order.
    pub fn replay_comments(&self, project: &str, session: &str) -> Result<Vec<ReplayComment>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, session, at_ms, user_id, author, body, created_at FROM replay_comments \
                 WHERE project = ?1 AND session = ?2 ORDER BY at_ms, id",
            )?;
            let rows = st
                .query_map(params![project, session], |r| {
                    Ok(ReplayComment {
                        id: r.get(0)?,
                        session: r.get(1)?,
                        at_ms: r.get(2)?,
                        user_id: r.get(3)?,
                        author: r.get(4)?,
                        body: r.get(5)?,
                        created_at: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Comments per session of a project.
    pub fn replay_comment_counts(&self, project: &str) -> Result<Vec<(String, i64)>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT session, count(*) FROM replay_comments WHERE project = ?1 GROUP BY session",
            )?;
            let rows = st
                .query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Delete a comment if `user_id` wrote it or `admin`. Returns false when
    /// there was nothing they may delete.
    pub fn delete_replay_comment(
        &self,
        project: &str,
        id: i64,
        user_id: &str,
        admin: bool,
    ) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM replay_comments WHERE project = ?1 AND id = ?2 AND (?4 OR user_id = ?3)",
                params![project, id, user_id, admin],
            )? == 1)
        })
    }
}

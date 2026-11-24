//! Login links and API tokens. Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
    /// Create a one-time login link token valid for `ttl_secs`. Only its
    /// SHA-256 is stored, so a leaked database does not leak live links.
    pub fn create_login_link(&self, ttl_secs: i64) -> Result<String> {
        self.create_login_link_for(None, ttl_secs)
    }

    /// Create an API token : `scope` is `read`, `write` or `admin`;
    /// `projects` limits it (`None`: every project). Only its hash is
    /// stored; the token is returned once.
    pub fn create_api_token(
        &self,
        scope: &str,
        projects: Option<&[String]>,
        label: &str,
    ) -> Result<(ApiToken, String)> {
        if !matches!(scope, "read" | "write" | "admin") {
            return Err(MetaError::Invalid("scope must be read, write or admin"));
        }
        let plain = format!("olt_{}", token(40));
        let t = ApiToken {
            id: format!("tok_{}", token(12)),
            scope: scope.to_owned(),
            projects: projects.map(<[String]>::to_vec),
            label: label.chars().take(100).collect(),
            created_at: Utc::now().to_rfc3339(),
            last_used: None,
        };
        self.with(|c| {
            c.execute(
                "INSERT INTO api_tokens (id, hash, scope, projects, label, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    t.id,
                    sha256_hex(&plain),
                    t.scope,
                    t.projects.as_ref().map(|p| p.join(",")),
                    t.label,
                    t.created_at
                ],
            )?;
            Ok(())
        })?;
        Ok((t, plain))
    }

    /// The API token with this value, if it exists (its use is recorded).
    pub fn api_token(&self, plain: &str) -> Result<Option<ApiToken>> {
        if plain.len() > 100 || !plain.starts_with("olt_") {
            return Ok(None);
        }
        let hash = sha256_hex(plain);
        let found = self.with(|c| {
            c.query_row(
                "SELECT id, scope, projects, label, created_at, last_used FROM api_tokens WHERE hash = ?1",
                [&hash],
                api_token_row,
            )
            .optional()
            .map_err(Into::into)
        })?;
        if found.is_some() {
            let now = Utc::now().to_rfc3339();
            self.with(|c| {
                c.execute(
                    "UPDATE api_tokens SET last_used = ?2 WHERE hash = ?1",
                    params![hash, now],
                )?;
                Ok(())
            })?;
        }
        Ok(found)
    }

    /// Every API token (never their values).
    pub fn api_tokens(&self) -> Result<Vec<ApiToken>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, scope, projects, label, created_at, last_used FROM api_tokens ORDER BY created_at",
            )?;
            let rows = st.query_map([], api_token_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Revoke an API token; false if there was none with this id.
    pub fn revoke_api_token(&self, id: &str) -> Result<bool> {
        self.with(|c| Ok(c.execute("DELETE FROM api_tokens WHERE id = ?1", [id])? > 0))
    }

    /// A one-time login link for a user (`None`: the owner, i.e. admin of
    /// everything, as with the admin token).
    pub fn create_login_link_for(&self, user_id: Option<&str>, ttl_secs: i64) -> Result<String> {
        let token = format!("ll_{}", token(40));
        let expires = Utc::now().timestamp() + ttl_secs;
        self.with(|c| {
            c.execute(
                "DELETE FROM login_links WHERE expires_at < ?1",
                [Utc::now().timestamp()],
            )?;
            c.execute(
                "INSERT INTO login_links (hash, expires_at, user_id) VALUES (?1, ?2, ?3)",
                params![sha256_hex(&token), expires, user_id],
            )?;
            Ok(token)
        })
    }

    /// Use a login link once. `Some(None)`: an owner link; `Some(Some(id))`:
    /// a link for that user; `None`: invalid, used or expired.
    pub fn consume_login_link(&self, token: &str) -> Result<Option<Option<String>>> {
        if token.len() > 100 {
            return Ok(None);
        }
        self.with(|c| {
            let hash = sha256_hex(token);
            let row: Option<Option<String>> = c
                .query_row(
                    "SELECT user_id FROM login_links WHERE hash = ?1 AND expires_at >= ?2",
                    params![hash, Utc::now().timestamp()],
                    |r| r.get(0),
                )
                .optional()?;
            if row.is_some() {
                c.execute("DELETE FROM login_links WHERE hash = ?1", [&hash])?;
            }
            Ok(row)
        })
    }
}

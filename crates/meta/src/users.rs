//! Dashboard users and their accounts. Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
    /// Invite or update a user. `role` is `admin` or `viewer`; `projects`
    /// limits what they see (`None`: all).
    pub fn upsert_user(
        &self,
        email: &str,
        role: &str,
        projects: Option<&[String]>,
    ) -> Result<User> {
        let email = email.trim().to_lowercase();
        if email.len() > 200 || !email.contains('@') || email.chars().any(char::is_whitespace) {
            return Err(MetaError::Invalid("invalid email"));
        }
        if role != "admin" && role != "viewer" {
            return Err(MetaError::Invalid("role must be admin or viewer"));
        }
        let projects = projects.map(|p| {
            p.iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(",")
        });
        let id = slug(&email.replace('@', "-at-"));
        self.with(|c| {
            c.execute(
                "INSERT INTO users (id, email, role, projects, created_at) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(email) DO UPDATE SET role = excluded.role, projects = excluded.projects",
                params![id, email, role, projects, Utc::now().to_rfc3339()],
            )?;
            Ok(c.query_row(
                &format!("{USER_SELECT} WHERE email = ?1"),
                [&email],
                user_from_row,
            )?)
        })
    }

    /// One user by id.
    pub fn user(&self, id: &str) -> Result<Option<User>> {
        self.read(|c| {
            Ok(
                c.query_row(&format!("{USER_SELECT} WHERE id = ?1"), [id], user_from_row)
                    .optional()?,
            )
        })
    }

    /// One user by email (case-insensitive).
    pub fn user_by_email(&self, email: &str) -> Result<Option<User>> {
        let email = email.trim().to_lowercase();
        self.read(|c| {
            Ok(c.query_row(
                &format!("{USER_SELECT} WHERE email = ?1"),
                [&email],
                user_from_row,
            )
            .optional()?)
        })
    }

    /// Every user, by email.
    pub fn users(&self) -> Result<Vec<User>> {
        self.read(|c| {
            let mut st = c.prepare(&format!("{USER_SELECT} ORDER BY email"))?;
            let rows = st
                .query_map([], user_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Would removing admin rights from `id` leave no enabled admin among
    /// the users? (The owner, holding the admin token, always remains.)
    fn last_admin(c: &Connection, id: &str) -> Result<bool> {
        let me: Option<(String, i64)> = c
            .query_row(
                "SELECT role, disabled FROM users WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if !matches!(me, Some((ref r, 0)) if r == "admin") {
            return Ok(false);
        }
        let others: i64 = c.query_row(
            "SELECT count(*) FROM users WHERE role = 'admin' AND disabled = 0 AND id <> ?1",
            [id],
            |r| r.get(0),
        )?;
        Ok(others == 0)
    }

    /// Change a user's name, email, role and projects (the id stays, so
    /// sessions survive an email change). Refuses to demote the last admin.
    pub fn update_user(
        &self,
        id: &str,
        name: Option<&str>,
        email: &str,
        role: &str,
        projects: Option<&[String]>,
    ) -> Result<Option<User>> {
        let email = email.trim().to_lowercase();
        if email.len() > 200 || !email.contains('@') || email.chars().any(char::is_whitespace) {
            return Err(MetaError::Invalid("invalid email"));
        }
        if role != "admin" && role != "viewer" {
            return Err(MetaError::Invalid("role must be admin or viewer"));
        }
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if name.is_some_and(|n| n.chars().count() > 80) {
            return Err(MetaError::Invalid("name must be at most 80 characters"));
        }
        let projects = projects.map(|p| {
            p.iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(",")
        });
        self.with(|c| {
            if role != "admin" && Self::last_admin(c, id)? {
                return Err(MetaError::Invalid("the last admin must stay an admin"));
            }
            let n = c
                .execute(
                    "UPDATE users SET name = ?2, email = ?3, role = ?4, projects = ?5 WHERE id = ?1",
                    params![id, name, email, role, projects],
                )
                .map_err(|e| match e {
                    rusqlite::Error::SqliteFailure(f, _)
                        if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        MetaError::Invalid("another user has that email")
                    }
                    e => e.into(),
                })?;
            if n == 0 {
                return Ok(None);
            }
            Ok(c.query_row(&format!("{} WHERE id = ?1", USER_SELECT), [id], user_from_row)
                .optional()?)
        })
    }

    /// Disable or enable sign-in. Disabling ends the user's sessions at
    /// once and drops their pending links. Refuses the last admin.
    pub fn set_user_disabled(&self, id: &str, disabled: bool) -> Result<bool> {
        self.with(|c| {
            if disabled && Self::last_admin(c, id)? {
                return Err(MetaError::Invalid("the last admin cannot be disabled"));
            }
            if disabled {
                c.execute("DELETE FROM login_links WHERE user_id = ?1", [id])?;
            }
            Ok(c.execute(
                "UPDATE users SET disabled = ?2 WHERE id = ?1",
                params![id, i64::from(disabled)],
            )? == 1)
        })
    }

    /// Set (or clear, with `None`) the time zone a user reads times in.
    pub fn set_user_timezone(&self, id: &str, timezone: Option<&str>) -> Result<bool> {
        if let Some(tz) = timezone
            && (tz.is_empty()
                || tz.len() > 64
                || !tz
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b)))
        {
            return Err(MetaError::Invalid(
                "time zone must be an IANA name like Europe/Paris",
            ));
        }
        self.with(|c| {
            Ok(c.execute(
                "UPDATE users SET timezone = ?2 WHERE id = ?1",
                params![id, timezone],
            )? == 1)
        })
    }

    /// Record a successful sign-in.
    pub fn touch_sign_in(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE users SET last_sign_in = ?2 WHERE id = ?1",
                params![id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// SEC-07: bind this user to the OIDC (issuer, subject) that just
    /// signed them in. The first sign-in records the pair; every one after
    /// must match it exactly, so a different account at the provider
    /// (even one the provider lets reuse this e-mail) cannot sign in as
    /// this user.
    pub fn bind_oidc_subject(&self, id: &str, iss: &str, sub: &str) -> Result<bool> {
        self.with(|c| {
            let bound: Option<(Option<String>, Option<String>)> = c
                .query_row(
                    "SELECT oidc_iss, oidc_sub FROM users WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match bound {
                None => Ok(false),
                Some((None, None)) => {
                    c.execute(
                        "UPDATE users SET oidc_iss = ?2, oidc_sub = ?3 WHERE id = ?1",
                        params![id, iss, sub],
                    )?;
                    Ok(true)
                }
                Some((i, s)) => Ok(i.as_deref() == Some(iss) && s.as_deref() == Some(sub)),
            }
        })
    }

    /// When the user's newest unused login link expires (Unix seconds), if
    /// one is pending: an invitation still open.
    pub fn pending_link(&self, id: &str) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT max(expires_at) FROM login_links WHERE user_id = ?1 AND expires_at >= ?2",
                params![id, Utc::now().timestamp()],
                |r| r.get::<_, Option<i64>>(0),
            )?)
        })
    }

    /// Drop the user's pending login links (revoke an invitation).
    pub fn revoke_links(&self, id: &str) -> Result<usize> {
        self.with(|c| Ok(c.execute("DELETE FROM login_links WHERE user_id = ?1", [id])?))
    }

    /// Remove a user (their sessions stop working at once) and their links.
    /// Refuses the last admin.
    pub fn delete_user(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            if Self::last_admin(c, id)? {
                return Err(MetaError::Invalid("the last admin cannot be removed"));
            }
            c.execute("DELETE FROM login_links WHERE user_id = ?1", [id])?;
            Ok(c.execute("DELETE FROM users WHERE id = ?1", [id])? == 1)
        })
    }
}

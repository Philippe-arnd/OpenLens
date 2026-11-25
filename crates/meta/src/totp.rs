//! A user's second factor for password sign-in (#440). The store keeps
//! only what the server hands it: the secret sealed with the server key,
//! and SHA-256 hashes of the recovery codes. Checking a code is the
//! server's job; the store makes each use count once.

use super::*;

/// A user's second factor, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Totp {
    /// The secret, sealed by the server.
    pub sealed: String,
    /// When enrolment was confirmed (Unix seconds); `None` while pending.
    pub enabled_at: Option<i64>,
    /// How many recovery codes are left unused.
    pub recovery_left: usize,
}

impl Meta {
    /// The user's second factor, pending or on; `None` without one.
    pub fn totp_of(&self, id: &str) -> Result<Option<Totp>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT totp_secret, totp_at, totp_recovery FROM users \
                 WHERE id = ?1 AND totp_secret IS NOT NULL",
                [id],
                |r| {
                    let rec: Option<String> = r.get(2)?;
                    Ok(Totp {
                        sealed: r.get(0)?,
                        enabled_at: r.get(1)?,
                        recovery_left: rec
                            .as_deref()
                            .map_or(0, |s| s.split(',').filter(|h| !h.is_empty()).count()),
                    })
                },
            )
            .optional()?)
        })
    }

    /// Is the user's second factor on (enrolment confirmed)?
    pub fn totp_enabled(&self, id: &str) -> Result<bool> {
        Ok(self.totp_of(id)?.is_some_and(|t| t.enabled_at.is_some()))
    }

    /// Start enrolment with a new sealed secret (replacing a pending one).
    /// Refused (`false`) while a second factor is on.
    pub fn totp_begin(&self, id: &str, sealed: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE users SET totp_secret = ?2, totp_step = NULL, totp_recovery = NULL \
                 WHERE id = ?1 AND totp_at IS NULL",
                params![id, sealed],
            )? == 1)
        })
    }

    /// Confirm a pending enrolment: the second factor is on from `at`, the
    /// confirming code's `step` counts as used, and `recovery` (hashes)
    /// replaces any earlier codes. Sessions issued before `at` end.
    pub fn totp_enable(&self, id: &str, step: i64, recovery: &[String], at: i64) -> Result<bool> {
        if recovery.iter().any(|h| h.is_empty() || h.contains(',')) {
            return Err(MetaError::Invalid("bad recovery code hash"));
        }
        self.with(|c| {
            Ok(c.execute(
                "UPDATE users SET totp_at = ?3, totp_step = ?2, totp_recovery = ?4, password_at = ?3 \
                 WHERE id = ?1 AND totp_secret IS NOT NULL AND totp_at IS NULL",
                params![id, step, recovery.join(","), at],
            )? == 1)
        })
    }

    /// Drop the user's second factor, pending or on. With `at`, sessions
    /// issued before it end (turning an active factor off); `None` only
    /// cancels a pending enrolment.
    pub fn totp_clear(&self, id: &str, at: Option<i64>) -> Result<bool> {
        self.with(|c| {
            let n = match at {
                Some(at) => c.execute(
                    "UPDATE users SET totp_secret = NULL, totp_at = NULL, totp_step = NULL, \
                     totp_recovery = NULL, password_at = ?2 WHERE id = ?1",
                    params![id, at],
                )?,
                None => c.execute(
                    "UPDATE users SET totp_secret = NULL, totp_step = NULL, totp_recovery = NULL \
                     WHERE id = ?1 AND totp_at IS NULL",
                    [id],
                )?,
            };
            Ok(n == 1)
        })
    }

    /// Use the 30 s `step` of a code: `true` only when it is later than the
    /// last step used, so a code works once (replay protection).
    pub fn totp_use_step(&self, id: &str, step: i64) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE users SET totp_step = ?2 WHERE id = ?1 AND totp_at IS NOT NULL \
                 AND (totp_step IS NULL OR totp_step < ?2)",
                params![id, step],
            )? == 1)
        })
    }

    /// Use a recovery code by its hash: `true` when it was unused, and it
    /// is gone afterwards.
    pub fn totp_use_recovery(&self, id: &str, hash: &str) -> Result<bool> {
        if hash.is_empty() || hash.contains(',') {
            return Ok(false);
        }
        self.with(|c| {
            let rec: Option<String> = c
                .query_row(
                    "SELECT totp_recovery FROM users WHERE id = ?1 AND totp_at IS NOT NULL",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let Some(rec) = rec else {
                return Ok(false);
            };
            let all: Vec<&str> = rec.split(',').filter(|h| !h.is_empty()).collect();
            if !all.contains(&hash) {
                return Ok(false);
            }
            let left: Vec<&str> = all.into_iter().filter(|h| *h != hash).collect();
            Ok(c.execute(
                "UPDATE users SET totp_recovery = ?2 WHERE id = ?1 AND totp_recovery = ?3",
                params![id, left.join(","), rec],
            )? == 1)
        })
    }
}

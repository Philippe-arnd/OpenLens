//! Feature flags: rules and deterministic rollout evaluation.
//!
//! A flag is on for a user when it is enabled, the user's canonical role
//! and account pass its targeting, and the user falls inside the rollout
//! percentage. The percentage bucket is a hash of `flag key + user id`, so a
//! user keeps the same answer across sessions and devices, and widening a
//! rollout only adds users (nobody flips from on to off).
//!
//! Layer: L2 (see `docs/architecture.md`).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A flag definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    /// Key used in code: `openlens.isEnabled("new-editor")`.
    pub key: String,
    /// What it is for (shown to the PM).
    #[serde(default)]
    pub description: String,
    /// Master switch.
    pub enabled: bool,
    /// Share of eligible users who get it, 0–100.
    pub rollout: u8,
    /// Only these canonical roles (empty: every role).
    #[serde(default)]
    pub roles: Vec<String>,
    /// Always on for these accounts, whatever the rollout.
    #[serde(default)]
    pub accounts: Vec<String>,
}

/// Who is asking.
#[derive(Debug, Clone, Default)]
pub struct Subject<'a> {
    /// User id.
    pub distinct_id: &'a str,
    /// Canonical role, if known.
    pub role: Option<&'a str>,
    /// Account, if known.
    pub account: Option<&'a str>,
}

/// Bucket of a user for a flag, in 0..10_000 (0.01% steps).
pub fn bucket(flag_key: &str, distinct_id: &str) -> u32 {
    let h = Sha256::digest(format!("{flag_key}:{distinct_id}").as_bytes());
    let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
    n % 10_000
}

impl Flag {
    /// Is the flag on for this subject?
    pub fn is_on(&self, s: &Subject<'_>) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(a) = s.account
            && self.accounts.iter().any(|x| x == a)
        {
            return true;
        }
        if !self.roles.is_empty() && !s.role.is_some_and(|r| self.roles.iter().any(|x| x == r)) {
            return false;
        }
        bucket(&self.key, s.distinct_id) < u32::from(self.rollout.min(100)) * 100
    }

    /// Validate a definition; the message says how to fix it.
    pub fn validate(&self) -> Result<(), String> {
        let k = &self.key;
        if k.is_empty()
            || k.len() > 64
            || !k
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return Err(format!(
                "`{k}`: flag keys are 1-64 chars of lowercase letters, digits, - or _"
            ));
        }
        if self.rollout > 100 {
            return Err("rollout is a percentage, 0-100".into());
        }
        if self.description.len() > 500 || self.roles.len() > 50 || self.accounts.len() > 1000 {
            return Err("description ≤ 500 chars, ≤ 50 roles, ≤ 1000 accounts".into());
        }
        Ok(())
    }
}

/// Keys of the flags that are on for `s`, sorted.
pub fn evaluate(flags: &[Flag], s: &Subject<'_>) -> Vec<String> {
    let mut on: Vec<String> = flags
        .iter()
        .filter(|f| f.is_on(s))
        .map(|f| f.key.clone())
        .collect();
    on.sort();
    on
}

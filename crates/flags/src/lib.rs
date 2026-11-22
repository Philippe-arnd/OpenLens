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

/// In-app micro-surveys (D-016): one short question shown right after a
/// trigger event, to a sample of users, once per user. Answers come back as
/// `$survey` events. Targeting works like flags (canonical roles, a
/// deterministic sample).
pub mod survey {
    use serde::{Deserialize, Serialize};

    use super::{Subject, bucket};

    /// A survey definition.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Survey {
        /// Key: `[a-z0-9_-]`, 1–64.
        pub key: String,
        /// The question, at most 200 characters.
        pub question: String,
        /// `rating` (1–5), `yes_no`, `choice` (one of `choices`) or `text`.
        pub kind: String,
        /// Options for `choice` (2–6, at most 40 characters each).
        #[serde(default)]
        pub choices: Vec<String>,
        /// Event that shows the survey (a success event, or `$pageview`).
        pub trigger: String,
        /// Master switch.
        pub enabled: bool,
        /// Share of eligible users asked, 0–100.
        pub sample: u8,
        /// Only these canonical roles (empty: every role).
        #[serde(default)]
        pub roles: Vec<String>,
    }

    /// What the browser receives (no targeting details).
    #[derive(Debug, Clone, Serialize)]
    pub struct Public<'a> {
        /// Key.
        pub key: &'a str,
        /// Question.
        pub question: &'a str,
        /// Kind.
        pub kind: &'a str,
        /// Choices.
        pub choices: &'a [String],
        /// Trigger event.
        pub trigger: &'a str,
    }

    fn valid_key(k: &str) -> bool {
        (1..=64).contains(&k.len())
            && k.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    }

    impl Survey {
        /// Check a definition; the error says what to fix.
        pub fn validate(&self) -> Result<(), String> {
            if !valid_key(&self.key) {
                return Err("key: 1–64 of [a-z0-9_-]".into());
            }
            if self.question.trim().is_empty() || self.question.chars().count() > 200 {
                return Err("question: 1–200 characters".into());
            }
            match self.kind.as_str() {
                "rating" | "yes_no" | "text" => {}
                "choice" => {
                    if !(2..=6).contains(&self.choices.len())
                        || self
                            .choices
                            .iter()
                            .any(|c| c.trim().is_empty() || c.chars().count() > 40)
                    {
                        return Err("choices: 2–6 options of 1–40 characters".into());
                    }
                }
                _ => return Err("kind: rating, yes_no, choice or text".into()),
            }
            if self.trigger.is_empty()
                || self.trigger.len() > 100
                || self.trigger.chars().any(|c| c.is_control() || c == '"')
            {
                return Err("trigger: an event name, e.g. entry_saved".into());
            }
            if self.sample > 100 {
                return Err("sample: 0–100".into());
            }
            Ok(())
        }

        /// Should this subject be asked?
        pub fn eligible(&self, s: &Subject<'_>) -> bool {
            self.enabled
                && (self.roles.is_empty()
                    || s.role.is_some_and(|r| self.roles.iter().any(|x| x == r)))
                && bucket(&format!("survey:{}", self.key), s.distinct_id)
                    < u32::from(self.sample) * 100
        }

        /// The browser's view of it.
        pub fn public(&self) -> Public<'_> {
            Public {
                key: &self.key,
                question: &self.question,
                kind: &self.kind,
                choices: &self.choices,
                trigger: &self.trigger,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used)]
        use super::*;

        fn s() -> Survey {
            Survey {
                key: "value".into(),
                question: "How useful was this?".into(),
                kind: "rating".into(),
                choices: vec![],
                trigger: "entry_saved".into(),
                enabled: true,
                sample: 100,
                roles: vec![],
            }
        }

        #[test]
        fn validation_and_targeting() {
            assert!(s().validate().is_ok());
            assert!(
                Survey {
                    kind: "essay".into(),
                    ..s()
                }
                .validate()
                .is_err()
            );
            assert!(
                Survey {
                    kind: "choice".into(),
                    choices: vec!["a".into()],
                    ..s()
                }
                .validate()
                .is_err()
            );
            assert!(
                Survey {
                    key: "Bad Key".into(),
                    ..s()
                }
                .validate()
                .is_err()
            );
            assert!(
                Survey {
                    trigger: "x\"y".into(),
                    ..s()
                }
                .validate()
                .is_err()
            );
            let who = Subject {
                distinct_id: "u1",
                role: Some("admin"),
                account: None,
            };
            assert!(s().eligible(&who));
            assert!(
                !Survey {
                    enabled: false,
                    ..s()
                }
                .eligible(&who)
            );
            assert!(
                !Survey {
                    roles: vec!["member".into()],
                    ..s()
                }
                .eligible(&who)
            );
            assert!(!Survey { sample: 0, ..s() }.eligible(&who));
            // Sampling is deterministic and roughly the share asked.
            let asked = (0..1000)
                .filter(|i| {
                    Survey { sample: 30, ..s() }.eligible(&Subject {
                        distinct_id: &format!("u{i}"),
                        role: None,
                        account: None,
                    })
                })
                .count();
            assert!((230..370).contains(&asked), "{asked}");
        }
    }
}

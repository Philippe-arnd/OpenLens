//! Deploys (#504, part 3 of #237, D-053): what the deploy hook receives,
//! and how a deploy is tied to a release on the Releases page.
//!
//! - [`parse`] reads the notification formats of the common platforms.
//! - [`finish`] checks and bounds what a notification says before it is
//!   recorded.
//! - [`link`] ties each recorded deploy to a release of its environment:
//!   by name; else by commit (one of the release name and the commit is a
//!   prefix of the other, at least 7 characters, as the Node SDK names a
//!   release after the commit, D-044); else, for a succeeded deploy that
//!   names neither, the first release first seen in the environment in
//!   the [`LINK_WINDOW_SECS`] after it.

use std::collections::HashMap;

use openlens_meta::deploys::Deploy;
use serde::{Deserialize, Serialize};

mod parse;
pub use parse::{Hints, Parsed, parse};

/// A succeeded deploy naming no release and no commit is tied to the first
/// release first seen in its environment this long after it.
pub const LINK_WINDOW_SECS: i64 = 2 * 3600;
/// Longest release name kept.
const MAX_RELEASE: usize = 200;
/// A deploy time further than this in the future is replaced by the time
/// it arrived.
const MAX_SKEW_SECS: i64 = 3600;

/// A deploy's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The platform started deploying.
    Started,
    /// The new version is out.
    Succeeded,
    /// The deploy failed (or was canceled): the old version still runs.
    Failed,
    /// The platform put the previous version back.
    RolledBack,
}

impl Status {
    /// As stored and sent.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }

    /// From the plain form, with the common synonyms.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "started" | "start" | "pending" | "in_progress" | "running" => Some(Self::Started),
            "succeeded" | "success" | "ok" | "done" | "deployed" => Some(Self::Succeeded),
            "failed" | "failure" | "error" | "canceled" | "cancelled" => Some(Self::Failed),
            "rolled_back" | "rollback" | "rolled-back" => Some(Self::RolledBack),
            _ => None,
        }
    }
}

/// What a notification says, before it is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The format it came in.
    pub source: &'static str,
    /// Its status.
    pub status: Status,
    /// Environment, as the platform names it.
    pub environment: Option<String>,
    /// Release name.
    pub release: Option<String>,
    /// Commit.
    pub commit: Option<String>,
    /// When (Unix seconds).
    pub at: Option<i64>,
}

/// The deploy to record from a notice: the environment normalised (the
/// hook's `env` parameter first, then the notice's, else `production`),
/// a release of at most 200 printable characters, a commit of 7 to 64 hex
/// digits (lowercased; anything else dropped), and a time no later than
/// an hour from `now` (else `now`).
///
/// # Errors
/// An environment that is not a plain name.
pub fn finish(n: Notice, env: Option<&str>, now: i64) -> Result<Deploy, &'static str> {
    let raw = env
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .or(n.environment.as_deref())
        .unwrap_or("");
    let environment = openlens_event::normalize_environment(raw);
    if environment.len() > 64
        || !environment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err("the environment must be a plain name (letters, digits, - _ .)");
    }
    let release = n
        .release
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty() && r.len() <= MAX_RELEASE && !r.chars().any(char::is_control));
    let commit = n.commit.as_deref().and_then(commit);
    let at =
        n.at.filter(|t| *t > 0 && *t <= now + MAX_SKEW_SECS)
            .unwrap_or(now);
    Ok(Deploy {
        environment,
        release,
        commit,
        status: n.status.as_str().to_owned(),
        at,
        source: n.source.to_owned(),
    })
}

/// A commit as kept: 7 to 64 hex digits, lowercased; anything else is
/// `None`.
#[must_use]
pub fn commit(raw: &str) -> Option<String> {
    let c = raw.trim().to_ascii_lowercase();
    ((7..=64).contains(&c.len()) && c.chars().all(|x| x.is_ascii_hexdigit())).then_some(c)
}

/// Whether a release name and a commit name the same build: one is a
/// prefix of the other, case aside, and the shorter has 7 characters or
/// more.
#[must_use]
pub fn same_commit(release: &str, commit: &str) -> bool {
    let (r, c) = (release.to_ascii_lowercase(), commit.to_ascii_lowercase());
    let (short, long) = if r.len() <= c.len() {
        (&r, &c)
    } else {
        (&c, &r)
    };
    short.len() >= 7 && long.starts_with(short.as_str())
}

/// A release seen in an environment, for [`link`]: its name and its
/// first event (Unix seconds).
#[derive(Debug, Clone)]
pub struct Seen {
    /// Environment.
    pub environment: String,
    /// Release name.
    pub release: String,
    /// First event.
    pub first: i64,
}

/// The release each deploy is tied to (by index in `deploys`), among
/// `seen`; `None` for a deploy tied to none. `deploys` are oldest first.
#[must_use]
pub fn link(deploys: &[Deploy], seen: &[Seen]) -> Vec<Option<String>> {
    let mut by_env: HashMap<&str, Vec<&Seen>> = HashMap::new();
    for s in seen {
        by_env.entry(s.environment.as_str()).or_default().push(s);
    }
    for v in by_env.values_mut() {
        v.sort_by(|a, b| {
            a.first
                .cmp(&b.first)
                .then_with(|| a.release.cmp(&b.release))
        });
    }
    // Releases already tied by time, per environment.
    let mut taken: Vec<(String, String)> = Vec::new();
    deploys
        .iter()
        .map(|d| {
            let rs = by_env.get(d.environment.as_str())?;
            if let Some(name) = &d.release {
                return rs
                    .iter()
                    .find(|s| &s.release == name)
                    .map(|s| s.release.clone());
            }
            if let Some(c) = &d.commit {
                return rs
                    .iter()
                    .find(|s| same_commit(&s.release, c))
                    .map(|s| s.release.clone());
            }
            if d.status != Status::Succeeded.as_str() {
                return None;
            }
            let s = rs.iter().find(|s| {
                s.first >= d.at
                    && s.first <= d.at + LINK_WINDOW_SECS
                    && !taken
                        .iter()
                        .any(|(e, r)| *e == s.environment && *r == s.release)
            })?;
            taken.push((s.environment.clone(), s.release.clone()));
            Some(s.release.clone())
        })
        .collect()
}

/// A deploy as listed by `releases.list`.
#[derive(Debug, Clone, Serialize)]
pub struct DeployRow {
    /// Environment.
    pub environment: String,
    /// The release it is tied to, if any.
    pub release: Option<String>,
    /// The release name the platform gave, if any.
    pub reported_release: Option<String>,
    /// The commit the platform gave, if any.
    pub commit: Option<String>,
    /// Its status.
    pub status: Status,
    /// When (`YYYY-MM-DD HH:MM`, in the list's time zone).
    pub at: String,
    /// The format it came in.
    pub source: String,
}

/// The name a deploy tied to no release is listed under: its release, its
/// commit (12 characters), or `deploy <time>`.
#[must_use]
pub fn label(d: &Deploy, at: &str) -> String {
    d.release
        .clone()
        .or_else(|| d.commit.as_ref().map(|c| c.chars().take(12).collect()))
        .unwrap_or_else(|| format!("deploy {at}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(
        env: &str,
        release: Option<&str>,
        commit: Option<&str>,
        status: Status,
        at: i64,
    ) -> Deploy {
        Deploy {
            environment: env.into(),
            release: release.map(str::to_owned),
            commit: commit.map(str::to_owned),
            status: status.as_str().into(),
            at,
            source: "plain".into(),
        }
    }

    fn seen(env: &str, release: &str, first: i64) -> Seen {
        Seen {
            environment: env.into(),
            release: release.into(),
            first,
        }
    }

    #[test]
    fn deploys_are_tied_by_name_then_commit_then_time() {
        let seen = [
            seen("production", "v1.2.0", 1_000),
            seen("production", "0123456789ab", 5_000),
            seen("production", "2026-11-26.1", 9_000),
            seen("staging", "v1.2.0", 500),
        ];
        let deploys = [
            d("production", Some("v1.2.0"), None, Status::Succeeded, 900),
            d(
                "production",
                None,
                Some("0123456789abcdef0123"),
                Status::Succeeded,
                4_000,
            ),
            d("production", None, None, Status::Succeeded, 8_000),
            // The same release is not tied twice by time.
            d("production", None, None, Status::Succeeded, 8_500),
            // Only succeeded deploys are tied by time.
            d("staging", None, None, Status::Failed, 400),
            // A name seen in another environment only.
            d("production", Some("v9"), None, Status::Failed, 100),
            // Too short to be a commit of that release.
            d("production", None, Some("012345"), Status::Succeeded, 100),
        ];
        let l = link(&deploys, &seen);
        assert_eq!(l[0].as_deref(), Some("v1.2.0"));
        assert_eq!(l[1].as_deref(), Some("0123456789ab"));
        assert_eq!(l[2].as_deref(), Some("2026-11-26.1"));
        assert_eq!(l[3], None);
        assert_eq!(l[4], None);
        assert_eq!(l[5], None);
        assert_eq!(l[6], None);
    }

    #[test]
    fn a_notice_is_bounded_before_it_is_recorded() {
        let n = Notice {
            source: "plain",
            status: Status::Failed,
            environment: Some("prod".into()),
            release: Some("x".repeat(500)),
            commit: Some("not-a-hash".into()),
            at: Some(10_000),
        };
        let out = finish(n.clone(), None, 1_000).unwrap();
        assert_eq!(out.environment, "production");
        assert_eq!(out.release, None);
        assert_eq!(out.commit, None);
        assert_eq!(out.at, 1_000, "a time far in the future is replaced");
        let staged = finish(n.clone(), Some("staging"), 1_000).unwrap();
        assert_eq!(staged.environment, "staging", "the hook's env wins");
        assert!(finish(n, Some("a b/c"), 1_000).is_err());
    }
}

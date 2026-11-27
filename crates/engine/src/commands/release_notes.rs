//! A release's notes from the git host (#574, D-092): the merged pull
//! requests of its commit range grouped plainly, the range's counts and
//! the matching tag, or why they are not there. The reads themselves are
//! the hourly pass of [`crate::release_git`]; this command only reads
//! what it kept. `releases.list` and `releases.report` carry the same
//! notes ([`crate::release_git::listed`], [`crate::release_git::notes`]).

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::release_git::{self, ReleaseNotes};
use crate::releases;

/// The input of `releases.notes`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotesInput {
    /// Project id.
    pub project: String,
    /// Release name, as recorded.
    pub release: String,
    /// Environment (production when absent).
    #[serde(default)]
    pub environment: Option<String>,
}

/// `releases.notes`: one release's notes, or why there are none (no
/// GitHub link, no commit range, not read yet, or the reads failed).
pub struct Notes;

impl Command for Notes {
    const NAME: &'static str = "releases.notes";
    const ACCESS: Access = Access::Read;
    type Input = NotesInput;
    type Output = ReleaseNotes;

    async fn run<H: Host>(host: &H, _: &Caller, input: NotesInput) -> Result<ReleaseNotes, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let env = input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .unwrap_or(releases::PRODUCTION);
        let release = input.release.trim();
        if release.is_empty() || release.len() > 200 {
            return Err(Error::Invalid(
                "Name a release (200 characters at most).".into(),
            ));
        }
        Ok(release_git::notes(host, &p.id, env, release))
    }
}

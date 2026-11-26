//! The deploy hook as commands (#504, D-053): make or rotate a project's
//! hook secret, and record a deploy the hook received. The formats and
//! the rules are in [`crate::deploys`]. A release recorded by its deploy
//! step, with its commit range (#521, D-066), is a deploy too.

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::deploys::Status;

/// A project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectInput {
    /// Project id.
    pub project: String,
}

/// A new hook secret: shown once, only its hash is kept.
#[derive(Debug, Clone, Serialize)]
pub struct Secret {
    /// The secret, the last segment of `/hooks/deploy/<secret>`.
    pub secret: String,
}

/// `deploys.rotate_hook`: a new deploy hook secret for the project; the
/// old one stops working at once.
pub struct RotateHook;

impl Command for RotateHook {
    const NAME: &'static str = "deploys.rotate_hook";
    const ACCESS: Access = Access::Write;
    type Input = ProjectInput;
    type Output = Secret;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProjectInput) -> Result<Secret, Error> {
        match host.meta().rotate_deploy_hook(&input.project) {
            Ok(Some(secret)) => Ok(Secret { secret }),
            Ok(None) => Err(Error::NotFound),
            Err(e) => Err(Error::Storage(e.to_string())),
        }
    }
}

/// The input of `deploys.record`: a deploy, already checked by
/// [`crate::deploys::finish`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordInput {
    /// Project id.
    pub project: String,
    /// Environment.
    pub environment: String,
    /// Release name.
    #[serde(default)]
    pub release: Option<String>,
    /// Commit.
    #[serde(default)]
    pub commit: Option<String>,
    /// Status.
    pub status: Status,
    /// When (Unix seconds).
    pub at: i64,
    /// The format it came in.
    pub source: String,
}

/// `deploys.record`: store a deploy (the project keeps its latest 500).
pub struct Record;

impl Command for Record {
    const NAME: &'static str = "deploys.record";
    const ACCESS: Access = Access::Write;
    type Input = RecordInput;
    type Output = ();

    async fn run<H: Host>(host: &H, _: &Caller, i: RecordInput) -> Result<(), Error> {
        let d = openlens_meta::deploys::Deploy {
            environment: i.environment,
            release: i.release,
            commit: i.commit,
            status: i.status.as_str().to_owned(),
            at: i.at,
            source: i.source,
        };
        host.meta()
            .record_deploy(&i.project, &d, chrono::Utc::now().timestamp())
            .map_err(|e| Error::Storage(e.to_string()))
    }
}

/// The input of `releases.record`: a release named by its deploy step
/// (`openlens-cli release`, the `openlens-release` action, MCP
/// `record_release`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordReleaseInput {
    /// Project id (the REST route takes it from the path).
    #[serde(default)]
    pub project: String,
    /// Release name, as the SDK sends it.
    pub release: String,
    /// Environment; `production` when absent.
    #[serde(default)]
    pub environment: Option<String>,
    /// The commit deployed (7 to 64 hex digits).
    #[serde(default)]
    pub commit: Option<String>,
    /// The commit the range starts after; the environment's previous
    /// deployed commit when absent.
    #[serde(default)]
    pub from: Option<String>,
}

/// A release as recorded.
#[derive(Debug, Clone, Serialize)]
pub struct RecordedRelease {
    /// Release name.
    pub release: String,
    /// Environment.
    pub environment: String,
    /// The commit deployed, if given.
    pub commit: Option<String>,
    /// The commit its range starts after, if known: `from..commit`.
    pub from: Option<String>,
    /// When it was recorded (Unix seconds).
    pub at: i64,
}

/// `releases.record`: record a release with its commit range, as a
/// succeeded deploy of the environment (D-066). It then shows on the
/// Releases page like a deploy the hook reported.
pub struct RecordRelease;

impl Command for RecordRelease {
    const NAME: &'static str = "releases.record";
    const ACCESS: Access = Access::Write;
    type Input = RecordReleaseInput;
    type Output = RecordedRelease;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        i: RecordReleaseInput,
    ) -> Result<RecordedRelease, Error> {
        let p = host.project(&i.project).await.ok_or(Error::NotFound)?;
        let now = chrono::Utc::now().timestamp();
        let commit = checked_commit(i.commit.as_deref(), "commit")?;
        let given_from = checked_commit(i.from.as_deref(), "from")?;
        let notice = crate::deploys::Notice {
            source: "cli",
            status: Status::Succeeded,
            environment: i.environment,
            release: Some(i.release),
            commit: commit.clone(),
            at: None,
        };
        let d =
            crate::deploys::finish(notice, None, now).map_err(|e| Error::Invalid(e.to_owned()))?;
        let release = d.release.clone().ok_or_else(|| {
            Error::Invalid("the release must be 1 to 200 printable characters".into())
        })?;
        let from = match (given_from, &commit) {
            (Some(f), _) => Some(f),
            (None, Some(c)) => host
                .meta()
                .previous_commit(&p.id, &d.environment, Some(c))
                .map_err(|e| Error::Storage(e.to_string()))?,
            (None, None) => None,
        };
        if from.is_some() && from == commit {
            return Err(Error::Invalid("from and commit are the same commit".into()));
        }
        host.meta()
            .record_release(&p.id, &d, from.as_deref(), now)
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(RecordedRelease {
            release,
            environment: d.environment,
            commit,
            from,
            at: d.at,
        })
    }
}

/// A commit given by the caller: absent or blank is `None`; anything but
/// 7 to 64 hex digits is refused, not dropped.
fn checked_commit(raw: Option<&str>, what: &str) -> Result<Option<String>, Error> {
    match raw.map(str::trim).filter(|c| !c.is_empty()) {
        None => Ok(None),
        Some(c) => crate::deploys::commit(c)
            .map(Some)
            .ok_or_else(|| Error::Invalid(format!("{what} must be a commit: 7 to 64 hex digits"))),
    }
}

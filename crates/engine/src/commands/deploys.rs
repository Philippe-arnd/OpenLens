//! The deploy hook as commands (#504, D-053): make or rotate a project's
//! hook secret, and record a deploy the hook received. The formats and
//! the rules are in [`crate::deploys`].

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

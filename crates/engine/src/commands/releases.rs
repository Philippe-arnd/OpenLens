//! Releases as commands (#500, D-049): each release per environment with
//! its state (rolling out, live, superseded, rolled back), first and last
//! seen, users and time in staging. The rules are in
//! [`crate::releases`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::releases::{self, Releases};

/// The input of `releases.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
    /// One environment only; every environment when absent.
    #[serde(default)]
    pub environment: Option<String>,
}

/// `releases.list`: the project's releases over the last 90 days.
pub struct List;

impl Command for List {
    const NAME: &'static str = "releases.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Releases;

    async fn run<H: Host>(host: &H, caller: &Caller, input: ListInput) -> Result<Releases, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let tz = caller.tz.clone().unwrap_or_else(|| p.timezone.clone());
        let builds = host
            .meta()
            .release_builds(&p.id)
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        let env = input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty());
        releases::releases(host.query(), &p.id, &tz, env, &builds)
            .await
            .map_err(|e| match e {
                crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
                _ => Error::Unavailable(e.to_string()),
            })
    }
}

//! Runtime health per release as a command (#610, D-087): restarts per
//! day, the largest RSS and its growth, event-loop delay, from the Node
//! SDK's `$runtime_stats`. The rules are in [`crate::runtime`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::runtime::{self, RuntimeShown, RuntimeStat};
use crate::verdict::Check;

/// The input of `runtime.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
    /// Environment (production when absent).
    #[serde(default)]
    pub environment: Option<String>,
}

/// One release's runtime.
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseRuntime {
    /// Release name.
    pub release: String,
    /// Its first runtime event, UTC.
    pub first_seen: String,
    /// The release before it with runtime stats, if any.
    pub previous_release: Option<String>,
    /// Its numbers.
    pub stats: RuntimeStat,
    /// What is shown next to the release before.
    pub shown: RuntimeShown,
    /// Restarts per day against the release before (`worse` is what makes
    /// a verdict regressed); none for the first release.
    pub restarts: Option<Check>,
}

/// The runtime of one environment's releases.
#[derive(Debug, Clone, Serialize)]
pub struct Runtime {
    /// Environment read.
    pub environment: String,
    /// Newest first, over the releases' 90 days.
    pub releases: Vec<ReleaseRuntime>,
    /// Why the list is empty, when it is.
    pub note: Option<String>,
}

/// `runtime.list`: each release's runtime health.
pub struct List;

impl Command for List {
    const NAME: &'static str = "runtime.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Runtime;

    async fn run<H: Host>(host: &H, _: &Caller, input: ListInput) -> Result<Runtime, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let environment = input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .unwrap_or(crate::releases::PRODUCTION)
            .to_owned();
        let since = (chrono::Utc::now() - chrono::Duration::days(crate::releases::WINDOW_DAYS + 1))
            .format("%Y-%m-%d")
            .to_string();
        let list = runtime::by_release(host.query(), &p.id, &environment, &since)
            .await
            .map_err(|e| match e {
                crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
                _ => Error::Unavailable(e.to_string()),
            })?;
        let note = list.is_empty().then(|| {
            "No runtime stats yet: the Node SDK sends them with its Express middleware or \
             `captureRuntime()` (sdk/SERVER.md)."
                .to_owned()
        });
        let mut releases: Vec<ReleaseRuntime> = Vec::with_capacity(list.len());
        let mut previous: Option<&runtime::ReleaseRuntime> = None;
        for r in &list {
            let before = previous.map(|x| &x.stat);
            releases.push(ReleaseRuntime {
                release: r.release.clone(),
                first_seen: r.first_seen.clone(),
                previous_release: previous.map(|x| x.release.clone()),
                stats: r.stat.clone(),
                shown: runtime::shown(&r.stat, before),
                restarts: before.map(|b| runtime::check(&r.stat, b)),
            });
            previous = Some(r);
        }
        releases.reverse();
        Ok(Runtime {
            environment,
            releases,
            note,
        })
    }
}

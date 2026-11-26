//! Engine commands (#308, D-042): every action the dashboard, the REST API,
//! Ask and MCP offer is a command here, with a name, a serialisable input
//! and output, and the access level it needs. Front ends are adapters: they
//! check the level (the server's typed extractors, #294), build the input,
//! call [`Command::run`] and render the output.
//!
//! [`REGISTRY`] lists every command, so a test can compare it with the MCP
//! tools.

use std::future::Future;

use openlens_featuremap::FeatureMap;
use openlens_meta::{Meta, Project};
use openlens_query::QueryEngine;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Board, Filters};

pub mod alerts;
pub mod ask;
pub mod deploys;
pub mod flags;
pub mod explore;
pub mod goals;
pub mod releases;
pub mod tickets;

/// Who may run a command, in the terms of the server's access policy
/// (`crates/server/src/access.rs`, #294).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Anyone signed in who may see the project (any role).
    Read,
    /// An admin who may see the project, or the bearer token
    /// (`ProjectWrite`).
    Write,
    /// An admin of every project, or the bearer token (`Admin`).
    Admin,
}

/// A command: its name, input, output and access level.
pub trait Command {
    /// Stable name, `area.verb` (`goals.set`).
    const NAME: &'static str;
    /// The level a caller needs.
    const ACCESS: Access;
    /// What the caller sends.
    type Input: Serialize + DeserializeOwned + Send;
    /// What the command returns.
    type Output: Serialize;

    /// Run the command for `caller`.
    fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: Self::Input,
    ) -> impl Future<Output = Result<Self::Output, Error>> + Send;
}

/// A registry entry: what a front end needs to list a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Spec {
    /// [`Command::NAME`].
    pub name: &'static str,
    /// [`Command::ACCESS`].
    pub access: Access,
}

/// The entry of `C`.
#[must_use]
pub const fn spec<C: Command>() -> Spec {
    Spec {
        name: C::NAME,
        access: C::ACCESS,
    }
}

/// Every command.
pub const REGISTRY: &[Spec] = &[
    spec::<goals::List>(),
    spec::<goals::Set>(),
    spec::<goals::Delete>(),
    spec::<goals::SetObjective>(),
    spec::<goals::DeleteObjective>(),
    spec::<alerts::GetStatus>(),
    spec::<alerts::GetUptime>(),
    spec::<alerts::GetSettings>(),
    spec::<alerts::SetSettings>(),
    spec::<alerts::Test>(),
    spec::<releases::List>(),
    spec::<tickets::List>(),
    spec::<tickets::DraftTicket>(),
    spec::<tickets::File>(),
    spec::<tickets::SetTarget>(),
    spec::<deploys::RotateHook>(),
    spec::<deploys::Record>(),
    // Ask's lookups and its confirmed flag change (#508).
    spec::<ask::Board>(),
    spec::<ask::Feature>(),
    spec::<ask::Errors>(),
    spec::<ask::Sql>(),
    spec::<ask::Vitals>(),
    spec::<ask::Goals>(),
    spec::<flags::List>(),
    spec::<flags::Update>(),
    // Explore (#507).
    spec::<explore::GetPerson>(),
    spec::<explore::Breakdown>(),
    spec::<explore::Sql>(),
    spec::<explore::GetAcquisition>(),
    spec::<explore::Forms>(),
    spec::<explore::Churn>(),
    spec::<explore::FlagAudiences>(),
    spec::<explore::MakeAudience>(),
    spec::<explore::Vitals>(),
    spec::<explore::GetAudience>(),
    spec::<explore::Flows>(),
    spec::<explore::GetLive>(),
];

/// A 12-character id from `s` (the first bytes of its SHA-256, in hex).
pub(crate) fn short_hash(s: &str) -> String {
    use sha2::Digest;
    let h = hex::encode(sha2::Sha256::digest(s.as_bytes()));
    h.get(..12).unwrap_or(&h).to_owned()
}

/// Who runs a command, beyond the access level the adapter checked.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The viewer's own time zone, when valid; `None` counts in the
    /// project's (D-017).
    pub tz: Option<String>,
}

/// Why a command did not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// No such project.
    #[error("unknown project")]
    NotFound,
    /// The input is refused; the text says why, for the person.
    #[error("{0}")]
    Invalid(String),
    /// The numbers it needs could not be computed.
    #[error("{0}")]
    Unavailable(String),
    /// Storing the result failed.
    #[error("{0}")]
    Storage(String),
}

/// What a command needs from the process it runs in: the stores, and the
/// cached board and timeline the server keeps.
pub trait Host: Sync {
    /// The metadata store.
    fn meta(&self) -> &Meta;
    /// The event query engine.
    fn query(&self) -> &QueryEngine;
    /// A project by id.
    fn project(&self, id: &str) -> impl Future<Output = Option<Project>> + Send;
    /// The project's latest valid feature map, with its version.
    fn feature_map(&self, project: &str) -> impl Future<Output = Option<(FeatureMap, i64)>> + Send;
    /// The board for these filters (cached by the host).
    fn board(
        &self,
        project: &str,
        f: &Filters,
        map: Option<(&FeatureMap, i64)>,
    ) -> impl Future<Output = crate::Result<Board>> + Send;
    /// What happened in the project over the last `days` (releases first
    /// seen, changes, alerts, notes).
    fn changes(&self, project: &str, releases: &[(String, String)], days: i64) -> Vec<Change>;
    /// The alert level of one of the project's environments (`notify` or
    /// `log`, #410), from the rules the host keeps.
    fn alert_level(&self, project: &str, env: &str) -> String;
    /// Send one alert message on every channel the project set (email,
    /// webhook, ntfy), now: what was tried and how it went.
    fn send_alert(
        &self,
        project: &str,
        title: &str,
        body: &str,
        rule: &str,
        resolved: bool,
    ) -> impl Future<Output = Vec<alerts::Delivery>> + Send;
    /// Where the project files its GitHub issues, when a repository and a
    /// token are set.
    fn github(&self, project: &str) -> Option<tickets::GitHub>;
    /// One call to GitHub's REST API (`GET`, `POST` or `PATCH` on `url`),
    /// with `token`: the answer, or why it failed (a status of 300 or more
    /// is a failure).
    fn github_call(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> impl Future<Output = Result<serde_json::Value, String>> + Send;
    /// Store the GitHub token every project shares, sealed: why it could
    /// not be sealed.
    fn set_github_token(&self, token: &str) -> Result<(), String>;
    /// `bytes` random bytes from the system's secure source, in hex (a new
    /// secret, such as Ask's per-project pseudonym key).
    fn random_hex(&self, bytes: usize) -> String;
    /// The data directory (where session recordings are kept).
    fn data_dir(&self) -> &std::path::Path;
}

/// Something that happened in a project, on the timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    /// Day (YYYY-MM-DD).
    pub day: String,
    /// `release`, `change`, `alert` or `note`.
    pub kind: String,
    /// What happened.
    pub text: String,
}

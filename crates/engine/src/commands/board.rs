//! The board's cost estimate as a command (#514, D-061): what a date
//! range, a past release or the current release's window would read
//! before it runs. The rules are in [`crate::board_cost`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::{Estimate, Filters, Window};

/// The input of `board.estimate`: the board's filters and window, as the
/// board page and `GET /api/projects/{id}/board` take them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EstimateInput {
    /// Project id.
    pub project: String,
    /// Environment (default `production`).
    #[serde(default)]
    pub environment: Option<String>,
    /// Raw role value, or all roles.
    #[serde(default)]
    pub role: Option<String>,
    /// Release filter, or all releases.
    #[serde(default)]
    pub release: Option<String>,
    /// Include the team's own users.
    #[serde(default)]
    pub internal: bool,
    /// A past release the window runs since.
    #[serde(default)]
    pub since: Option<String>,
    /// First day of a date range (`YYYY-MM-DD`, the project's days).
    #[serde(default)]
    pub from: Option<String>,
    /// Last day of the range; today when absent.
    #[serde(default)]
    pub to: Option<String>,
}

/// `board.estimate`: days read from the rollups and from the events,
/// bytes, rows and a rough time for the window asked (the current
/// release's when none is), and whether it needs confirming. None: a
/// board filtered on one release keeps the fixed windows, which have no
/// estimate.
pub struct GetEstimate;

impl Command for GetEstimate {
    const NAME: &'static str = "board.estimate";
    const ACCESS: Access = Access::Read;
    type Input = EstimateInput;
    type Output = Option<Estimate>;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: EstimateInput,
    ) -> Result<Option<Estimate>, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let some = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let tz = caller.tz.clone().unwrap_or_else(|| p.timezone.clone());
        let invalid = |e: crate::EngineError| Error::Invalid(e.to_string());
        let window = Window::asked(
            input.since.as_deref(),
            input.from.as_deref(),
            input.to.as_deref(),
            &tz,
            chrono::Utc::now(),
        )
        .map_err(invalid)?
        .unwrap_or(Window::Release);
        let f = Filters {
            environment: some(input.environment).unwrap_or_else(|| "production".into()),
            role: some(input.role),
            release: some(input.release),
            internal: input.internal,
            tz,
            window,
        };
        let version = host.feature_map(&p.id).await.map(|(_, v)| v);
        crate::estimate(host.query(), &p.id, &f, version)
            .await
            .map_err(|e| match e {
                crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
                crate::EngineError::Query(_) => Error::Unavailable(e.to_string()),
            })
    }
}

//! Delivery metrics as a command (#523, D-068): deploy frequency, lead
//! time, change failure rate and time to restore per environment. The
//! rules are in [`crate::delivery`]. A change fails when its release is
//! judged *regressed* by `releases.list` (#611).

use serde::{Deserialize, Serialize};

use super::releases::{List, ListInput};
use super::{Access, Caller, Command, Error, Host};
use crate::delivery::{self, DEFAULT_DAYS, Delivery, Known};
use crate::releases::{Releases, WINDOW_DAYS};

/// The input of `releases.delivery`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryInput {
    /// Project id.
    pub project: String,
    /// One environment only; every environment when absent.
    #[serde(default)]
    pub environment: Option<String>,
    /// Days of the period, 1 to 90 (90 when absent).
    #[serde(default)]
    pub days: Option<i64>,
}

/// `releases.delivery`: the project's delivery metrics over a period.
pub struct GetDelivery;

impl Command for GetDelivery {
    const NAME: &'static str = "releases.delivery";
    const ACCESS: Access = Access::Read;
    type Input = DeliveryInput;
    type Output = Delivery;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: DeliveryInput,
    ) -> Result<Delivery, Error> {
        check_days(&input)?;
        // The verdicts are those `releases.list` gives for the same
        // environment (production when none is asked for).
        let listed = List::run(
            host,
            caller,
            ListInput {
                project: input.project.clone(),
                environment: input.environment.clone(),
            },
        )
        .await?;
        with_releases(host, caller, input, &listed).await
    }
}

fn check_days(input: &DeliveryInput) -> Result<i64, Error> {
    let days = input.days.unwrap_or(DEFAULT_DAYS);
    if (1..=WINDOW_DAYS).contains(&days) {
        Ok(days)
    } else {
        Err(Error::Invalid(format!(
            "days must be between 1 and {WINDOW_DAYS}"
        )))
    }
}

/// `releases.delivery` from `listed`, what `releases.list` returned for the
/// same project and environment: its *regressed* verdicts fail their
/// changes (#611). The Releases page lists first and passes its list here,
/// so the verdicts are read once.
///
/// # Errors
/// As `releases.delivery`.
pub async fn with_releases<H: Host>(
    host: &H,
    caller: &Caller,
    input: DeliveryInput,
    listed: &Releases,
) -> Result<Delivery, Error> {
    let days = check_days(&input)?;
    let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
    let tz = caller.tz.clone().unwrap_or_else(|| p.timezone.clone());
    let env = input
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    let since = chrono::Utc::now().timestamp() - (WINDOW_DAYS + 1) * 86_400;
    let deploys = host
        .meta()
        .deploys(&p.id, since)
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    // The first commit of each release (#237 part 4) is not read yet: lead
    // time is reported unknown, with why.
    let known = Known {
        regressed: delivery::regressed(&listed.releases),
        ..Known::default()
    };
    delivery::delivery(host.query(), &p.id, &tz, env, days, &deploys, &known)
        .await
        .map_err(|e| match e {
            crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
            _ => Error::Unavailable(e.to_string()),
        })
}

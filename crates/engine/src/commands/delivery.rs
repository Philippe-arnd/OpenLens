//! Delivery metrics as a command (#523, D-068): deploy frequency, lead
//! time, change failure rate and time to restore per environment. The
//! rules are in [`crate::delivery`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::delivery::{self, DEFAULT_DAYS, Delivery, Known};
use crate::releases::WINDOW_DAYS;

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
        let days = input.days.unwrap_or(DEFAULT_DAYS);
        if !(1..=WINDOW_DAYS).contains(&days) {
            return Err(Error::Invalid(format!(
                "days must be between 1 and {WINDOW_DAYS}"
            )));
        }
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
        // The first commit of each release (#237 part 4) and the releases
        // judged regressed (#237 part 5) are not recorded yet: lead time is
        // reported unknown, with why, and failures come from deploys and
        // rollbacks only until they are.
        let known = Known::default();
        delivery::delivery(host.query(), &p.id, &tz, env, days, &deploys, &known)
            .await
            .map_err(|e| match e {
                crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
                _ => Error::Unavailable(e.to_string()),
            })
    }
}

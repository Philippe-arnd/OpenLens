//! `feedback.list` (#718, D-116): what users said, per rating and
//! feature, over a period. The texts (with who sent them) go only to a
//! caller allowed to read personal text ([`Caller::detail`]: an admin or
//! the automation token); viewers get the counts. The rules are in
//! [`crate::feedback`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::feedback::{self, FeedbackReport, LATEST, MAX_DAYS, MAX_LATEST};

/// The input of `feedback.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
    /// One feature key of the map; none: every feedback.
    #[serde(default)]
    pub feature: Option<String>,
    /// Days of the period, 1 to 90 (30 when absent).
    #[serde(default)]
    pub days: Option<i64>,
    /// Environment (production when absent).
    #[serde(default)]
    pub environment: Option<String>,
    /// Texts listed, newest first, 1 to 200 (20 when absent).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `feedback.list`: the project's feedback.
pub struct List;

impl Command for List {
    const NAME: &'static str = "feedback.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = FeedbackReport;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: ListInput,
    ) -> Result<FeedbackReport, Error> {
        let days = input.days.unwrap_or(30);
        if !(1..=MAX_DAYS).contains(&days) {
            return Err(Error::Invalid(format!(
                "days must be between 1 and {MAX_DAYS}"
            )));
        }
        let limit = input.limit.unwrap_or(LATEST);
        if !(1..=MAX_LATEST).contains(&limit) {
            return Err(Error::Invalid(format!(
                "limit must be between 1 and {MAX_LATEST}"
            )));
        }
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        // An old key counts as the feature it was renamed to.
        let feature = match input.feature.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(k) => Some(
                host.feature_map(&p.id)
                    .await
                    .and_then(|(m, _)| m.current_key(k).map(str::to_owned))
                    .unwrap_or_else(|| k.to_owned()),
            ),
        };
        let f = crate::Filters {
            environment: input
                .environment
                .as_deref()
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .unwrap_or(crate::releases::PRODUCTION)
                .to_owned(),
            tz: p.timezone.clone(),
            ..crate::Filters::default()
        };
        feedback::feedback(
            host.query(),
            &p.id,
            &f,
            feature.as_deref(),
            days,
            caller.detail,
            limit,
        )
        .await
        .map_err(|e| match e {
            crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
            _ => Error::Unavailable(e.to_string()),
        })
    }
}

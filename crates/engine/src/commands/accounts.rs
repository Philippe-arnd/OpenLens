//! `accounts.list` (#719, D-117): account health, at risk first, when the
//! app sends `account`. The rules are in [`crate::accounts`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::accounts::{self, AccountState, Accounts};

/// Accounts listed by default.
pub const LIMIT: usize = 50;
/// Most accounts one call lists.
pub const MAX_LIMIT: usize = 500;

/// The input of `accounts.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
    /// Only the accounts in this state: `healthy`, `at_risk` (or
    /// `at risk`), `inactive`.
    #[serde(default)]
    pub state: Option<String>,
    /// Accounts listed, 1 to 500 (50 when absent).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Environment (production when absent).
    #[serde(default)]
    pub environment: Option<String>,
}

/// `accounts.list`: the project's accounts and their health.
pub struct List;

impl Command for List {
    const NAME: &'static str = "accounts.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Accounts;

    async fn run<H: Host>(host: &H, _: &Caller, input: ListInput) -> Result<Accounts, Error> {
        let only = match input.state.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) => Some(AccountState::parse(s).ok_or_else(|| {
                Error::Invalid("state must be healthy, at_risk or inactive".into())
            })?),
        };
        let limit = input.limit.unwrap_or(LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(Error::Invalid(format!(
                "limit must be between 1 and {MAX_LIMIT}"
            )));
        }
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
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
        let known = host
            .meta()
            .account_plans(&p.id)
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        let (activity, truncated) = accounts::activity(host.query(), &p.id, &f, chrono::Utc::now())
            .await
            .map_err(|e| match e {
                crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
                _ => Error::Unavailable(e.to_string()),
            })?;
        let (mut list, small) = accounts::assemble(&known, &activity);
        let count = |s: AccountState| list.iter().filter(|a| a.state == s).count();
        let (at_risk, inactive, healthy) = (
            count(AccountState::AtRisk),
            count(AccountState::Inactive),
            count(AccountState::Healthy),
        );
        if let Some(s) = only {
            list.retain(|a| a.state == s);
        }
        list.truncate(limit);
        let note = (list.is_empty() && small.accounts == 0).then(|| {
            if known.is_empty() && activity.is_empty() {
                "No account yet: the app sends none (identify(id, { account }))."
            } else {
                "No account in this state."
            }
            .to_owned()
        });
        Ok(Accounts {
            environment: f.environment,
            days: accounts::DAYS,
            accounts: list,
            at_risk,
            inactive,
            healthy,
            small,
            truncated,
            note,
        })
    }
}

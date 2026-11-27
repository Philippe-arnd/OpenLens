//! Server routes as a command (#594, D-083): requests, 5xx share and
//! durations per route over a period, from the server SDKs' route stats.
//! The rules are in [`crate::routes`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::routes::{self, MAX_DAYS, RouteStat};

/// The input of `routes.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
    /// Environment (production when absent).
    #[serde(default)]
    pub environment: Option<String>,
    /// Days of the period, 1 to 90 (7 when absent).
    #[serde(default)]
    pub days: Option<i64>,
}

/// One route, as listed.
#[derive(Debug, Clone, Serialize)]
pub struct Route {
    /// The route's numbers.
    #[serde(flatten)]
    pub stat: RouteStat,
    /// Share of requests answered 5xx.
    pub error_rate: f64,
    /// Median duration, ms (upper bound of its histogram bucket).
    pub p50_ms: f64,
    /// 95th percentile, ms (upper bound of its histogram bucket).
    pub p95_ms: f64,
}

/// The routes of one environment.
#[derive(Debug, Clone, Serialize)]
pub struct Routes {
    /// Environment read.
    pub environment: String,
    /// Days read.
    pub days: i64,
    /// Most requests first.
    pub routes: Vec<Route>,
    /// Why the list is empty, when it is.
    pub note: Option<String>,
}

/// `routes.list`: the project's server routes.
pub struct List;

impl Command for List {
    const NAME: &'static str = "routes.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Routes;

    async fn run<H: Host>(host: &H, _: &Caller, input: ListInput) -> Result<Routes, Error> {
        let days = input.days.unwrap_or(7);
        if !(1..=MAX_DAYS).contains(&days) {
            return Err(Error::Invalid(format!(
                "days must be between 1 and {MAX_DAYS}"
            )));
        }
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let environment = input
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .unwrap_or(crate::releases::PRODUCTION)
            .to_owned();
        let map = host.feature_map(&p.id).await;
        let list = routes::list(
            host.query(),
            &p.id,
            &environment,
            days,
            map.as_ref().map(|(m, _)| m),
        )
        .await
        .map_err(|e| match e {
            crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
            _ => Error::Unavailable(e.to_string()),
        })?;
        let note = list.is_empty().then(|| {
            "No server route stats yet: add the server SDK's middleware (sdk/SERVER.md).".to_owned()
        });
        Ok(Routes {
            environment,
            days,
            routes: list
                .into_iter()
                .map(|stat| Route {
                    error_rate: stat.error_rate(),
                    p50_ms: stat.p50(),
                    p95_ms: stat.p95(),
                    stat,
                })
                .collect(),
            note,
        })
    }
}

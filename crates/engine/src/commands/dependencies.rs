//! Outgoing dependencies as a command (#608, D-090): calls, failure share
//! and p95 per external host over a period, from the `$client_stats` of
//! the Node SDK's `captureOutgoing()` or an OTLP exporter, each host with
//! how it moved since the release before. The rules are in
//! [`crate::dependencies`].

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::dependencies::{self, DependencyChange, DependencyStat, MAX_DAYS};

/// The input of `dependencies.list`.
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

/// One external host, as listed.
#[derive(Debug, Clone, Serialize)]
pub struct Dependency {
    /// The host's numbers.
    #[serde(flatten)]
    pub stat: DependencyStat,
    /// Share of calls that failed (5xx or no response).
    pub error_rate: f64,
    /// 95th percentile, ms (upper bound of its histogram bucket).
    pub p95_ms: f64,
    /// How it moved from `previous_release` to `release`, when both had
    /// enough calls to compare. Shown, never judged.
    pub change: Option<DependencyChange>,
}

/// The outgoing dependencies of one environment.
#[derive(Debug, Clone, Serialize)]
pub struct Dependencies {
    /// Environment read.
    pub environment: String,
    /// Days read.
    pub days: i64,
    /// The hosts that fail more first, then those that got slower, each
    /// by the largest change, then the most called.
    pub hosts: Vec<Dependency>,
    /// Calls over the period, every host.
    pub calls: i64,
    /// Of those, failed.
    pub errors: i64,
    /// The latest release with client stats.
    pub release: Option<String>,
    /// The release before it.
    pub previous_release: Option<String>,
    /// Hosts that fail more or got slower than on the release before.
    pub worse: usize,
    /// Why the list is empty, when it is.
    pub note: Option<String>,
}

/// How a host sorts: those that fail more than on the release before
/// first (by points of failed calls gained), then those that got slower
/// (by how many times their p95 grew), then the others.
fn rank(d: &Dependency) -> (u8, f64) {
    match &d.change {
        Some(c) if c.fails_more => (0, c.error_rate - c.previous_error_rate),
        Some(c) if c.slower => (1, c.p95_ms / c.previous_p95_ms.max(1.0)),
        _ => (2, 0.0),
    }
}

/// Failing first, then the largest change, then the most called.
fn sort(hosts: &mut [Dependency]) {
    hosts.sort_by(|a, b| {
        let (ra, rb) = (rank(a), rank(b));
        ra.0.cmp(&rb.0)
            .then(rb.1.total_cmp(&ra.1))
            .then(b.stat.calls.cmp(&a.stat.calls))
    });
}

/// `dependencies.list`: the project's outgoing dependencies.
pub struct List;

impl Command for List {
    const NAME: &'static str = "dependencies.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Dependencies;

    async fn run<H: Host>(host: &H, _: &Caller, input: ListInput) -> Result<Dependencies, Error> {
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
        let q = host.query();
        let (list, pair) = futures::join!(
            dependencies::list(q, &p.id, &environment, days),
            dependencies::latest_releases(q, &p.id, &environment),
        );
        let failed = |e: crate::EngineError| match e {
            crate::EngineError::InvalidFilter(_) => Error::Invalid(e.to_string()),
            _ => Error::Unavailable(e.to_string()),
        };
        let list = list.map_err(failed)?;
        let pair = pair.map_err(failed)?;
        let note = list.is_empty().then(|| {
            "No outgoing call stats yet: call the Node SDK's `captureOutgoing()` (sdk/SERVER.md), \
             or export `http.client.request.duration` over OTLP to /v1/metrics."
                .to_owned()
        });
        let (release, previous_release, changes) = match pair {
            Some(pair) => {
                let changes = pair
                    .previous
                    .as_ref()
                    .map(|(_, before)| dependencies::changes(&pair.current_hosts, before))
                    .unwrap_or_default();
                (Some(pair.current), pair.previous.map(|(p, _)| p), changes)
            }
            None => (None, None, Vec::new()),
        };
        let mut hosts: Vec<Dependency> = list
            .into_iter()
            .map(|stat| Dependency {
                error_rate: stat.error_rate(),
                p95_ms: stat.p95(),
                change: changes.iter().find(|c| c.host == stat.host).cloned(),
                stat,
            })
            .collect();
        let worse = |d: &Dependency| d.change.as_ref().is_some_and(DependencyChange::worse);
        sort(&mut hosts);
        Ok(Dependencies {
            environment,
            days,
            calls: hosts.iter().map(|d| d.stat.calls).sum(),
            errors: hosts.iter().map(|d| d.stat.errors).sum(),
            release,
            previous_release,
            worse: hosts.iter().filter(|d| worse(d)).count(),
            hosts,
            note,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, calls: i64, errors: (f64, f64), p95: (f64, f64)) -> Dependency {
        let change = DependencyChange {
            host: name.into(),
            calls,
            previous_calls: calls,
            error_rate: errors.0,
            previous_error_rate: errors.1,
            p95_ms: p95.0,
            previous_p95_ms: p95.1,
            fails_more: errors.0 > errors.1,
            slower: p95.0 > p95.1,
        };
        Dependency {
            stat: DependencyStat {
                host: name.into(),
                calls,
                ..DependencyStat::default()
            },
            error_rate: errors.0,
            p95_ms: p95.0,
            change: Some(change),
        }
    }

    #[test]
    fn failing_hosts_come_first_then_the_largest_change_then_the_most_called() {
        let mut hosts = vec![
            host("busy.example", 9_000, (0.01, 0.01), (100.0, 100.0)),
            host("a-bit-slower.example", 500, (0.0, 0.0), (300.0, 200.0)),
            host("much-slower.example", 400, (0.0, 0.0), (900.0, 100.0)),
            host("fails-a-bit.example", 600, (0.08, 0.01), (100.0, 100.0)),
            host("fails-a-lot.example", 300, (0.6, 0.01), (100.0, 100.0)),
            host("quiet.example", 50, (0.0, 0.0), (100.0, 100.0)),
        ];
        sort(&mut hosts);
        let order: Vec<&str> = hosts.iter().map(|d| d.stat.host.as_str()).collect();
        assert_eq!(
            order,
            [
                "fails-a-lot.example",
                "fails-a-bit.example",
                "much-slower.example",
                "a-bit-slower.example",
                "busy.example",
                "quiet.example",
            ]
        );
    }
}

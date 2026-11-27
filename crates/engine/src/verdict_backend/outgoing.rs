//! The outgoing dependencies of the Backend group (#630, D-090): a host
//! that fails more or got slower than on the release before is shown,
//! marked *shown, not judged*, in words that say it is usually not the
//! release's fault. It never is a check, so it never changes the verdict.

use crate::board_verdict::Row;
use crate::verdict::{Outcome, Report};

/// Hosts the group shows at most.
const MAX_HOSTS: usize = 3;

/// The rows of the hosts that got worse, at most [`MAX_HOSTS`], and how
/// many were left out.
pub(super) fn rows(r: &Report) -> (Vec<Row>, usize) {
    let mut rows = Vec::new();
    for d in r.dependencies.iter().take(MAX_HOSTS) {
        let mut row = |what: &str, from: String, to: String| {
            rows.push(Row {
                label: format!("{} {what}, usually not the release's fault", d.host),
                from: Some(from),
                to,
                outcome: Outcome::Info,
                key: false,
            });
        };
        if d.fails_more {
            row(
                "fails more",
                super::shown("5xx", d.previous_error_rate),
                super::shown("5xx", d.error_rate),
            );
        }
        if d.slower {
            row(
                "got slower",
                super::shown("p95", d.previous_p95_ms),
                super::shown("p95", d.p95_ms),
            );
        }
    }
    (rows, r.dependencies.len().saturating_sub(MAX_HOSTS))
}

#[cfg(test)]
mod tests {
    use crate::dependencies::DependencyStat;
    use crate::routes::BUCKETS;
    use crate::verdict::{Outcome, Side, Verdict, compare};

    fn side(release: &str, host: DependencyStat) -> Side {
        Side {
            release: release.into(),
            users: 120,
            sessions: 400,
            bounced: 100,
            error_sessions: 8,
            task_sessions: 200,
            tasks: 400,
            avg_secs: 60.0,
            dependencies: vec![host],
            ..Side::default()
        }
    }

    fn host(errors: i64, bucket: usize) -> DependencyStat {
        let mut buckets = vec![0; BUCKETS];
        if let Some(b) = buckets.get_mut(bucket) {
            *b = 500;
        }
        DependencyStat {
            host: "api.stripe.com".into(),
            calls: 500,
            errors,
            max_ms: 20_000,
            buckets,
            ..DependencyStat::default()
        }
    }

    #[test]
    fn a_dependency_regression_is_shown_in_the_backend_group_and_never_regresses_the_release() {
        let prev = side("v1", host(0, 5));
        let same = compare("production", &side("v2", host(0, 5)), Some(&prev), 30, &[]);
        let r = compare("production", &side("v2", host(400, 12)), Some(&prev), 30, &[]);
        assert_ne!(r.verdict, Some(Verdict::Regressed), "{:?}", r.reasons);
        assert_eq!(r.verdict, same.verdict, "{:?}", r.reasons);
        assert!(crate::board_verdict::lead(&r).is_none());
        let g = super::super::backend_layer(&r).unwrap();
        let shown: Vec<_> = g
            .rows
            .iter()
            .filter(|x| x.label.contains("api.stripe.com"))
            .collect();
        assert!(shown.len() >= 2, "fails more and got slower: {g:?}");
        assert!(
            shown.iter().all(|x| x.outcome == Outcome::Info
                && x.label.contains("usually not the release")),
            "{g:?}"
        );
        assert!(g.rows.iter().all(|x| x.outcome != Outcome::Worse), "{g:?}");
        // A quiet dependency shows nothing.
        assert!(super::super::backend_layer(&same).is_none());
    }
}

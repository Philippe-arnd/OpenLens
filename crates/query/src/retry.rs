//! A query that lost a segment runs once more (#459).
//!
//! Compaction, click trimming and erasure replace a day's files while
//! queries run. A query that listed the day before the rewrite then opens a
//! file that is gone, and failed with a storage error. The listing is not
//! cached (#272), so the same query run again sees the new files.

use std::future::Future;

use crate::QueryError;

/// Whether `e` says a listed Parquet file was gone when it was opened.
pub(crate) fn segment_gone(e: &QueryError) -> bool {
    match e {
        QueryError::Rejected(m) | QueryError::Storage(m) => {
            m.contains(".parquet")
                && (m.contains("No such file or directory") || m.contains("not found"))
        }
        _ => false,
    }
}

/// Run `attempt`, and once more if a segment was gone.
pub(crate) async fn once_more<T, F, Fut>(mut attempt: F) -> Result<T, QueryError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, QueryError>>,
{
    match attempt().await {
        Err(e) if segment_gone(&e) => {
            tracing::debug!(%e, "a segment was replaced during the query; running it again");
            attempt().await
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// The message DataFusion gave when compaction replaced a day during a
    /// board (2026-11-25).
    const GONE: &str = "Parquet error: Parquet error: Failed to fetch metadata for file \
        tmp/data/events/project=default/date=2026-08-31/01a212d1.parquet: Object Store error: \
        Object at location /tmp/data/events/project=default/date=2026-08-31/01a212d1.parquet \
        not found: No such file or directory (os error 2)";

    #[tokio::test]
    async fn a_lost_segment_runs_once_more_and_nothing_else_does() {
        let mut calls = 0;
        let r = once_more(|| {
            calls += 1;
            let first = calls == 1;
            async move {
                if first {
                    Err(QueryError::Storage(GONE.into()))
                } else {
                    Ok(7)
                }
            }
        })
        .await;
        assert_eq!(r.unwrap(), 7);
        assert_eq!(calls, 2);

        // Other errors, and a second loss in a row, are returned as they are.
        let mut calls = 0;
        let r: Result<i32, _> = once_more(|| {
            calls += 1;
            async { Err(QueryError::Rejected("syntax error".into())) }
        })
        .await;
        assert!(r.is_err());
        assert_eq!(calls, 1);
        let mut calls = 0;
        let r: Result<i32, _> = once_more(|| {
            calls += 1;
            async { Err(QueryError::Storage(GONE.into())) }
        })
        .await;
        assert!(r.is_err());
        assert_eq!(calls, 2);
        assert!(!segment_gone(&QueryError::TooBig));
    }
}

//! A query that lost a segment runs once more (#459), and so does one
//! that ran out of the shared pool beside other queries (D-071).
//!
//! Compaction, click trimming and erasure replace a day's files while
//! queries run. A query that listed the day before the rewrite then opens a
//! file that is gone, and failed with a storage error. The listing is not
//! cached (#272), so the same query run again sees the new files.
//!
//! A board runs some forty queries, a dozen or more at a time, in one
//! memory pool (D-033, D-051): each one's fair share of the pool shrinks
//! with their number, so a query could run out of memory only because the
//! others ran, fail the whole board, and leave nothing to cache: the next
//! page computed the board again, and failed again as often. Such a query
//! runs once more when the others are done (or after [`CALM`]), one such
//! retry at a time. A query that ran out of memory alone is too big: it is
//! not run again.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore};

use crate::QueryError;

/// Queries running now, in this process.
static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// Woken when the last running query ends.
static IDLE: Notify = Notify::const_new();
/// One retry after running out of memory at a time.
static RETRY: Semaphore = Semaphore::const_new(1);
/// Longest wait for the other queries before running again anyway.
const CALM: Duration = Duration::from_secs(10);

/// Counts a query as running while it lives.
struct Running;

impl Running {
    fn start() -> Self {
        RUNNING.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if RUNNING.fetch_sub(1, Ordering::SeqCst) == 1 {
            IDLE.notify_waiters();
        }
    }
}

/// Wait until no query runs, for [`CALM`] at most.
async fn calm() {
    let wait = async {
        loop {
            let mut idle = std::pin::pin!(IDLE.notified());
            idle.as_mut().enable();
            if RUNNING.load(Ordering::SeqCst) == 0 {
                return;
            }
            idle.await;
        }
    };
    let _ = tokio::time::timeout(CALM, wait).await;
}

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

/// Run `attempt`, and once more if a segment was gone, or if it ran out
/// of memory while other queries ran (D-071).
pub(crate) async fn once_more<T, F, Fut>(mut attempt: F) -> Result<T, QueryError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, QueryError>>,
{
    let first = {
        let _running = Running::start();
        let r = attempt().await;
        // Others were running when it ended (itself still counted).
        let crowded = RUNNING.load(Ordering::SeqCst) > 1;
        (r, crowded)
    };
    match first {
        (Err(e), _) if segment_gone(&e) => {
            tracing::debug!(%e, "a segment was replaced during the query; running it again");
            let _running = Running::start();
            attempt().await
        }
        (Err(QueryError::TooBig), true) => {
            tracing::info!("a query ran out of memory beside others; running it again after them");
            let _turn = RETRY.acquire().await;
            calm().await;
            let _running = Running::start();
            attempt().await
        }
        (other, _) => other,
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

    /// D-071: out of memory while another query ran, the query runs again
    /// once the other is done.
    #[tokio::test]
    async fn out_of_memory_beside_another_query_runs_again_after_it() {
        let (end, ended) = tokio::sync::oneshot::channel::<()>();
        let mut ended = Some(ended);
        // Another query, running until it is told to end.
        let other = tokio::spawn(once_more(move || {
            let e = ended.take();
            async move {
                if let Some(e) = e {
                    let _ = e.await;
                }
                Ok::<_, QueryError>(1)
            }
        }));
        while RUNNING.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let mut end = Some(end);
        let mut calls = 0;
        let r = once_more(|| {
            calls += 1;
            let first = calls == 1;
            let end = end.take();
            async move {
                if first {
                    // The other ends once this one has failed.
                    tokio::spawn(async move {
                        if let Some(end) = end {
                            let _ = end.send(());
                        }
                    });
                    Err(QueryError::TooBig)
                } else {
                    Ok(2)
                }
            }
        })
        .await;
        assert_eq!(r.unwrap(), 2);
        assert_eq!(calls, 2, "run once more");
        assert_eq!(other.await.unwrap().unwrap(), 1);
    }
}

//! The blocking half of a rollup pass (#676): the maintenance lock (a std
//! mutex an erasure can hold for minutes) and the Parquet write run on a
//! blocking thread, never on an async worker, so `/capture` keeps its
//! workers while an erasure runs.

use std::path::Path;

use chrono::NaiveDate;

use crate::{AppState, privacy};

/// Write one closed day's rollup tables with `write` (given the data
/// directory and the project), under the maintenance lock, on a blocking
/// thread. `None` when the pass must stop (the lock is poisoned or the
/// task died); otherwise whether the tables were written. A failed write
/// is logged and counts as not written.
pub(crate) async fn day<F>(
    state: &AppState,
    project: &str,
    day: NaiveDate,
    write: F,
) -> Option<bool>
where
    F: FnOnce(&Path, &str) -> openlens_storage::Result<bool> + Send + 'static,
{
    let (st, id) = (state.clone(), project.to_owned());
    let done = tokio::task::spawn_blocking(move || {
        let Ok(_guard) = privacy::MAINTENANCE.lock() else {
            return None;
        };
        match write(&st.data_dir, &id) {
            Ok(true) => {
                st.boards.closed().days_changed(&id, [day]);
                Some(true)
            }
            Ok(false) => Some(false),
            Err(err) => {
                tracing::warn!(project = %id, %day, %err, "rollup not written");
                Some(false)
            }
        }
    });
    match done.await {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(project, %err, "rollup write crashed");
            None
        }
    }
}

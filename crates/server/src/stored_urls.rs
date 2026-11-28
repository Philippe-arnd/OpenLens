//! The one-off cleaning of stored URLs (#290). Moved out of `lib.rs` as is
//! (file length, #307).

use crate::AppState;

/// One-off rewrite of the URLs stored before capture cleaned them (#290),
/// recorded per project so it runs once. The caller holds the maintenance
/// lock.
pub(crate) fn clean(state: &AppState, project: &str) {
    const DONE: &str = "urls_cleaned";
    if state
        .meta
        .project_setting(project, DONE)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    let run = crate::flush_now(state).and_then(|_| {
        openlens_storage::clean_urls(&state.data_dir, project).map_err(|e| e.to_string())
    });
    match run {
        Ok(n) => {
            state.boards.closed().project_changed(project);
            tracing::info!(project, n, "query strings removed from stored URLs");
            if let Err(err) = state.meta.set_project_setting(project, DONE, "1") {
                tracing::warn!(project, %err, "URL cleaning not recorded");
            }
        }
        Err(err) => tracing::warn!(project, %err, "URL cleaning failed"),
    }
}

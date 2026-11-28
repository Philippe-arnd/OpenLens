//! Deleting a project's files (#664 audit): the folders a project owns,
//! its removal on delete, and the sweep that catches files a late flush
//! wrote after it.

use crate::AppState;

/// The folders that hold a project's files: events, rollups, replays and
/// source maps.
fn project_dirs(st: &AppState, project: &str) -> Vec<std::path::PathBuf> {
    let mut dirs = vec![
        st.data_dir
            .join("replay")
            .join(format!("project={project}")),
        st.data_dir
            .join("sourcemaps")
            .join(format!("project={project}")),
    ];
    if let Ok(d) = openlens_storage::project_dir(&st.data_dir, project) {
        dirs.push(d);
    }
    if let Ok(d) = openlens_storage::rollup::rollups_dir(&st.data_dir, project) {
        dirs.push(d);
    }
    dirs
}

fn remove_dirs(dirs: &[std::path::PathBuf]) -> Result<(), String> {
    for d in dirs {
        if d.exists() {
            std::fs::remove_dir_all(d).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Delete a project and every file of it (blocking): events, replays,
/// source maps and its metadata. False when it did not exist.
///
/// The metadata goes first, so its key is refused from then on; then,
/// under the maintenance lock that compaction and rollups hold, buffered
/// events are flushed and the folders removed. A capture accepted just
/// before the delete and flushed after it is swept by the next
/// compaction ([`sweep_deleted_projects`]).
pub(crate) fn purge_project(st: &AppState, project: &str) -> Result<bool, String> {
    let existed = st.meta.delete_project(project).map_err(|e| e.to_string())?;
    if existed {
        let _guard = crate::privacy::MAINTENANCE
            .lock()
            .map_err(|_| "maintenance lock poisoned")?;
        crate::flush_now(st)?;
        remove_dirs(&project_dirs(st, project))?;
    }
    Ok(existed)
}

/// Remove the folders of projects that no longer exist (blocking; run by
/// compaction under the maintenance lock, public for tests). Each one is checked against the
/// metadata store right before it goes, so a project created meanwhile
/// keeps its files, and nothing is removed when the store cannot answer.
pub fn sweep_deleted_projects(st: &AppState) -> usize {
    let mut parents = vec![st.data_dir.join("replay"), st.data_dir.join("sourcemaps")];
    for d in [
        openlens_storage::project_dir(&st.data_dir, "x"),
        openlens_storage::rollup::rollups_dir(&st.data_dir, "x"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(p) = d.parent() {
            parents.push(p.to_path_buf());
        }
    }
    let mut ids = std::collections::BTreeSet::new();
    for parent in parents {
        let Ok(entries) = std::fs::read_dir(&parent) else {
            continue;
        };
        for e in entries.flatten() {
            if let Some(id) = e
                .file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("project="))
                && openlens_storage::valid_project(id)
            {
                ids.insert(id.to_owned());
            }
        }
    }
    let mut swept = 0;
    for id in ids {
        if matches!(st.meta.project(&id), Ok(None)) {
            match remove_dirs(&project_dirs(st, &id)) {
                Ok(()) => swept += 1,
                Err(err) => {
                    tracing::warn!(project = %id, %err, "removing a deleted project's files failed")
                }
            }
        }
    }
    if swept > 0 {
        tracing::info!(projects = swept, "removed files of deleted projects");
    }
    swept
}

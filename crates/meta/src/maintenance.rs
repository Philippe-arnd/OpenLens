//! Planned maintenance windows (#563, D-076): one-off periods, declared
//! ahead, when a project's components (production or a named environment)
//! are expected to be down. Incidents still open during them but are not
//! announced, and their time is left out of availability.
//!
//! A small table, in its own module so the uptime history (#559) and the
//! status page (#561) can grow next to it. Windows ended more than
//! [`KEEP_DAYS`] ago are dropped when a new one is scheduled.

use super::*;

/// The table, created next to the main schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS maintenance_windows (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project TEXT NOT NULL,
  starts_at INTEGER NOT NULL,
  ends_at INTEGER NOT NULL,
  components TEXT NOT NULL,
  text TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  created_by TEXT NOT NULL,
  announced_at INTEGER
);
CREATE INDEX IF NOT EXISTS maintenance_windows_project ON maintenance_windows (project, ends_at);
";

/// Ended windows are kept this long (a year and a month), for the
/// availability of past periods and an SLO over a year (#560).
pub const KEEP_DAYS: i64 = 400;

/// One maintenance window as stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Window {
    /// Id.
    pub id: i64,
    /// Start (Unix seconds).
    pub starts_at: i64,
    /// End (Unix seconds, excluded).
    pub ends_at: i64,
    /// What it covers: `production` or environment names.
    pub components: Vec<String>,
    /// What is done, for the people who read it.
    pub text: String,
    /// When it was scheduled (Unix seconds).
    pub created_at: i64,
    /// Who scheduled it (an email, `owner` or `api`).
    pub created_by: String,
    /// When it was announced on the alert channels, if it was.
    pub announced_at: Option<i64>,
}

/// A window to store.
#[derive(Debug, Clone)]
pub struct NewWindow<'a> {
    /// Start (Unix seconds).
    pub starts_at: i64,
    /// End (Unix seconds, excluded).
    pub ends_at: i64,
    /// What it covers.
    pub components: &'a [String],
    /// What is done.
    pub text: &'a str,
    /// Who schedules it.
    pub created_by: &'a str,
    /// Now (Unix seconds).
    pub now: i64,
}

const COLS: &str = "id, starts_at, ends_at, components, text, created_at, created_by, announced_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Window> {
    let comps: String = r.get(3)?;
    Ok(Window {
        id: r.get(0)?,
        starts_at: r.get(1)?,
        ends_at: r.get(2)?,
        components: comps
            .split(',')
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect(),
        text: r.get(4)?,
        created_at: r.get(5)?,
        created_by: r.get(6)?,
        announced_at: r.get(7)?,
    })
}

impl Meta {
    /// The windows of a project that overlap `[from, to)`, by start
    /// (at most 500).
    pub fn maintenance_windows(&self, project: &str, from: i64, to: i64) -> Result<Vec<Window>> {
        self.read(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {COLS} FROM maintenance_windows \
                 WHERE project = ?1 AND ends_at > ?2 AND starts_at < ?3 ORDER BY starts_at, id LIMIT 500"
            ))?;
            let rows = st
                .query_map(params![project, from, to], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One window of a project.
    pub fn maintenance_window(&self, project: &str, id: i64) -> Result<Option<Window>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM maintenance_windows WHERE project = ?1 AND id = ?2"),
                params![project, id],
                row,
            )
            .optional()?)
        })
    }

    /// Windows of a project that have not ended at `now` (planned or
    /// running).
    pub fn open_maintenance_count(&self, project: &str, now: i64) -> Result<i64> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM maintenance_windows WHERE project = ?1 AND ends_at > ?2",
                params![project, now],
                |r| r.get(0),
            )?)
        })
    }

    /// Store a window (checked by the caller) and drop the project's
    /// windows ended more than [`KEEP_DAYS`] ago. Returns its id.
    pub fn add_maintenance(&self, project: &str, w: &NewWindow<'_>) -> Result<i64> {
        if w.ends_at <= w.starts_at {
            return Err(MetaError::Invalid("a window ends after it starts"));
        }
        self.with(|c| {
            c.execute(
                "INSERT INTO maintenance_windows (project, starts_at, ends_at, components, text, created_at, created_by) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    project,
                    w.starts_at,
                    w.ends_at,
                    w.components.join(","),
                    w.text,
                    w.now,
                    w.created_by
                ],
            )?;
            let id = c.last_insert_rowid();
            c.execute(
                "DELETE FROM maintenance_windows WHERE project = ?1 AND ends_at < ?2",
                params![project, w.now - KEEP_DAYS * 86400],
            )?;
            Ok(id)
        })
    }

    /// Move a window's end. False when there is no such window.
    pub fn set_maintenance_end(&self, project: &str, id: i64, ends_at: i64) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "UPDATE maintenance_windows SET ends_at = ?3 WHERE project = ?1 AND id = ?2 AND starts_at < ?3",
                params![project, id, ends_at],
            )?;
            Ok(n > 0)
        })
    }

    /// Mark a window as announced on the alert channels.
    pub fn maintenance_announced(&self, project: &str, id: i64, at: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE maintenance_windows SET announced_at = ?3 WHERE project = ?1 AND id = ?2",
                params![project, id, at],
            )?;
            Ok(())
        })
    }

    /// Remove a window. False when there is no such window.
    pub fn delete_maintenance(&self, project: &str, id: i64) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "DELETE FROM maintenance_windows WHERE project = ?1 AND id = ?2",
                params![project, id],
            )?;
            Ok(n > 0)
        })
    }
}

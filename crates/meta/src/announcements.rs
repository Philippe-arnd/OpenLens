//! Status announcements and their dated updates (#562).
//!
//! An announcement is what the team says about the app's state: a title,
//! a plain-text body, a level, the components it concerns (`production`
//! or environment names) and a start and optional end. It may follow a
//! detected incident as a list of dated updates (investigating,
//! identified, monitoring, resolved). They are kept as long as the
//! project, at most [`MAX_ANNOUNCEMENTS`] per project and [`MAX_UPDATES`]
//! updates each. The checks on the text are the engine's
//! (`openlens_engine::commands::announcements`); this module stores.

use super::*;

/// Announcements kept per project; a new one past it is refused.
pub const MAX_ANNOUNCEMENTS: i64 = 1_000;

/// Updates per announcement; a new one past it is refused.
pub const MAX_UPDATES: i64 = 100;

/// The tables, created next to the main schema (kept here so the uptime
/// and status work of other changes does not touch the same lines).
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS announcements (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  level TEXT NOT NULL,
  components TEXT NOT NULL,
  starts_at INTEGER NOT NULL,
  ends_at INTEGER,
  pinned INTEGER NOT NULL DEFAULT 0,
  incident INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS announcements_project ON announcements (project, starts_at);
CREATE TABLE IF NOT EXISTS announcement_updates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project TEXT NOT NULL,
  announcement INTEGER NOT NULL,
  at INTEGER NOT NULL,
  state TEXT NOT NULL,
  body TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS announcement_updates_of ON announcement_updates (announcement, at);
";

/// One dated update of an announcement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AnnouncementUpdate {
    /// Id.
    pub id: i64,
    /// When it was posted (Unix seconds).
    pub at: i64,
    /// `investigating`, `identified`, `monitoring` or `resolved`.
    pub state: String,
    /// What the team says, plain text.
    pub body: String,
}

/// What a caller sets on an announcement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AnnouncementFields {
    /// Title, one line.
    pub title: String,
    /// Body, plain text.
    pub body: String,
    /// `information`, `degraded`, `outage` or `resolved`.
    pub level: String,
    /// `production` or environment names.
    pub components: Vec<String>,
    /// Start (Unix seconds).
    pub starts_at: i64,
    /// End (Unix seconds), if any.
    pub ends_at: Option<i64>,
    /// Pinned at the top of the page (information only, one per project).
    pub pinned: bool,
    /// The incident it follows, if any.
    pub incident: Option<i64>,
}

/// An announcement as stored, with its updates oldest first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Announcement {
    /// Id.
    pub id: i64,
    /// What was set.
    #[serde(flatten)]
    pub fields: AnnouncementFields,
    /// Created (Unix seconds).
    pub created_at: i64,
    /// Last changed (Unix seconds).
    pub updated_at: i64,
    /// Its updates, oldest first.
    pub updates: Vec<AnnouncementUpdate>,
}

/// An incident the probe or a check recorded, with its id (to link an
/// announcement to it).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IncidentRef {
    /// Id.
    pub id: i64,
    /// Its rule (`down`, `traffic`, `env:<env>:down`…).
    pub rule: String,
    /// Start (Unix seconds).
    pub started_at: i64,
    /// End (Unix seconds), once closed.
    pub ended_at: Option<i64>,
    /// What happened.
    pub cause: String,
}

const COLS: &str = "id, title, body, level, components, starts_at, ends_at, pinned, incident, created_at, updated_at";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Announcement> {
    let components: String = r.get(4)?;
    Ok(Announcement {
        id: r.get(0)?,
        fields: AnnouncementFields {
            title: r.get(1)?,
            body: r.get(2)?,
            level: r.get(3)?,
            components: components
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            starts_at: r.get(5)?,
            ends_at: r.get(6)?,
            pinned: r.get::<_, i64>(7)? != 0,
            incident: r.get(8)?,
        },
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
        updates: Vec::new(),
    })
}

fn updates_of(c: &Connection, id: i64) -> Result<Vec<AnnouncementUpdate>> {
    let mut st = c.prepare(
        "SELECT id, at, state, body FROM announcement_updates WHERE announcement = ?1 ORDER BY at, id LIMIT ?2",
    )?;
    let rows = st
        .query_map(params![id, MAX_UPDATES], |r| {
            Ok(AnnouncementUpdate {
                id: r.get(0)?,
                at: r.get(1)?,
                state: r.get(2)?,
                body: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn one(c: &Connection, project: &str, id: i64) -> Result<Option<Announcement>> {
    let a = c
        .query_row(
            &format!("SELECT {COLS} FROM announcements WHERE project = ?1 AND id = ?2"),
            params![project, id],
            from_row,
        )
        .optional()?;
    match a {
        Some(mut a) => {
            a.updates = updates_of(c, a.id)?;
            Ok(Some(a))
        }
        None => Ok(None),
    }
}

/// The other pinned announcements of the project lose their pin.
fn unpin_others(c: &Connection, project: &str, id: i64) -> Result<()> {
    c.execute(
        "UPDATE announcements SET pinned = 0 WHERE project = ?1 AND id != ?2 AND pinned = 1",
        params![project, id],
    )?;
    Ok(())
}

impl Meta {
    /// The project's announcements that started since `since` or had not
    /// ended by then (Unix seconds), newest start first, with their
    /// updates oldest first. Scheduled ones (starting later) are included.
    pub fn announcements(&self, project: &str, since: i64) -> Result<Vec<Announcement>> {
        self.read(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {COLS} FROM announcements WHERE project = ?1 \
                 AND (starts_at >= ?2 OR ends_at IS NULL OR ends_at >= ?2) \
                 ORDER BY starts_at DESC, id DESC LIMIT ?3"
            ))?;
            let mut rows = st
                .query_map(params![project, since, MAX_ANNOUNCEMENTS], from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for a in &mut rows {
                a.updates = updates_of(c, a.id)?;
            }
            Ok(rows)
        })
    }

    /// One announcement of the project, with its updates.
    pub fn announcement(&self, project: &str, id: i64) -> Result<Option<Announcement>> {
        self.read(|c| one(c, project, id))
    }

    /// Store a new announcement (already checked) and, when `first` is
    /// given, its first update `(state, body)`. A pinned one unpins the
    /// project's others. Refused past [`MAX_ANNOUNCEMENTS`].
    pub fn create_announcement(
        &self,
        project: &str,
        f: &AnnouncementFields,
        first: Option<(&str, &str)>,
        now: i64,
    ) -> Result<Announcement> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let n: i64 = tx.query_row(
                "SELECT COUNT(*) FROM announcements WHERE project = ?1",
                [project],
                |r| r.get(0),
            )?;
            if n >= MAX_ANNOUNCEMENTS {
                return Err(MetaError::Invalid(
                    "this project keeps 1000 announcements at most: delete old ones first",
                ));
            }
            tx.execute(
                "INSERT INTO announcements (project, title, body, level, components, starts_at, ends_at, pinned, incident, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    project,
                    f.title,
                    f.body,
                    f.level,
                    f.components.join(","),
                    f.starts_at,
                    f.ends_at,
                    i64::from(f.pinned),
                    f.incident,
                    now
                ],
            )?;
            let id = tx.last_insert_rowid();
            if f.pinned {
                unpin_others(&tx, project, id)?;
            }
            if let Some((state, body)) = first {
                tx.execute(
                    "INSERT INTO announcement_updates (project, announcement, at, state, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![project, id, now, state, body],
                )?;
            }
            let a = one(&tx, project, id)?;
            tx.commit()?;
            a.ok_or(MetaError::Invalid("the announcement was not stored"))
        })
    }

    /// Replace what is set on an announcement (already checked). A pinned
    /// one unpins the project's others. `None` when it does not exist.
    pub fn save_announcement(
        &self,
        project: &str,
        id: i64,
        f: &AnnouncementFields,
        now: i64,
    ) -> Result<Option<Announcement>> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let n = tx.execute(
                "UPDATE announcements SET title = ?3, body = ?4, level = ?5, components = ?6, starts_at = ?7, \
                 ends_at = ?8, pinned = ?9, incident = ?10, updated_at = ?11 WHERE project = ?1 AND id = ?2",
                params![
                    project,
                    id,
                    f.title,
                    f.body,
                    f.level,
                    f.components.join(","),
                    f.starts_at,
                    f.ends_at,
                    i64::from(f.pinned),
                    f.incident,
                    now
                ],
            )?;
            if n == 0 {
                return Ok(None);
            }
            if f.pinned {
                unpin_others(&tx, project, id)?;
            }
            let a = one(&tx, project, id)?;
            tx.commit()?;
            Ok(a)
        })
    }

    /// Add a dated update to an announcement, and, when `fields` is
    /// given, replace what is set on it in the same transaction (a
    /// *resolved* update also ends it). `None` when it does not exist;
    /// refused past [`MAX_UPDATES`].
    pub fn add_announcement_update(
        &self,
        project: &str,
        id: i64,
        state: &str,
        body: &str,
        fields: Option<&AnnouncementFields>,
        now: i64,
    ) -> Result<Option<Announcement>> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let exists = tx
                .prepare("SELECT 1 FROM announcements WHERE project = ?1 AND id = ?2")?
                .exists(params![project, id])?;
            if !exists {
                return Ok(None);
            }
            let n: i64 = tx.query_row(
                "SELECT COUNT(*) FROM announcement_updates WHERE announcement = ?1",
                [id],
                |r| r.get(0),
            )?;
            if n >= MAX_UPDATES {
                return Err(MetaError::Invalid(
                    "an announcement keeps 100 updates at most: write a new one",
                ));
            }
            tx.execute(
                "INSERT INTO announcement_updates (project, announcement, at, state, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![project, id, now, state, body],
            )?;
            if let Some(f) = fields {
                tx.execute(
                    "UPDATE announcements SET level = ?3, ends_at = ?4, pinned = ?5, updated_at = ?6 WHERE project = ?1 AND id = ?2",
                    params![project, id, f.level, f.ends_at, i64::from(f.pinned), now],
                )?;
            } else {
                tx.execute(
                    "UPDATE announcements SET updated_at = ?3 WHERE project = ?1 AND id = ?2",
                    params![project, id, now],
                )?;
            }
            let a = one(&tx, project, id)?;
            tx.commit()?;
            Ok(a)
        })
    }

    /// Delete an announcement and its updates. False when it did not exist.
    pub fn delete_announcement(&self, project: &str, id: i64) -> Result<bool> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM announcement_updates WHERE project = ?1 AND announcement = ?2",
                params![project, id],
            )?;
            let n = tx.execute(
                "DELETE FROM announcements WHERE project = ?1 AND id = ?2",
                params![project, id],
            )?;
            tx.commit()?;
            Ok(n > 0)
        })
    }

    /// Incidents started since `since` or still open, newest first, with
    /// their ids (at most 100).
    pub fn incident_refs(&self, project: &str, since: i64) -> Result<Vec<IncidentRef>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, rule, started_at, ended_at, cause FROM incidents \
                 WHERE project = ?1 AND (started_at >= ?2 OR ended_at IS NULL) ORDER BY started_at DESC LIMIT 100",
            )?;
            let rows = st
                .query_map(params![project, since], |r| {
                    Ok(IncidentRef {
                        id: r.get(0)?,
                        rule: r.get(1)?,
                        started_at: r.get(2)?,
                        ended_at: r.get(3)?,
                        cause: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One incident of the project, by id.
    pub fn incident_ref(&self, project: &str, id: i64) -> Result<Option<IncidentRef>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT id, rule, started_at, ended_at, cause FROM incidents WHERE project = ?1 AND id = ?2",
                params![project, id],
                |r| {
                    Ok(IncidentRef {
                        id: r.get(0)?,
                        rule: r.get(1)?,
                        started_at: r.get(2)?,
                        ended_at: r.get(3)?,
                        cause: r.get(4)?,
                    })
                },
            )
            .optional()?)
        })
    }
}

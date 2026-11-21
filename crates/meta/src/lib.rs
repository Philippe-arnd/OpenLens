//! Metadata store (SQLite): projects, API keys, persons and role history.
//!
//! Small, transactional and rarely written, so it lives in one SQLite file
//! next to the event segments. Events themselves never go here.
//!
//! Layer: L1 (see `docs/architecture.md`).

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};

/// Metadata errors.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    /// SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The connection mutex was poisoned by a panic elsewhere.
    #[error("metadata store lock poisoned")]
    Poisoned,
    /// Invalid input.
    #[error("{0}")]
    Invalid(&'static str),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, MetaError>;

/// A project: one app being measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Id, also the storage partition name (`[a-z0-9-]`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Public capture key (safe to ship in the browser).
    pub api_key: String,
}

/// A known user, with the dimensions OpenLens slices by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The app's user id.
    pub distinct_id: String,
    /// Raw role value, as last sent.
    pub role: Option<String>,
    /// Account / organisation.
    pub account: Option<String>,
    /// Plan.
    pub plan: Option<String>,
    /// First event seen.
    pub first_seen: String,
    /// Last identify seen.
    pub last_seen: String,
}

/// The metadata store.
pub struct Meta {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  api_key TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS persons (
  project TEXT NOT NULL,
  distinct_id TEXT NOT NULL,
  role TEXT, account TEXT, plan TEXT,
  first_seen TEXT NOT NULL,
  last_seen TEXT NOT NULL,
  PRIMARY KEY (project, distinct_id)
);
CREATE TABLE IF NOT EXISTS role_history (
  project TEXT NOT NULL,
  distinct_id TEXT NOT NULL,
  role TEXT NOT NULL,
  since TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS role_history_person ON role_history (project, distinct_id, since);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Random lowercase alphanumeric string.
fn token(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

/// Turn a display name into a project id: lowercase `[a-z0-9-]`, ≤ 40 chars.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

impl Meta {
    /// Open (or create) the store at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// In-memory store, for tests.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().map_err(|_| MetaError::Poisoned)?;
        f(&conn)
    }

    /// Get a setting, or store `init()` under `key` and return it.
    pub fn setting_or_init(&self, key: &str, init: impl FnOnce() -> String) -> Result<String> {
        self.with(|c| {
            if let Some(v) = c
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                    r.get(0)
                })
                .optional()?
            {
                return Ok(v);
            }
            let v = init();
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                params![key, v],
            )?;
            Ok(v)
        })
    }

    /// The admin token, generated on first call.
    pub fn admin_token(&self) -> Result<String> {
        self.setting_or_init("admin_token", || format!("adm_{}", token(32)))
    }

    /// Create a project. A fresh capture key is generated unless `key` is given.
    pub fn create_project(&self, name: &str, key: Option<&str>) -> Result<Project> {
        let id = slug(name);
        if id.is_empty() {
            return Err(MetaError::Invalid("project name needs a letter or digit"));
        }
        let api_key = key.map_or_else(|| format!("pk_{}", token(24)), str::to_owned);
        self.with(|c| {
            c.execute(
                "INSERT INTO projects (id, name, api_key, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, name, api_key, Utc::now().to_rfc3339()],
            )?;
            Ok(Project {
                id,
                name: name.to_owned(),
                api_key,
            })
        })
    }

    /// All projects, oldest first.
    pub fn projects(&self) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, name, api_key FROM projects ORDER BY created_at")?;
            let rows = st.query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    api_key: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The project with this capture key.
    pub fn project_by_key(&self, key: &str) -> Result<Option<Project>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT id, name, api_key FROM projects WHERE api_key = ?1",
                [key],
                |r| {
                    Ok(Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        api_key: r.get(2)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// The project with this id.
    pub fn project(&self, id: &str) -> Result<Option<Project>> {
        Ok(self.projects()?.into_iter().find(|p| p.id == id))
    }

    /// Ensure at least one project exists; returns the first one.
    pub fn ensure_default_project(&self, key: Option<&str>) -> Result<Project> {
        match self.projects()?.into_iter().next() {
            Some(p) => Ok(p),
            None => self.create_project("default", key),
        }
    }

    /// Record an identify: upsert the person and, when the role changed,
    /// append to the role history (D-009).
    pub fn identify(
        &self,
        project: &str,
        distinct_id: &str,
        role: Option<&str>,
        account: Option<&str>,
        plan: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let at = at.to_rfc3339();
        self.with(|c| {
            let previous: Option<Option<String>> = c
                .query_row(
                    "SELECT role FROM persons WHERE project = ?1 AND distinct_id = ?2",
                    params![project, distinct_id],
                    |r| r.get(0),
                )
                .optional()?;
            c.execute(
                "INSERT INTO persons (project, distinct_id, role, account, plan, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT (project, distinct_id) DO UPDATE SET
                   role = COALESCE(excluded.role, role),
                   account = COALESCE(excluded.account, account),
                   plan = COALESCE(excluded.plan, plan),
                   last_seen = excluded.last_seen",
                params![project, distinct_id, role, account, plan, at],
            )?;
            let changed = match (&previous, role) {
                (_, None) => false,
                (Some(Some(old)), Some(new)) => old != new,
                _ => true,
            };
            if let (true, Some(r)) = (changed, role) {
                c.execute(
                    "INSERT INTO role_history (project, distinct_id, role, since) VALUES (?1, ?2, ?3, ?4)",
                    params![project, distinct_id, r, at],
                )?;
            }
            Ok(())
        })
    }

    /// A person by id.
    pub fn person(&self, project: &str, distinct_id: &str) -> Result<Option<Person>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT distinct_id, role, account, plan, first_seen, last_seen FROM persons
                 WHERE project = ?1 AND distinct_id = ?2",
                params![project, distinct_id],
                |r| {
                    Ok(Person {
                        distinct_id: r.get(0)?,
                        role: r.get(1)?,
                        account: r.get(2)?,
                        plan: r.get(3)?,
                        first_seen: r.get(4)?,
                        last_seen: r.get(5)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Role history of a person, oldest first: `(role, since)`.
    pub fn role_history(&self, project: &str, distinct_id: &str) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT role, since FROM role_history WHERE project = ?1 AND distinct_id = ?2 ORDER BY since",
            )?;
            let rows = st.query_map(params![project, distinct_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Count of persons per raw role value in a project, most common first.
    pub fn role_counts(&self, project: &str) -> Result<Vec<(Option<String>, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT role, COUNT(*) AS n FROM persons WHERE project = ?1 GROUP BY role ORDER BY n DESC",
            )?;
            let rows = st.query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}

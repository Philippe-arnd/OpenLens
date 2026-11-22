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
CREATE TABLE IF NOT EXISTS feature_maps (
  project TEXT NOT NULL,
  version INTEGER NOT NULL,
  yaml TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project, version)
);
CREATE TABLE IF NOT EXISTS imports (
  project TEXT NOT NULL,
  sha TEXT NOT NULL,
  rows INTEGER NOT NULL,
  imported_at TEXT NOT NULL,
  PRIMARY KEY (project, sha)
);
CREATE TABLE IF NOT EXISTS flags (
  project TEXT NOT NULL,
  key TEXT NOT NULL,
  json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project, key)
);
CREATE TABLE IF NOT EXISTS login_links (
  hash TEXT PRIMARY KEY,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL CHECK (role IN ('admin', 'viewer')),
  projects TEXT,
  created_at TEXT NOT NULL
);
";

/// Additive schema changes for stores created by older versions.
fn migrate(c: &Connection) -> rusqlite::Result<()> {
    let has_user_col: bool = c
        .prepare("SELECT 1 FROM pragma_table_info('login_links') WHERE name = 'user_id'")?
        .exists([])?;
    if !has_user_col {
        c.execute_batch("ALTER TABLE login_links ADD COLUMN user_id TEXT")?;
    }
    Ok(())
}

/// A dashboard user (D-016). `admin` can change things (flags, users);
/// `viewer` can only look. `projects: None` means every project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct User {
    /// Stable id (derived from the email).
    pub id: String,
    /// Email, lowercased.
    pub email: String,
    /// `admin` or `viewer`.
    pub role: String,
    /// Projects this user may see, or `None` for all.
    pub projects: Option<Vec<String>>,
    /// When the user was invited.
    pub created_at: String,
}

fn user_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    let projects: Option<String> = r.get(3)?;
    Ok(User {
        id: r.get(0)?,
        email: r.get(1)?,
        role: r.get(2)?,
        projects: projects.map(|p| {
            p.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        }),
        created_at: r.get(4)?,
    })
}

/// Random lowercase alphanumeric string.
fn token(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

fn sha256_hex(s: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(s.as_bytes()))
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
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Write a consistent copy of the store to `dest` (which must not
    /// exist), safe while the server is running (`VACUUM INTO`).
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        let dest = dest.to_string_lossy().into_owned();
        self.with(|c| {
            c.execute("VACUUM INTO ?1", [dest])?;
            Ok(())
        })
    }

    /// In-memory store, for tests.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().map_err(|_| MetaError::Poisoned)?;
        f(&conn)
    }

    /// A stored setting, if set.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            Ok(
                c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                    r.get(0)
                })
                .optional()?,
            )
        })
    }

    /// Store a setting (insert or replace).
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
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

    /// Create a one-time login link token valid for `ttl_secs`. Only its
    /// SHA-256 is stored, so a leaked database does not leak live links.
    pub fn create_login_link(&self, ttl_secs: i64) -> Result<String> {
        self.create_login_link_for(None, ttl_secs)
    }

    /// A one-time login link for a user (`None`: the owner, i.e. admin of
    /// everything, as with the admin token).
    pub fn create_login_link_for(&self, user_id: Option<&str>, ttl_secs: i64) -> Result<String> {
        let token = format!("ll_{}", token(40));
        let expires = Utc::now().timestamp() + ttl_secs;
        self.with(|c| {
            c.execute(
                "DELETE FROM login_links WHERE expires_at < ?1",
                [Utc::now().timestamp()],
            )?;
            c.execute(
                "INSERT INTO login_links (hash, expires_at, user_id) VALUES (?1, ?2, ?3)",
                params![sha256_hex(&token), expires, user_id],
            )?;
            Ok(token)
        })
    }

    /// Use a login link once. `Some(None)`: an owner link; `Some(Some(id))`:
    /// a link for that user; `None`: invalid, used or expired.
    pub fn consume_login_link(&self, token: &str) -> Result<Option<Option<String>>> {
        if token.len() > 100 {
            return Ok(None);
        }
        self.with(|c| {
            let hash = sha256_hex(token);
            let row: Option<Option<String>> = c
                .query_row(
                    "SELECT user_id FROM login_links WHERE hash = ?1 AND expires_at >= ?2",
                    params![hash, Utc::now().timestamp()],
                    |r| r.get(0),
                )
                .optional()?;
            if row.is_some() {
                c.execute("DELETE FROM login_links WHERE hash = ?1", [&hash])?;
            }
            Ok(row)
        })
    }

    /// Invite or update a user. `role` is `admin` or `viewer`; `projects`
    /// limits what they see (`None`: all).
    pub fn upsert_user(
        &self,
        email: &str,
        role: &str,
        projects: Option<&[String]>,
    ) -> Result<User> {
        let email = email.trim().to_lowercase();
        if email.len() > 200 || !email.contains('@') || email.chars().any(char::is_whitespace) {
            return Err(MetaError::Invalid("invalid email"));
        }
        if role != "admin" && role != "viewer" {
            return Err(MetaError::Invalid("role must be admin or viewer"));
        }
        let projects = projects.map(|p| {
            p.iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(",")
        });
        let id = slug(&email.replace('@', "-at-"));
        self.with(|c| {
            c.execute(
                "INSERT INTO users (id, email, role, projects, created_at) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(email) DO UPDATE SET role = excluded.role, projects = excluded.projects",
                params![id, email, role, projects, Utc::now().to_rfc3339()],
            )?;
            Ok(c.query_row(
                "SELECT id, email, role, projects, created_at FROM users WHERE email = ?1",
                [&email],
                user_from_row,
            )?)
        })
    }

    /// One user by id.
    pub fn user(&self, id: &str) -> Result<Option<User>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT id, email, role, projects, created_at FROM users WHERE id = ?1",
                [id],
                user_from_row,
            )
            .optional()?)
        })
    }

    /// One user by email (case-insensitive).
    pub fn user_by_email(&self, email: &str) -> Result<Option<User>> {
        let email = email.trim().to_lowercase();
        self.with(|c| {
            Ok(c.query_row(
                "SELECT id, email, role, projects, created_at FROM users WHERE email = ?1",
                [&email],
                user_from_row,
            )
            .optional()?)
        })
    }

    /// Every user, by email.
    pub fn users(&self) -> Result<Vec<User>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, email, role, projects, created_at FROM users ORDER BY email",
            )?;
            let rows = st
                .query_map([], user_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Remove a user (their sessions stop working at once) and their links.
    pub fn delete_user(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute("DELETE FROM login_links WHERE user_id = ?1", [id])?;
            Ok(c.execute("DELETE FROM users WHERE id = ?1", [id])? == 1)
        })
    }

    /// Store a new version of a project's feature map; returns the version.
    /// An identical map is not stored again (same version returned).
    pub fn save_feature_map(&self, project: &str, yaml: &str) -> Result<i64> {
        self.with(|c| {
            let latest: Option<(i64, String)> = c
                .query_row(
                    "SELECT version, yaml FROM feature_maps WHERE project = ?1 ORDER BY version DESC LIMIT 1",
                    [project],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((v, y)) = &latest
                && y == yaml
            {
                return Ok(*v);
            }
            let version = latest.map_or(1, |(v, _)| v + 1);
            c.execute(
                "INSERT INTO feature_maps (project, version, yaml, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![project, version, yaml, Utc::now().to_rfc3339()],
            )?;
            Ok(version)
        })
    }

    /// Latest feature map of a project: `(version, yaml)`.
    pub fn feature_map(&self, project: &str) -> Result<Option<(i64, String)>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT version, yaml FROM feature_maps WHERE project = ?1 ORDER BY version DESC LIMIT 1",
                [project],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// Record an import chunk by content hash. Returns false when the same
    /// chunk was already imported (so re-running a backfill is a no-op).
    pub fn record_import(&self, project: &str, sha: &str, rows: i64) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO imports (project, sha, rows, imported_at) VALUES (?1, ?2, ?3, ?4)",
                params![project, sha, rows, Utc::now().to_rfc3339()],
            )?;
            Ok(n == 1)
        })
    }

    /// Create or replace a flag definition (stored as JSON).
    pub fn save_flag(&self, project: &str, key: &str, json: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO flags (project, key, json, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (project, key) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
                params![project, key, json, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// All flag definitions of a project (JSON), by key.
    pub fn flags(&self, project: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT json FROM flags WHERE project = ?1 ORDER BY key")?;
            let rows = st.query_map([project], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Delete a flag; true when it existed.
    pub fn delete_flag(&self, project: &str, key: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM flags WHERE project = ?1 AND key = ?2",
                params![project, key],
            )? == 1)
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

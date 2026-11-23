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
    /// IANA time zone the team works in (`Europe/Paris`); days and weeks of
    /// every KPI are counted in it (D-017). `UTC` until set.
    pub timezone: String,
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
  created_at TEXT NOT NULL,
  timezone TEXT NOT NULL DEFAULT 'UTC'
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
CREATE TABLE IF NOT EXISTS surveys (
  project TEXT NOT NULL,
  key TEXT NOT NULL,
  json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project, key)
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
CREATE TABLE IF NOT EXISTS replay_comments (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project TEXT NOT NULL,
  session TEXT NOT NULL,
  at_ms INTEGER NOT NULL,
  user_id TEXT NOT NULL,
  author TEXT NOT NULL,
  body TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS replay_comments_session ON replay_comments (project, session, at_ms);
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL CHECK (role IN ('admin', 'viewer')),
  projects TEXT,
  created_at TEXT NOT NULL,
  name TEXT,
  timezone TEXT,
  disabled INTEGER NOT NULL DEFAULT 0,
  last_sign_in TEXT
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
    let has_tz: bool = c
        .prepare("SELECT 1 FROM pragma_table_info('projects') WHERE name = 'timezone'")?
        .exists([])?;
    if !has_tz {
        c.execute_batch("ALTER TABLE projects ADD COLUMN timezone TEXT NOT NULL DEFAULT 'UTC'")?;
    }
    // Users: name, time zone, disabled, last sign-in (#215, #216).
    for (col, ddl) in [
        ("name", "ALTER TABLE users ADD COLUMN name TEXT"),
        ("timezone", "ALTER TABLE users ADD COLUMN timezone TEXT"),
        (
            "disabled",
            "ALTER TABLE users ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "last_sign_in",
            "ALTER TABLE users ADD COLUMN last_sign_in TEXT",
        ),
    ] {
        let has: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('users') WHERE name = ?1")?
            .exists([col])?;
        if !has {
            c.execute_batch(ddl)?;
        }
    }
    Ok(())
}

const USER_SELECT: &str = "SELECT id, email, role, projects, created_at, name, timezone, disabled, last_sign_in FROM users";

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
    /// Display name, if given.
    pub name: Option<String>,
    /// IANA time zone the user reads times in (#215); `None`: the project's.
    pub timezone: Option<String>,
    /// Sign-in refused (sessions stop at once) while true.
    pub disabled: bool,
    /// Last successful sign-in (RFC 3339), if any.
    pub last_sign_in: Option<String>,
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
        name: r.get(5)?,
        timezone: r.get(6)?,
        disabled: r.get::<_, i64>(7)? != 0,
        last_sign_in: r.get(8)?,
    })
}

/// A comment pinned to a moment of a replay (#213).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReplayComment {
    /// Id (for links: `#c<id>`).
    pub id: i64,
    /// Session id.
    pub session: String,
    /// Position in the recording, milliseconds from its start.
    pub at_ms: i64,
    /// `owner` or the user's id.
    pub user_id: String,
    /// Who wrote it, as shown (email or "owner"; kept if the user leaves).
    pub author: String,
    /// Plain text, at most 2000 characters.
    pub body: String,
    /// When it was written (RFC 3339).
    pub created_at: String,
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
                timezone: "UTC".to_owned(),
            })
        })
    }

    /// All projects, oldest first.
    pub fn projects(&self) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st =
                c.prepare("SELECT id, name, api_key, timezone FROM projects ORDER BY created_at")?;
            let rows = st.query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    api_key: r.get(2)?,
                    timezone: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The project with this capture key.
    pub fn project_by_key(&self, key: &str) -> Result<Option<Project>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT id, name, api_key, timezone FROM projects WHERE api_key = ?1",
                [key],
                |r| {
                    Ok(Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        api_key: r.get(2)?,
                        timezone: r.get(3)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Set the time zone of a project (an IANA name, checked by the
    /// caller). Returns false when the project does not exist.
    pub fn set_project_timezone(&self, id: &str, timezone: &str) -> Result<bool> {
        if timezone.is_empty()
            || timezone.len() > 64
            || !timezone
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
        {
            return Err(MetaError::Invalid(
                "time zone must be an IANA name like Europe/Paris",
            ));
        }
        self.with(|c| {
            Ok(c.execute(
                "UPDATE projects SET timezone = ?2 WHERE id = ?1",
                params![id, timezone],
            )? > 0)
        })
    }

    /// Rename a project (its id, and so its storage, stays). False when it
    /// does not exist.
    pub fn rename_project(&self, id: &str, name: &str) -> Result<bool> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(MetaError::Invalid("project name must be 1-80 characters"));
        }
        self.with(|c| {
            Ok(c.execute(
                "UPDATE projects SET name = ?2 WHERE id = ?1",
                params![id, name],
            )? > 0)
        })
    }

    /// Give a project a new capture key; the old one stops working at once.
    /// Returns the new key, or None when the project does not exist.
    pub fn rotate_project_key(&self, id: &str) -> Result<Option<String>> {
        let key = format!("pk_{}", token(24));
        self.with(|c| {
            Ok((c.execute(
                "UPDATE projects SET api_key = ?2 WHERE id = ?1",
                params![id, key],
            )? > 0)
                .then_some(key))
        })
    }

    /// A per-project setting (`app_url`, `icon`, …), stored as
    /// `project:<id>:<name>` so older releases ignore it.
    pub fn project_setting(&self, id: &str, name: &str) -> Result<Option<String>> {
        self.setting(&format!("project:{id}:{name}"))
    }

    /// Set a per-project setting; an empty value removes it.
    pub fn set_project_setting(&self, id: &str, name: &str, value: &str) -> Result<()> {
        let key = format!("project:{id}:{name}");
        if value.is_empty() {
            return self.with(|c| {
                c.execute("DELETE FROM settings WHERE key = ?1", [key])?;
                Ok(())
            });
        }
        self.set_setting(&key, value)
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

    /// Delete a project's metadata: the project, its key, people and role
    /// history, feature maps, imports, flags and surveys. Returns false when
    /// it did not exist. (Its event, replay and source-map files are the
    /// caller's to remove.)
    pub fn delete_project(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            for table in [
                "persons",
                "role_history",
                "feature_maps",
                "imports",
                "flags",
                "surveys",
                "replay_comments",
            ] {
                tx.execute(&format!("DELETE FROM {table} WHERE project = ?1"), [id])?;
            }
            tx.execute(
                "DELETE FROM settings WHERE substr(key, 1, length(?1)) = ?1",
                [format!("project:{id}:")],
            )?;
            let n = tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
            tx.commit()?;
            Ok(n > 0)
        })
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
                "SELECT id, email, role, projects, created_at, name, timezone, disabled, last_sign_in FROM users WHERE email = ?1",
                [&email],
                user_from_row,
            )?)
        })
    }

    /// One user by id.
    pub fn user(&self, id: &str) -> Result<Option<User>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT id, email, role, projects, created_at, name, timezone, disabled, last_sign_in FROM users WHERE id = ?1",
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
                "SELECT id, email, role, projects, created_at, name, timezone, disabled, last_sign_in FROM users WHERE email = ?1",
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
                "SELECT id, email, role, projects, created_at, name, timezone, disabled, last_sign_in FROM users ORDER BY email",
            )?;
            let rows = st
                .query_map([], user_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Pin a comment to a moment of a replay. The body is plain text,
    /// 1 to 2000 characters.
    pub fn add_replay_comment(
        &self,
        project: &str,
        session: &str,
        at_ms: i64,
        user_id: &str,
        author: &str,
        body: &str,
    ) -> Result<ReplayComment> {
        let body = body.trim();
        if body.is_empty() || body.chars().count() > 2000 {
            return Err(MetaError::Invalid("a comment is 1 to 2000 characters"));
        }
        if !(0..=7 * 24 * 3600 * 1000).contains(&at_ms) {
            return Err(MetaError::Invalid("the time is outside the recording"));
        }
        let now = Utc::now().to_rfc3339();
        self.with(|c| {
            c.execute(
                "INSERT INTO replay_comments (project, session, at_ms, user_id, author, body, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![project, session, at_ms, user_id, author, body, now],
            )?;
            Ok(ReplayComment {
                id: c.last_insert_rowid(),
                session: session.to_owned(),
                at_ms,
                user_id: user_id.to_owned(),
                author: author.to_owned(),
                body: body.to_owned(),
                created_at: now,
            })
        })
    }

    /// A replay's comments, in playback order.
    pub fn replay_comments(&self, project: &str, session: &str) -> Result<Vec<ReplayComment>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, session, at_ms, user_id, author, body, created_at FROM replay_comments \
                 WHERE project = ?1 AND session = ?2 ORDER BY at_ms, id",
            )?;
            let rows = st
                .query_map(params![project, session], |r| {
                    Ok(ReplayComment {
                        id: r.get(0)?,
                        session: r.get(1)?,
                        at_ms: r.get(2)?,
                        user_id: r.get(3)?,
                        author: r.get(4)?,
                        body: r.get(5)?,
                        created_at: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Comments per session of a project.
    pub fn replay_comment_counts(&self, project: &str) -> Result<Vec<(String, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT session, count(*) FROM replay_comments WHERE project = ?1 GROUP BY session",
            )?;
            let rows = st
                .query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Delete a comment if `user_id` wrote it or `admin`. Returns false when
    /// there was nothing they may delete.
    pub fn delete_replay_comment(
        &self,
        project: &str,
        id: i64,
        user_id: &str,
        admin: bool,
    ) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM replay_comments WHERE project = ?1 AND id = ?2 AND (?4 OR user_id = ?3)",
                params![project, id, user_id, admin],
            )? == 1)
        })
    }

    /// Would removing admin rights from `id` leave no enabled admin among
    /// the users? (The owner, holding the admin token, always remains.)
    fn last_admin(c: &Connection, id: &str) -> Result<bool> {
        let me: Option<(String, i64)> = c
            .query_row(
                "SELECT role, disabled FROM users WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if !matches!(me, Some((ref r, 0)) if r == "admin") {
            return Ok(false);
        }
        let others: i64 = c.query_row(
            "SELECT count(*) FROM users WHERE role = 'admin' AND disabled = 0 AND id <> ?1",
            [id],
            |r| r.get(0),
        )?;
        Ok(others == 0)
    }

    /// Change a user's name, email, role and projects (the id stays, so
    /// sessions survive an email change). Refuses to demote the last admin.
    pub fn update_user(
        &self,
        id: &str,
        name: Option<&str>,
        email: &str,
        role: &str,
        projects: Option<&[String]>,
    ) -> Result<Option<User>> {
        let email = email.trim().to_lowercase();
        if email.len() > 200 || !email.contains('@') || email.chars().any(char::is_whitespace) {
            return Err(MetaError::Invalid("invalid email"));
        }
        if role != "admin" && role != "viewer" {
            return Err(MetaError::Invalid("role must be admin or viewer"));
        }
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if name.is_some_and(|n| n.chars().count() > 80) {
            return Err(MetaError::Invalid("name must be at most 80 characters"));
        }
        let projects = projects.map(|p| {
            p.iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(",")
        });
        self.with(|c| {
            if role != "admin" && Self::last_admin(c, id)? {
                return Err(MetaError::Invalid("the last admin must stay an admin"));
            }
            let n = c
                .execute(
                    "UPDATE users SET name = ?2, email = ?3, role = ?4, projects = ?5 WHERE id = ?1",
                    params![id, name, email, role, projects],
                )
                .map_err(|e| match e {
                    rusqlite::Error::SqliteFailure(f, _)
                        if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        MetaError::Invalid("another user has that email")
                    }
                    e => e.into(),
                })?;
            if n == 0 {
                return Ok(None);
            }
            Ok(c.query_row(&format!("{} WHERE id = ?1", USER_SELECT), [id], user_from_row)
                .optional()?)
        })
    }

    /// Disable or enable sign-in. Disabling ends the user's sessions at
    /// once and drops their pending links. Refuses the last admin.
    pub fn set_user_disabled(&self, id: &str, disabled: bool) -> Result<bool> {
        self.with(|c| {
            if disabled && Self::last_admin(c, id)? {
                return Err(MetaError::Invalid("the last admin cannot be disabled"));
            }
            if disabled {
                c.execute("DELETE FROM login_links WHERE user_id = ?1", [id])?;
            }
            Ok(c.execute(
                "UPDATE users SET disabled = ?2 WHERE id = ?1",
                params![id, i64::from(disabled)],
            )? == 1)
        })
    }

    /// Set (or clear, with `None`) the time zone a user reads times in.
    pub fn set_user_timezone(&self, id: &str, timezone: Option<&str>) -> Result<bool> {
        if let Some(tz) = timezone
            && (tz.is_empty()
                || tz.len() > 64
                || !tz
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b)))
        {
            return Err(MetaError::Invalid(
                "time zone must be an IANA name like Europe/Paris",
            ));
        }
        self.with(|c| {
            Ok(c.execute(
                "UPDATE users SET timezone = ?2 WHERE id = ?1",
                params![id, timezone],
            )? == 1)
        })
    }

    /// Record a successful sign-in.
    pub fn touch_sign_in(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE users SET last_sign_in = ?2 WHERE id = ?1",
                params![id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// When the user's newest unused login link expires (Unix seconds), if
    /// one is pending: an invitation still open.
    pub fn pending_link(&self, id: &str) -> Result<Option<i64>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT max(expires_at) FROM login_links WHERE user_id = ?1 AND expires_at >= ?2",
                params![id, Utc::now().timestamp()],
                |r| r.get::<_, Option<i64>>(0),
            )?)
        })
    }

    /// Drop the user's pending login links (revoke an invitation).
    pub fn revoke_links(&self, id: &str) -> Result<usize> {
        self.with(|c| Ok(c.execute("DELETE FROM login_links WHERE user_id = ?1", [id])?))
    }

    /// Remove a user (their sessions stop working at once) and their links.
    /// Refuses the last admin.
    pub fn delete_user(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            if Self::last_admin(c, id)? {
                return Err(MetaError::Invalid("the last admin cannot be removed"));
            }
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

    /// Create or replace a survey definition (stored as JSON).
    pub fn save_survey(&self, project: &str, key: &str, json: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO surveys (project, key, json, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (project, key) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
                params![project, key, json, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// All survey definitions of a project (JSON), by key.
    pub fn surveys(&self, project: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT json FROM surveys WHERE project = ?1 ORDER BY key")?;
            let rows = st.query_map([project], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Delete a survey; true when it existed.
    pub fn delete_survey(&self, project: &str, key: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM surveys WHERE project = ?1 AND key = ?2",
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

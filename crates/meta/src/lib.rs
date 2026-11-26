//! Metadata store (SQLite): projects, API keys, persons and role history.
//!
//! Small, transactional and rarely written, so it lives in one SQLite file
//! next to the event segments. Events themselves never go here.
//!
//! Layer: L1 (see `docs/architecture.md`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

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

mod access;
mod collab;
mod config;
mod forget;
/// Read-only connections next to the writer (#344).
mod projects;
mod totp;
mod uptime;
mod users;
pub use totp::Totp;

const READERS: usize = 4;

/// The metadata store.
pub struct Meta {
    conn: Mutex<Connection>,
    /// Read-only connections (#344): WAL lets them read while the one
    /// writer writes, so dashboard reads stop queueing behind each other
    /// and behind writes. Empty for [`Meta::in_memory`]: reads use `conn`.
    readers: Vec<Mutex<Connection>>,
    next_reader: std::sync::atomic::AtomicUsize,
    /// Per project, the people forgotten (#295): capture checks every event
    /// against it, so it is read once and kept until the next `forget`.
    forgotten: Mutex<HashMap<String, Arc<HashSet<String>>>>,
    /// Bumped by every forget, so a set read before it is not cached after
    /// it (review of v0.3.0).
    forgotten_gen: std::sync::atomic::AtomicU64,
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
CREATE TABLE IF NOT EXISTS api_tokens (
  id TEXT PRIMARY KEY,
  hash TEXT NOT NULL UNIQUE,
  scope TEXT NOT NULL,
  projects TEXT,
  label TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_used TEXT
);
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
CREATE TABLE IF NOT EXISTS uptime_checks (
  project TEXT NOT NULL,
  at INTEGER NOT NULL,
  ok INTEGER NOT NULL,
  status INTEGER,
  ms INTEGER,
  error TEXT
);
CREATE INDEX IF NOT EXISTS uptime_checks_project ON uptime_checks (project, at);
CREATE TABLE IF NOT EXISTS incidents (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project TEXT NOT NULL,
  rule TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  cause TEXT NOT NULL,
  notified_at INTEGER
);
CREATE INDEX IF NOT EXISTS incidents_project ON incidents (project, started_at);
CREATE TABLE IF NOT EXISTS forgotten (
  project TEXT NOT NULL,
  distinct_id TEXT NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY (project, distinct_id)
);
CREATE TABLE IF NOT EXISTS audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL,
  actor TEXT NOT NULL,
  project TEXT,
  action TEXT NOT NULL,
  detail TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS share_links (
  hash TEXT PRIMARY KEY,
  id TEXT NOT NULL UNIQUE,
  project TEXT NOT NULL,
  scope TEXT NOT NULL,
  environment TEXT NOT NULL,
  release TEXT,
  embed TEXT,
  expires_at INTEGER,
  created_at INTEGER NOT NULL,
  created_by TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS notes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project TEXT NOT NULL,
  day TEXT NOT NULL,
  author TEXT NOT NULL,
  author_id TEXT NOT NULL,
  body TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_project ON notes (project, day);
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
    // Feature map versions: who saved them and why (#240).
    for (col, ddl) in [
        ("author", "ALTER TABLE feature_maps ADD COLUMN author TEXT"),
        (
            "message",
            "ALTER TABLE feature_maps ADD COLUMN message TEXT",
        ),
    ] {
        let has: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('feature_maps') WHERE name = ?1")?
            .exists([col])?;
        if !has {
            c.execute_batch(ddl)?;
        }
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
        // SEC-07: the (issuer, subject) an OIDC sign-in bound to this user
        // on their first sign-in; required to match on every one after.
        ("oidc_iss", "ALTER TABLE users ADD COLUMN oidc_iss TEXT"),
        ("oidc_sub", "ALTER TABLE users ADD COLUMN oidc_sub TEXT"),
        // #428: an Argon2id hash (PHC string) and when it was set (unix
        // seconds): sessions issued before then stop working.
        (
            "password_hash",
            "ALTER TABLE users ADD COLUMN password_hash TEXT",
        ),
        (
            "password_at",
            "ALTER TABLE users ADD COLUMN password_at INTEGER",
        ),
        // #438: the user's theme (`light`, `dark`; NULL follows the
        // system) and whether the sidebar starts folded, across browsers.
        ("theme", "ALTER TABLE users ADD COLUMN theme TEXT"),
        (
            "nav_folded",
            "ALTER TABLE users ADD COLUMN nav_folded INTEGER NOT NULL DEFAULT 0",
        ),
        // #440: the second factor. The secret is sealed by the server
        // (never stored in clear); `totp_at` is set once enrolment is
        // confirmed, `totp_step` is the last 30 s step used (a replay is
        // refused), and `totp_recovery` holds the SHA-256 hashes of the
        // unused recovery codes, comma-separated.
        (
            "totp_secret",
            "ALTER TABLE users ADD COLUMN totp_secret TEXT",
        ),
        ("totp_at", "ALTER TABLE users ADD COLUMN totp_at INTEGER"),
        (
            "totp_step",
            "ALTER TABLE users ADD COLUMN totp_step INTEGER",
        ),
        (
            "totp_recovery",
            "ALTER TABLE users ADD COLUMN totp_recovery TEXT",
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

/// One stored version of a feature map.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MapVersion {
    /// Version number.
    pub version: i64,
    /// When it was saved (RFC 3339).
    pub created_at: String,
    /// Size of the YAML.
    pub bytes: i64,
    /// Who saved it (an email, `owner` or `api (admin token)`).
    pub author: Option<String>,
    /// Why, as given with the push (`--message`).
    pub message: Option<String>,
}

const USER_SELECT: &str = "SELECT id, email, role, projects, created_at, name, timezone, disabled, last_sign_in, oidc_iss, oidc_sub FROM users";

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
    /// The OIDC issuer this user first signed in with (SEC-07), if any.
    pub oidc_iss: Option<String>,
    /// The OIDC subject this user first signed in as (SEC-07), if any.
    pub oidc_sub: Option<String>,
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
        oidc_iss: r.get(9)?,
        oidc_sub: r.get(10)?,
    })
}

/// A read-only share link (#229). The token itself is never stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShareLink {
    /// Public id (to list and revoke).
    pub id: String,
    /// Project.
    pub project: String,
    /// `board` or `digest`.
    pub scope: String,
    /// Environment shown.
    pub environment: String,
    /// Release shown, if fixed.
    pub release: Option<String>,
    /// Origins allowed to embed it (space-separated), if any.
    pub embed: Option<String>,
    /// Expiry (Unix seconds), if any.
    pub expires_at: Option<i64>,
    /// Created (Unix seconds).
    pub created_at: i64,
    /// Who made it.
    pub created_by: String,
}

fn share_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ShareLink> {
    Ok(ShareLink {
        id: r.get(0)?,
        project: r.get(1)?,
        scope: r.get(2)?,
        environment: r.get(3)?,
        release: r.get(4)?,
        embed: r.get(5)?,
        expires_at: r.get(6)?,
        created_at: r.get(7)?,
        created_by: r.get(8)?,
    })
}

const SHARE_COLS: &str =
    "id, project, scope, environment, release, embed, expires_at, created_at, created_by";

/// A timeline note: `(id, day, author, author_id, body)` (#267).
pub type Note = (i64, String, String, String, String);

/// One audit entry: `(at, actor, project, action, detail)` (#230).
pub type AuditEntry = (i64, String, Option<String>, String, String);

/// One uptime probe: `(at, ok, status, ms, error)` (#256).
pub type Check = (i64, bool, Option<i64>, Option<i64>, Option<String>);

/// One incident: `(rule, started_at, ended_at, cause, notified)` (#256).
pub type Incident = (String, i64, Option<i64>, String, bool);

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

/// An API token with a scope , as listed: never its value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ApiToken {
    /// Stable id, used to revoke it.
    pub id: String,
    /// `read`, `write` or `admin`.
    pub scope: String,
    /// The projects it may reach; `None`: every project.
    pub projects: Option<Vec<String>>,
    /// What it is for.
    pub label: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339, when it was last used.
    pub last_used: Option<String>,
}

fn api_token_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ApiToken> {
    let projects: Option<String> = r.get(2)?;
    Ok(ApiToken {
        id: r.get(0)?,
        scope: r.get(1)?,
        projects: projects.map(|p| p.split(',').map(str::to_owned).collect()),
        label: r.get(3)?,
        created_at: r.get(4)?,
        last_used: r.get(5)?,
    })
}

/// Prefix of a hashed secret in `settings`.
const HASHED: &str = "sha256:";

/// SHA-256 of a token, in hex: how tokens are stored and compared.
pub fn sha256_hex(s: &str) -> String {
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
        let readers = (0..READERS)
            .map(|_| {
                let r = Connection::open_with_flags(
                    path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                r.busy_timeout(std::time::Duration::from_secs(5))?;
                Ok(Mutex::new(r))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            conn: Mutex::new(conn),
            readers,
            next_reader: Default::default(),
            forgotten: Mutex::default(),
            forgotten_gen: std::sync::atomic::AtomicU64::new(0),
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
            readers: Vec::new(),
            next_reader: Default::default(),
            forgotten: Mutex::default(),
            forgotten_gen: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().map_err(|_| MetaError::Poisoned)?;
        f(&conn)
    }

    /// Run a read on a free reader connection (any free one, else wait for
    /// one in turn); on the writer when there are none (in memory).
    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let n = self.readers.len();
        if n == 0 {
            return self.with(f);
        }
        let first = self
            .next_reader
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        for i in 0..n {
            if let Ok(c) = self.readers[(first + i) % n].try_lock() {
                return f(&c);
            }
        }
        let c = self.readers[first % n]
            .lock()
            .map_err(|_| MetaError::Poisoned)?;
        f(&c)
    }

    /// A stored setting, if set.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.read(|c| {
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

    /// The SHA-256 (hex) of the admin token, and the token itself when this
    /// call generated it (the only time it exists in clear: show it once).
    /// The store keeps only the hash; a token stored in clear by an older
    /// release is hashed in place.
    pub fn admin_token_hash(&self) -> Result<(String, Option<String>)> {
        const KEY: &str = "admin_token";
        match self.setting(KEY)? {
            Some(v) if v.starts_with(HASHED) => Ok((v[HASHED.len()..].to_owned(), None)),
            // Stored in clear by older releases: hash it in place.
            Some(plain) => {
                let h = sha256_hex(&plain);
                self.set_setting(KEY, &format!("{HASHED}{h}"))?;
                Ok((h, None))
            }
            None => {
                let plain = format!("adm_{}", token(32));
                let h = sha256_hex(&plain);
                self.set_setting(KEY, &format!("{HASHED}{h}"))?;
                Ok((h, Some(plain)))
            }
        }
    }
}

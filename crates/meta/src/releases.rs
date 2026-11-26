//! Automatic release names per build fingerprint (#495, D-044).
//!
//! The browser SDK sends `$build`, a short hash of the page's own script and
//! stylesheet URLs. An event with no explicit release gets the name its
//! fingerprint was given the first time it was seen in that project and
//! environment: `YYYY-MM-DD.N` (the project's day, N the order that day), or
//! the explicit release later sent with the same fingerprint.

use super::*;

/// Fingerprints kept per project; past it the oldest go.
pub const MAX_BUILDS: i64 = 500;

/// New fingerprints recorded per project and day; past it events of a new
/// fingerprint keep no release (bounds the writes a public key can cause).
pub const MAX_NEW_BUILDS_PER_DAY: i64 = 50;

/// A build fingerprint as recorded: where it was seen and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseBuild {
    /// Environment it was seen in.
    pub environment: String,
    /// The fingerprint.
    pub build: String,
    /// Its release name (automatic or explicit).
    pub name: String,
    /// When it was first seen (Unix seconds).
    pub first_seen: i64,
}

impl Meta {
    /// Every fingerprint of `project` (at most [`MAX_BUILDS`]), oldest
    /// first: what ties a release in one environment to the same build in
    /// another (#500).
    pub fn release_builds(&self, project: &str) -> Result<Vec<ReleaseBuild>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT environment, build, name, first_seen FROM release_builds \
                 WHERE project = ?1 ORDER BY first_seen, rowid LIMIT ?2",
            )?;
            let rows = st.query_map(params![project, MAX_BUILDS], |r| {
                Ok(ReleaseBuild {
                    environment: r.get(0)?,
                    build: r.get(1)?,
                    name: r.get(2)?,
                    first_seen: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The release name of a build fingerprint, if it was seen.
    pub fn build_release(
        &self,
        project: &str,
        environment: &str,
        build: &str,
    ) -> Result<Option<String>> {
        self.read(|c| lookup(c, project, environment, build))
    }

    /// Record a build fingerprint seen on `day` (the project's day,
    /// `YYYY-MM-DD`) at `at` (Unix seconds) and return its release name.
    ///
    /// With `explicit`, that release becomes the fingerprint's name, new or
    /// not (an alias: later events with no release get it). Without, a known
    /// fingerprint keeps its name and a new one is named `day.N`. `None` when
    /// a new fingerprint is refused because the project already recorded
    /// [`MAX_NEW_BUILDS_PER_DAY`] that day.
    pub fn record_build(
        &self,
        project: &str,
        environment: &str,
        build: &str,
        explicit: Option<&str>,
        day: &str,
        at: i64,
    ) -> Result<Option<String>> {
        if build.is_empty() || day.is_empty() {
            return Err(MetaError::Invalid("empty build or day"));
        }
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let known = lookup(&tx, project, environment, build)?;
            let name = match (known, explicit) {
                (Some(old), Some(new)) if old == new => Some(old),
                (Some(_), Some(new)) => {
                    tx.execute(
                        "UPDATE release_builds SET name = ?4 WHERE project = ?1 AND environment = ?2 AND build = ?3",
                        params![project, environment, build, new],
                    )?;
                    Some(new.to_owned())
                }
                (Some(old), None) => Some(old),
                (None, explicit) => insert(&tx, project, environment, build, explicit, day, at)?,
            };
            tx.commit()?;
            Ok(name)
        })
    }
}

fn lookup(c: &Connection, project: &str, environment: &str, build: &str) -> Result<Option<String>> {
    Ok(c.query_row(
        "SELECT name FROM release_builds WHERE project = ?1 AND environment = ?2 AND build = ?3",
        params![project, environment, build],
        |r| r.get(0),
    )
    .optional()?)
}

/// A new fingerprint: named `explicit` or `day.N`, then the project is cut
/// back to [`MAX_BUILDS`], oldest first.
fn insert(
    c: &Connection,
    project: &str,
    environment: &str,
    build: &str,
    explicit: Option<&str>,
    day: &str,
    at: i64,
) -> Result<Option<String>> {
    let today: i64 = c.query_row(
        "SELECT COUNT(*) FROM release_builds WHERE project = ?1 AND day = ?2",
        params![project, day],
        |r| r.get(0),
    )?;
    if today >= MAX_NEW_BUILDS_PER_DAY {
        return Ok(None);
    }
    // N counts the automatic names of that day in that environment.
    let seq: i64 = c.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM release_builds WHERE project = ?1 AND environment = ?2 AND day = ?3",
        params![project, environment, day],
        |r| r.get(0),
    )?;
    let (name, seq) = match explicit {
        Some(e) => (e.to_owned(), None),
        None => (format!("{day}.{seq}"), Some(seq)),
    };
    c.execute(
        "INSERT INTO release_builds (project, environment, build, name, day, seq, first_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project, environment, build, name, day, seq, at],
    )?;
    c.execute(
        "DELETE FROM release_builds WHERE project = ?1 AND rowid NOT IN \
         (SELECT rowid FROM release_builds WHERE project = ?1 ORDER BY first_seen DESC, rowid DESC LIMIT ?2)",
        params![project, MAX_BUILDS],
    )?;
    Ok(Some(name))
}

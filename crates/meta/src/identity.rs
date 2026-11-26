//! Identity kept at ingestion (#450, D-036): which anonymous id counts as
//! which user (`person_aliases`), and every identified person with a dense
//! integer id (`person_ids`). Recorded when a `$identify` is stored, so the
//! query layer reads two small tables instead of scanning every event.
//!
//! The rules are those the query layer applied to the files before:
//! - an anonymous id identified as several users counts as the smallest
//!   one, and an id is never an alias of itself;
//! - a person's `first_seen` is their earliest `$identify`, and `internal`
//!   once set stays set.
//!
//! Ids come from a per-project counter and are never reused, not even after
//! an erasure, so a stored set of ids never points at someone else.

use super::*;

/// One identified person, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonIdentity {
    /// The user id.
    pub person: String,
    /// Dense per-project id, from 1, never reused.
    pub id: u32,
    /// Earliest `$identify`, in microseconds since the Unix epoch (UTC).
    pub first_seen: i64,
    /// Marked as the team's own.
    pub internal: bool,
}

/// A project's identity tables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityTables {
    /// `(anon, person)`, sorted by anon.
    pub aliases: Vec<(String, String)>,
    /// Every identified person, by id.
    pub persons: Vec<PersonIdentity>,
}

/// One `$identify` to record: `(person, anon, first_seen µs, internal)`.
pub type Identified = (String, Option<String>, i64, bool);

/// The project setting that says the one-time backfill ran.
const READY: &str = "identity_ready";
/// Rows written per transaction by a backfill, so capture is not held
/// behind one long write.
const CHUNK: usize = 2_000;

/// Record one `$identify`; true when it changed a table.
fn record_one(tx: &Connection, project: &str, row: &Identified) -> Result<bool> {
    let (person, anon, first_seen, internal) = row;
    let forgotten = |id: &str| -> Result<bool> {
        Ok(tx
            .prepare_cached("SELECT 1 FROM forgotten WHERE project = ?1 AND distinct_id = ?2")?
            .exists(params![project, id])?)
    };
    // Erased people are not brought back by a late write.
    if forgotten(person)? {
        return Ok(false);
    }
    let known = tx
        .prepare_cached("SELECT 1 FROM person_ids WHERE project = ?1 AND person = ?2")?
        .exists(params![project, person])?;
    let mut changed = if known {
        tx.prepare_cached(
            "UPDATE person_ids SET first_seen = min(first_seen, ?3), internal = max(internal, ?4)
             WHERE project = ?1 AND person = ?2 AND (first_seen > ?3 OR internal < ?4)",
        )?
        .execute(params![project, person, first_seen, i64::from(*internal)])?
            > 0
    } else {
        let id: i64 = tx
            .prepare_cached(
                "INSERT INTO person_id_seq (project, last) VALUES (?1, 1)
                 ON CONFLICT (project) DO UPDATE SET last = last + 1 RETURNING last",
            )?
            .query_row([project], |r| r.get(0))?;
        if u32::try_from(id).is_err() {
            return Err(MetaError::Invalid("too many people in this project"));
        }
        tx.prepare_cached(
            "INSERT INTO person_ids (project, person, id, first_seen, internal)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            project,
            person,
            id,
            first_seen,
            i64::from(*internal)
        ])?;
        true
    };
    if let Some(anon) = anon
        && anon != person
        && !forgotten(anon)?
    {
        changed |= tx
            .prepare_cached(
                "INSERT INTO person_aliases (project, anon, person) VALUES (?1, ?2, ?3)
                 ON CONFLICT (project, anon) DO UPDATE SET person = excluded.person
                 WHERE excluded.person < person_aliases.person",
            )?
            .execute(params![project, anon, person])?
            > 0;
    }
    Ok(changed)
}

impl Meta {
    /// Record `$identify` events of `project` (at capture and import).
    /// True when a table changed (a new person or alias, an earlier first
    /// visit, a person newly marked internal).
    pub fn record_identities(&self, project: &str, rows: &[Identified]) -> Result<bool> {
        if rows.is_empty() {
            return Ok(false);
        }
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let mut changed = false;
            for row in rows {
                changed |= record_one(&tx, project, row)?;
            }
            tx.commit()?;
            Ok(changed)
        })
    }

    /// Store what the files say about `project`'s identity, once, after an
    /// upgrade (merged with what capture recorded meanwhile), then mark the
    /// tables ready: from then on the query layer reads them.
    pub fn backfill_identities(
        &self,
        project: &str,
        persons: &[(String, i64, bool)],
        aliases: &[(String, String)],
    ) -> Result<()> {
        let mut ordered: Vec<&(String, i64, bool)> = persons.iter().collect();
        ordered.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
        let first: HashMap<&str, i64> = persons.iter().map(|p| (p.0.as_str(), p.1)).collect();
        let mut rows: Vec<Identified> = ordered
            .into_iter()
            .map(|(p, at, internal)| (p.clone(), None, *at, *internal))
            .collect();
        // An alias is recorded with its person (who is in `persons`: both
        // come from the same `$identify` rows).
        for (anon, person) in aliases {
            if let Some(at) = first.get(person.as_str()) {
                rows.push((person.clone(), Some(anon.clone()), *at, false));
            }
        }
        for chunk in rows.chunks(CHUNK) {
            self.record_identities(project, chunk)?;
        }
        self.set_project_setting(project, READY, &Utc::now().to_rfc3339())
    }

    /// The identity tables of `project`, or `None` until its backfill ran.
    pub fn identities(&self, project: &str) -> Result<Option<IdentityTables>> {
        if self.project_setting(project, READY)?.is_none() {
            return Ok(None);
        }
        self.read(|c| {
            let mut st = c.prepare_cached(
                "SELECT anon, person FROM person_aliases WHERE project = ?1 ORDER BY anon",
            )?;
            let aliases = st
                .query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut st = c.prepare_cached(
                "SELECT person, id, first_seen, internal FROM person_ids WHERE project = ?1 ORDER BY id",
            )?;
            let persons = st
                .query_map([project], |r| {
                    Ok(PersonIdentity {
                        person: r.get(0)?,
                        id: r.get(1)?,
                        first_seen: r.get(2)?,
                        internal: r.get::<_, i64>(3)? != 0,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(IdentityTables { aliases, persons }))
        })
    }

    /// The anonymous ids recorded as `person` (for erasure and export).
    pub fn aliases_of(&self, project: &str, person: &str) -> Result<Vec<String>> {
        self.read(|c| {
            let mut st = c.prepare_cached(
                "SELECT anon FROM person_aliases WHERE project = ?1 AND person = ?2 ORDER BY anon",
            )?;
            let rows = st
                .query_map(params![project, person], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(rows)
        })
    }
}

/// Drop the identity rows of `ids` (erasure): their person rows and every
/// alias from or to them. Runs inside [`Meta::forget`]'s transaction.
pub(crate) fn forget_in(tx: &Connection, project: &str, id: &str) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM person_ids WHERE project = ?1 AND person = ?2",
        params![project, id],
    )?;
    tx.execute(
        "DELETE FROM person_aliases WHERE project = ?1 AND (anon = ?2 OR person = ?2)",
        params![project, id],
    )?;
    Ok(())
}

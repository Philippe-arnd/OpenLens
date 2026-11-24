//! Uptime checks and incidents. Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
    /// Record one uptime probe (#256). Checks older than 7 days are dropped.
    pub fn record_check(
        &self,
        project: &str,
        at: i64,
        ok: bool,
        status: Option<u16>,
        ms: Option<i64>,
        error: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO uptime_checks (project, at, ok, status, ms, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![project, at, i64::from(ok), status, ms, error],
            )?;
            c.execute(
                "DELETE FROM uptime_checks WHERE project = ?1 AND at < ?2",
                params![project, at - 7 * 24 * 3600],
            )?;
            Ok(())
        })
    }

    /// Probes of a project since `since` (Unix seconds), oldest first:
    /// `(at, ok, status, ms, error)`.
    pub fn checks(&self, project: &str, since: i64) -> Result<Vec<Check>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT at, ok, status, ms, error FROM uptime_checks WHERE project = ?1 AND at >= ?2 ORDER BY at",
            )?;
            let rows = st
                .query_map(params![project, since], |r| {
                    Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The open incident of a rule, if any: `(id, started_at, notified)`.
    pub fn open_incident(&self, project: &str, rule: &str) -> Result<Option<(i64, i64, bool)>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT id, started_at, notified_at IS NOT NULL FROM incidents WHERE project = ?1 AND rule = ?2 AND ended_at IS NULL",
                params![project, rule],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
        })
    }

    /// Open an incident.
    pub fn start_incident(&self, project: &str, rule: &str, at: i64, cause: &str) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO incidents (project, rule, started_at, cause) VALUES (?1, ?2, ?3, ?4)",
                params![project, rule, at, cause],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// When an incident was last announced (Unix seconds), if it was.
    pub fn incident_last_notified(&self, id: i64) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT notified_at FROM incidents WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
        })
    }

    /// Mark an incident as announced.
    pub fn incident_notified(&self, id: i64, at: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE incidents SET notified_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
            Ok(())
        })
    }

    /// Close an incident.
    pub fn end_incident(&self, id: i64, at: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE incidents SET ended_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
            Ok(())
        })
    }

    /// Incidents started since `since`, newest first:
    /// `(rule, started_at, ended_at, cause, notified)`.
    pub fn incidents(&self, project: &str, since: i64) -> Result<Vec<Incident>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT rule, started_at, ended_at, cause, notified_at IS NOT NULL FROM incidents \
                 WHERE project = ?1 AND (started_at >= ?2 OR ended_at IS NULL) ORDER BY started_at DESC LIMIT 100",
            )?;
            let rows = st
                .query_map(params![project, since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}

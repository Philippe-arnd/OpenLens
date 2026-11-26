//! Uptime checks and incidents. Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
    /// Record one uptime probe of production (#256). Checks older than 7
    /// days are dropped, except those kept around an incident (#559).
    pub fn record_check(
        &self,
        project: &str,
        at: i64,
        ok: bool,
        status: Option<u16>,
        ms: Option<i64>,
        error: Option<&str>,
    ) -> Result<()> {
        let probe = Probe {
            ok,
            status,
            ms,
            error,
        };
        self.record_check_in(project, uptime_history::PRODUCTION, at, &probe)
    }

    /// Probes of production since `since` (Unix seconds), oldest first:
    /// `(at, ok, status, ms, error)`.
    pub fn checks(&self, project: &str, since: i64) -> Result<Vec<Check>> {
        self.checks_in(project, uptime_history::PRODUCTION, since)
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

    /// Open an incident; the checks of the hour before an availability
    /// incident are kept with it (#559).
    pub fn start_incident(&self, project: &str, rule: &str, at: i64, cause: &str) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO incidents (project, rule, started_at, cause) VALUES (?1, ?2, ?3, ?4)",
                params![project, rule, at, cause],
            )?;
            let id = c.last_insert_rowid();
            // The hour of checks before it is kept with it (#559).
            uptime_history::mark_incident(c, id)?;
            Ok(id)
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
            uptime_history::mark_incident(c, id)?;
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

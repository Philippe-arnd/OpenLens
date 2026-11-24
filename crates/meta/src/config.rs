//! Feature maps, flags, surveys and role counts. Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
    /// Store a new version of a project's feature map; returns the version.
    /// An identical map is not stored again (same version returned).
    pub fn save_feature_map(&self, project: &str, yaml: &str) -> Result<i64> {
        self.save_feature_map_by(project, yaml, None, None)
    }

    /// [`Self::save_feature_map`], recording who saved it and why (#240).
    pub fn save_feature_map_by(
        &self,
        project: &str,
        yaml: &str,
        author: Option<&str>,
        message: Option<&str>,
    ) -> Result<i64> {
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
                "INSERT INTO feature_maps (project, version, yaml, created_at, author, message) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    project,
                    version,
                    yaml,
                    Utc::now().to_rfc3339(),
                    author,
                    message.map(|m| m.chars().take(500).collect::<String>())
                ],
            )?;
            Ok(version)
        })
    }

    /// Latest feature map of a project: `(version, yaml)`.
    pub fn feature_map(&self, project: &str) -> Result<Option<(i64, String)>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT version, yaml FROM feature_maps WHERE project = ?1 ORDER BY version DESC LIMIT 1",
                [project],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// Every stored version of a project's map, newest first (#221, #240).
    pub fn feature_map_versions(&self, project: &str) -> Result<Vec<MapVersion>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT version, created_at, length(yaml), author, message FROM feature_maps WHERE project = ?1 ORDER BY version DESC LIMIT 50",
            )?;
            let rows = st
                .query_map([project], |r| {
                    Ok(MapVersion {
                        version: r.get(0)?,
                        created_at: r.get(1)?,
                        bytes: r.get(2)?,
                        author: r.get(3)?,
                        message: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One stored version of a project's map.
    pub fn feature_map_version(&self, project: &str, version: i64) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT yaml FROM feature_maps WHERE project = ?1 AND version = ?2",
                params![project, version],
                |r| r.get(0),
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
        self.read(|c| {
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
        self.read(|c| {
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
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT role, COUNT(*) AS n FROM persons WHERE project = ?1 GROUP BY role ORDER BY n DESC",
            )?;
            let rows = st.query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}

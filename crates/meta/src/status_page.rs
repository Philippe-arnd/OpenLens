//! The public status page's settings (#561, D-079): one row per project
//! that ever chose a slug, with the slug (unique across projects, kept
//! while the page is off so nobody else takes it), whether the page is on,
//! and its components as JSON. The checks on the values are the engine's
//! (`openlens_engine::commands::status_page`); this module stores.

use super::*;

/// The table, created next to the main schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS status_pages (
  project TEXT PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,
  enabled INTEGER NOT NULL DEFAULT 0,
  components TEXT NOT NULL DEFAULT '[]',
  updated_at INTEGER NOT NULL
);
";

/// A project's status page, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPageRow {
    /// The project.
    pub project: String,
    /// The slug of `/status/<slug>`.
    pub slug: String,
    /// Served to anyone.
    pub enabled: bool,
    /// The components, as the engine wrote them (JSON).
    pub components: String,
    /// Last changed (Unix seconds).
    pub updated_at: i64,
}

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StatusPageRow> {
    Ok(StatusPageRow {
        project: r.get(0)?,
        slug: r.get(1)?,
        enabled: r.get::<_, i64>(2)? != 0,
        components: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

const COLS: &str = "project, slug, enabled, components, updated_at";

impl Meta {
    /// The project's status page, if it ever chose a slug.
    pub fn status_page(&self, project: &str) -> Result<Option<StatusPageRow>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM status_pages WHERE project = ?1"),
                [project],
                from_row,
            )
            .optional()?)
        })
    }

    /// The status page with this slug, on or off. One indexed lookup:
    /// the public routes call it for every slug a client sends.
    pub fn status_page_by_slug(&self, slug: &str) -> Result<Option<StatusPageRow>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM status_pages WHERE slug = ?1"),
                [slug],
                from_row,
            )
            .optional()?)
        })
    }

    /// Store the project's status page (already checked). Refused when
    /// another project holds the slug.
    pub fn save_status_page(
        &self,
        project: &str,
        slug: &str,
        enabled: bool,
        components: &str,
        now: i64,
    ) -> Result<StatusPageRow> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let taken = tx
                .prepare("SELECT 1 FROM status_pages WHERE slug = ?1 AND project != ?2")?
                .exists(params![slug, project])?;
            if taken {
                return Err(MetaError::Invalid(
                    "another project's status page has this address: pick another",
                ));
            }
            tx.execute(
                "INSERT INTO status_pages (project, slug, enabled, components, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(project) DO UPDATE SET slug = ?2, enabled = ?3, components = ?4, updated_at = ?5",
                params![project, slug, i64::from(enabled), components, now],
            )?;
            let row = tx
                .query_row(
                    &format!("SELECT {COLS} FROM status_pages WHERE project = ?1"),
                    [project],
                    from_row,
                )
                .optional()?;
            tx.commit()?;
            row.ok_or(MetaError::Invalid("the status page was not stored"))
        })
    }
}

//! Projects, their settings, and the people seen in them. Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
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
        self.read(|c| {
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
        self.read(|c| {
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
                "uptime_checks",
                "incidents",
                "forgotten",
                "share_links",
                "notes",
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
        self.read(|c| {
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
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT role, since FROM role_history WHERE project = ?1 AND distinct_id = ?2 ORDER BY since",
            )?;
            let rows = st.query_map(params![project, distinct_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}

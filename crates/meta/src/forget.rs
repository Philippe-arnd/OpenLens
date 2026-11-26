//! People forgotten on request (erasure). Moved out of lib.rs as is (#307).

use super::*;

impl Meta {
    /// Forget people (#230): their events are rewritten out of the files
    /// and new ones are dropped at capture. Also drops their person rows,
    /// role history and identity rows (ids and aliases, #450). The list stays (it is re-applied after a restore).
    pub fn forget(&self, project: &str, ids: &[String]) -> Result<()> {
        let now = Utc::now().timestamp();
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            for id in ids {
                tx.execute(
                    "INSERT OR REPLACE INTO forgotten (project, distinct_id, at) VALUES (?1, ?2, ?3)",
                    params![project, id, now],
                )?;
                tx.execute("DELETE FROM persons WHERE project = ?1 AND distinct_id = ?2", params![project, id])?;
                tx.execute("DELETE FROM role_history WHERE project = ?1 AND distinct_id = ?2", params![project, id])?;
                identity::forget_in(&tx, project, id)?;
            }
            tx.commit()?;
            Ok(())
        })?;
        if let Ok(mut cache) = self.forgotten.lock() {
            self.forgotten_gen
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            cache.remove(project);
        }
        Ok(())
    }

    /// Is this person forgotten?
    pub fn is_forgotten(&self, project: &str, id: &str) -> Result<bool> {
        if let Ok(cache) = self.forgotten.lock()
            && let Some(set) = cache.get(project)
        {
            return Ok(set.contains(id));
        }
        let generation = self.forgotten_gen.load(std::sync::atomic::Ordering::SeqCst);
        let set: HashSet<String> = self.read(|c| {
            let mut st = c.prepare("SELECT distinct_id FROM forgotten WHERE project = ?1")?;
            let rows = st.query_map([project], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
        })?;
        let found = set.contains(id);
        // Cached only if no forget happened since the read: the bump and the
        // check both run under the cache lock.
        if let Ok(mut cache) = self.forgotten.lock()
            && self.forgotten_gen.load(std::sync::atomic::Ordering::SeqCst) == generation
        {
            cache.insert(project.to_owned(), Arc::new(set));
        }
        Ok(found)
    }

    /// People forgotten since `since` (Unix seconds).
    pub fn forgotten_since(&self, project: &str, since: i64) -> Result<Vec<String>> {
        self.read(|c| {
            let mut st =
                c.prepare("SELECT distinct_id FROM forgotten WHERE project = ?1 AND at >= ?2")?;
            let rows = st
                .query_map(params![project, since], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(rows)
        })
    }
}

//! Events handed over by a flush, not yet in Parquet (#293), moved out of lib.rs (#307).

use std::fs;
use std::path::PathBuf;

use openlens_event::Event;

use crate::{Result, sync_dir, write_segments};

/// Events taken by [`Writer::take`], not yet in Parquet.
#[derive(Clone)]
pub struct Pending {
    pub(crate) root: PathBuf,
    pub(crate) events: Vec<Event>,
    pub(crate) old_wals: Vec<PathBuf>,
}

pub(crate) fn projects_of(events: &[Event]) -> std::collections::BTreeSet<String> {
    events.iter().map(|e| e.project.clone()).collect()
}

/// The UTC days (`date=` partitions) the events land on, per project.
pub(crate) fn days_of(
    events: &[Event],
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<chrono::NaiveDate>> {
    let mut by: std::collections::BTreeMap<&str, std::collections::BTreeSet<chrono::NaiveDate>> =
        std::collections::BTreeMap::new();
    for e in events {
        by.entry(e.project.as_str())
            .or_default()
            .insert(e.timestamp.date_naive());
    }
    by.into_iter().map(|(p, d)| (p.to_owned(), d)).collect()
}

impl Pending {
    /// The distinct projects these events belong to.
    pub fn projects(&self) -> std::collections::BTreeSet<String> {
        projects_of(&self.events)
    }

    /// The UTC days these events are written to, per project (#516: a
    /// late event invalidates the results kept for its day).
    pub fn days(
        &self,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<chrono::NaiveDate>> {
        days_of(&self.events)
    }

    /// Write the segments, then remove the WAL that covered them. Returns
    /// the number of events written. On error the WAL stays: hand the
    /// events back with [`Writer::restore`].
    pub fn write(&self) -> Result<usize> {
        write_segments(&self.root, &self.events)?;
        // Segments are durable: the WAL that covered them can go.
        for wal in &self.old_wals {
            match fs::remove_file(wal) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        sync_dir(&self.root.join("wal"))?;
        Ok(self.events.len())
    }
}

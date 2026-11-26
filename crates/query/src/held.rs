//! Rollup rows held in memory for the queries of one board (#452, D-072).
//!
//! A cold board runs some forty queries, and most of them read the same
//! closed days of the same rollup tables (`person_day`, `session_day`,
//! `success_day`, `session_feature_day`), each from the Parquet files.
//! [`QueryEngine::holding`] reads each table's rows once, keeps them in
//! memory, and gives a copy of the engine whose queries read that copy in
//! place of the files under the same table name. The caller selects rows
//! that are a superset of what its queries read, so their answers are the
//! same.
//!
//! The rows count against the query pool (D-033) and against a budget the
//! caller gives for all its tables together: a table that does not fit is
//! not held, and its queries read the files as before.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use datafusion::arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use datafusion::execution::memory_pool::{MemoryConsumer, MemoryReservation};
use futures::StreamExt;

use crate::{QueryEngine, QueryError, bind, offload, plan, retry};

/// A rollup table held in memory for one project, and its reservation in
/// the pool: both freed when the last engine copy holding it is dropped.
#[derive(Debug)]
pub(crate) struct Held {
    pub(crate) project: String,
    pub(crate) name: &'static str,
    pub(crate) table: Arc<MemTable>,
    _reservation: MemoryReservation,
}

/// What [`QueryEngine::holding`] did with one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTable {
    /// The rollup table.
    pub name: &'static str,
    /// Whether its rows are held: when not, its queries read the files.
    pub held: bool,
    /// Rows held (0 when not held).
    pub rows: usize,
    /// Bytes held (0 when not held).
    pub bytes: usize,
}

/// The part of a budget one build has taken, given back unless kept.
struct Taken<'a> {
    budget: &'a AtomicUsize,
    n: usize,
    keep: bool,
}

impl Taken<'_> {
    /// Take `bytes` more from the budget, if it has them.
    fn take(&mut self, bytes: usize) -> bool {
        let ok = self
            .budget
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(bytes)
            })
            .is_ok();
        if ok {
            self.n += bytes;
        }
        ok
    }
}

impl Drop for Taken<'_> {
    fn drop(&mut self) {
        if !self.keep {
            self.budget.fetch_add(self.n, Ordering::SeqCst);
        }
    }
}

impl QueryEngine {
    /// A copy of this engine whose queries on `project` read each table of
    /// `tables` (a rollup table name and the `SELECT` of its rows, with
    /// this engine's parameters) from memory, read once here, instead of
    /// from its files. The tables are read side by side and take at most
    /// `budget` bytes together, within the query pool; a table that does
    /// not fit, takes longer than the time limit or fails is not held, its
    /// queries read the files, and the log says so.
    ///
    /// The rows selected must include every row the copy's queries read
    /// from that table: the copy sees nothing else.
    pub async fn holding(
        &self,
        project: &str,
        tables: Vec<(&'static str, String)>,
        budget: usize,
    ) -> (QueryEngine, Vec<HeldTable>) {
        let budget = Arc::new(AtomicUsize::new(budget));
        let builds = tables.into_iter().map(|(name, sql)| {
            let (this, project, budget) = (self.clone(), project.to_owned(), budget.clone());
            async move {
                let rollup = openlens_storage::rollup::tables()
                    .iter()
                    .any(|(n, _)| *n == name);
                let built = if rollup {
                    offload(async move {
                        retry::once_more(|| this.hold(&project, name, &sql, &budget)).await
                    })
                    .await
                } else {
                    Err(QueryError::Rejected(format!(
                        "{name} is not a rollup table"
                    )))
                };
                (name, built)
            }
        });
        let mut held = self.held.as_ref().clone();
        let mut report = Vec::new();
        for (name, built) in futures::future::join_all(builds).await {
            match built {
                Ok(Some((h, rows, bytes))) => {
                    held.retain(|x| !(x.project == h.project && x.name == h.name));
                    held.push(Arc::new(h));
                    report.push(HeldTable {
                        name,
                        held: true,
                        rows,
                        bytes,
                    });
                }
                Ok(None) => {
                    tracing::info!(
                        project,
                        table = name,
                        "rollup rows too large to hold in memory; the board reads the files"
                    );
                    report.push(HeldTable {
                        name,
                        held: false,
                        rows: 0,
                        bytes: 0,
                    });
                }
                Err(err) => {
                    tracing::warn!(project, table = name, %err, "rollup rows not held in memory; the board reads the files");
                    report.push(HeldTable {
                        name,
                        held: false,
                        rows: 0,
                        bytes: 0,
                    });
                }
            }
        }
        let kept: Vec<&HeldTable> = report.iter().filter(|t| t.held).collect();
        tracing::info!(
            project,
            tables = kept.len(),
            rows = kept.iter().map(|t| t.rows).sum::<usize>(),
            bytes = kept.iter().map(|t| t.bytes).sum::<usize>(),
            "rollup rows held in memory for the board"
        );
        let copy = QueryEngine {
            held: Arc::new(held),
            ..self.clone()
        };
        (copy, report)
    }

    /// Read `sql` into an in-memory table named `name`: `None` when it
    /// does not fit in what is left of `budget` or in the pool.
    async fn hold(
        &self,
        project: &str,
        name: &'static str,
        sql: &str,
        budget: &AtomicUsize,
    ) -> Result<Option<(Held, usize, usize)>, QueryError> {
        if sql.len() > 20_000 {
            return Err(QueryError::Rejected("query is too long".into()));
        }
        let Some(runtime) = &self.runtime else {
            return Ok(None);
        };
        let run = async {
            let ctx = self.context(project).await?;
            let df = bind(plan(&ctx, sql).await?, &self.params)?;
            let schema = Arc::new(df.schema().as_arrow().clone());
            let mut stream = df
                .execute_stream()
                .await
                .map_err(|e| QueryError::Rejected(e.to_string()))?;
            let reservation =
                MemoryConsumer::new(format!("held {name}")).register(&runtime.memory_pool);
            let mut taken = Taken {
                budget,
                n: 0,
                keep: false,
            };
            let mut batches: Vec<RecordBatch> = Vec::new();
            let mut rows = 0usize;
            while let Some(batch) = stream.next().await {
                let batch = batch.map_err(|e| QueryError::Rejected(e.to_string()))?;
                let bytes = batch.get_array_memory_size();
                if !taken.take(bytes) || reservation.try_grow(bytes).is_err() {
                    return Ok(None);
                }
                rows += batch.num_rows();
                batches.push(batch);
            }
            // Spread over the query's partitions, so a scan of the held
            // rows runs on every thread, as a scan of the files did.
            let parts = crate::host::query_threads().max(1);
            let mut partitions: Vec<Vec<RecordBatch>> = vec![Vec::new(); parts];
            for (i, b) in batches.into_iter().enumerate() {
                if let Some(p) = partitions.get_mut(i % parts) {
                    p.push(b);
                }
            }
            let table = MemTable::try_new(schema, partitions)
                .map_err(|e| QueryError::Storage(e.to_string()))?;
            taken.keep = true;
            let bytes = taken.n;
            Ok(Some((
                Held {
                    project: project.to_owned(),
                    name,
                    table: Arc::new(table),
                    _reservation: reservation,
                },
                rows,
                bytes,
            )))
        };
        match tokio::time::timeout(self.limits.timeout, run).await {
            Ok(r) => r.map_err(crate::friendly),
            Err(_) => Err(self.timed_out(self.limits.timeout)),
        }
    }
}

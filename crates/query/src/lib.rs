//! Query layer on DataFusion: read-only SQL per project.
//!
//! Every query runs in a fresh session that only knows one project's
//! partition tree, registered as the table `events` (with a `date`
//! partition column, so `WHERE date >= '…'` prunes directories). A query
//! therefore cannot see another project's data by construction.
//!
//! Guards: no DDL, DML or statements (so no `CREATE EXTERNAL TABLE` to read
//! arbitrary files), a memory pool cap, a wall-clock timeout and a row cap.
//!
//! Layer: L2 (see `docs/architecture.md`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::datatypes::DataType;
use datafusion::datasource::MemTable;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::memory_pool::{FairSpillPool, MemoryConsumer, MemoryReservation};
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{SQLOptions, SessionConfig, SessionContext};
use futures::StreamExt;

mod aggregate;
pub use aggregate::MIN_USERS;
use serde::Serialize;

/// How long the anonymous-id table of a project is reused before it is
/// read again. Events are flushed every ~10 s, so this stays fresh enough.
const ALIAS_TTL: Duration = Duration::from_secs(30);

/// `person_aliases(anon, person)`: anonymous ids that were later identified.
///
/// The SDK sends `$anon_distinct_id` on `$identify`, so events sent before
/// login (often the first page view) can be given to the user instead of
/// counting as an extra visitor. When an anonymous id was identified as
/// several users, the smallest id wins.
const ALIAS_SQL: &str = "SELECT anon, min(person) AS person FROM (\
     SELECT regexp_match(properties, '\"\\$anon_distinct_id\":\"([^\"]{1,200})\"')[1] AS anon, \
     distinct_id AS person FROM raw_events WHERE event = '$identify') \
     WHERE anon IS NOT NULL AND anon <> person GROUP BY anon";

/// The `events` table as callers see it: `raw_events` with every
/// anonymous id found in `person_aliases` replaced by its user. Derived
/// from the data, so it also fixes what was stored before, and nothing is
/// rewritten.
fn events_view_sql() -> String {
    let cols: Vec<String> = openlens_storage::event_schema()
        .fields()
        .iter()
        .map(|f| {
            if f.name() == "distinct_id" {
                "COALESCE(a.person, r.distinct_id) AS distinct_id".to_owned()
            } else {
                format!("r.{}", f.name())
            }
        })
        .collect();
    format!(
        "SELECT {}, r.date FROM raw_events r LEFT JOIN person_aliases a ON r.distinct_id = a.anon",
        cols.join(", ")
    )
}

/// `persons(person, first_seen, internal)`: every identified user, their
/// first `$identify` over all time (not just a query's window, so "first
/// seen" is the real first visit of a returning user), and whether the app
/// marked them as the team's own (`identify(id, { internal: true })`).
const PERSONS_SQL: &str = "SELECT distinct_id AS person, min(timestamp) AS first_seen, \
     max(CASE WHEN properties LIKE '%\"$internal\":true%' THEN 1 ELSE 0 END) = 1 AS internal \
     FROM raw_events WHERE event = '$identify' GROUP BY distinct_id";

/// The identity tables of one project, read together.
#[derive(Debug, Clone)]
struct Identity {
    /// `person_aliases`; `None` when no anonymous id was linked.
    aliases: Option<Arc<MemTable>>,
    /// `persons`; may be empty.
    persons: Arc<MemTable>,
}

type AliasCache = Arc<std::sync::Mutex<HashMap<String, (Instant, Identity)>>>;

/// Run `sql` and keep its result as an in-memory table, with its row count.
async fn collect(ctx: &SessionContext, sql: &str) -> Result<(Arc<MemTable>, usize), QueryError> {
    let df = ctx
        .sql(sql)
        .await
        .map_err(|e| QueryError::Storage(e.to_string()))?;
    let schema = Arc::new(df.schema().as_arrow().clone());
    let batches = df
        .collect()
        .await
        .map_err(|e| QueryError::Storage(e.to_string()))?;
    let rows = batches.iter().map(|b| b.num_rows()).sum();
    let table =
        MemTable::try_new(schema, vec![batches]).map_err(|e| QueryError::Storage(e.to_string()))?;
    Ok((Arc::new(table), rows))
}

/// The DataFusion runtime shared by every query.
///
/// DataFusion caches directory listings by default, with no expiry. A shared
/// runtime would then never see the segments written after the first query
/// (new events, compacted days), until a restart: the listing cache is off.
/// Parquet footers stay cached: a segment file never changes once written.
fn runtime_builder(memory: usize) -> RuntimeEnvBuilder {
    RuntimeEnvBuilder::new()
        .with_memory_pool(Arc::new(FairSpillPool::new(memory)))
        .with_object_list_cache_limit(0)
}

/// Query errors. Messages are safe to show to the caller.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// The SQL is invalid or not allowed.
    #[error("{0}")]
    Rejected(String),
    /// The query ran longer than allowed.
    #[error("query took longer than {0:?}")]
    Timeout(Duration),
    /// Storage problem.
    #[error("storage: {0}")]
    Storage(String),
}

/// Limits applied to every query.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Maximum rows returned (more are cut and flagged `truncated`).
    pub max_rows: usize,
    /// Wall-clock limit.
    pub timeout: Duration,
    /// Memory pool size, in bytes (spills or fails beyond it).
    pub memory: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            timeout: Duration::from_secs(15),
            // Queries and the board snapshot share it (#329).
            memory: 768 * 1024 * 1024,
        }
    }
}

/// A query result as JSON-friendly rows.
#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    /// Column names, in order.
    pub columns: Vec<String>,
    /// Rows as JSON objects keyed by column name.
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// True when `max_rows` cut the result.
    pub truncated: bool,
}

/// An in-memory table of one project, and its reservation in the pool.
type Snapshot = (String, String, Arc<MemTable>, Arc<MemoryReservation>);

/// Runs read-only SQL against a data root.
#[derive(Debug, Clone)]
pub struct QueryEngine {
    root: PathBuf,
    limits: Limits,
    /// One runtime for every query: a single memory cap for the process, and
    /// DataFusion's Parquet metadata cache is reused across queries and
    /// requests (segments are immutable, so cached footers never go stale).
    runtime: Option<Arc<datafusion::execution::runtime_env::RuntimeEnv>>,
    /// In-memory tables added by [`QueryEngine::snapshot`], for one project.
    /// Each holds its bytes reserved in the memory pool (#329) until the
    /// last copy of the engine that uses it is dropped.
    snapshots: Arc<Vec<Snapshot>>,
    /// Per project: the identity tables and when they were read.
    aliases: AliasCache,
    /// Queries stopped at the time limit, shared by every copy (#346).
    timeouts: Arc<std::sync::atomic::AtomicU64>,
}

impl QueryEngine {
    /// Engine over the data root written by `openlens-storage`.
    pub fn new(root: impl Into<PathBuf>, limits: Limits) -> Self {
        let runtime = runtime_builder(limits.memory).build_arc().ok();
        Self {
            root: root.into(),
            limits,
            runtime,
            snapshots: Arc::default(),
            aliases: AliasCache::default(),
            timeouts: Arc::default(),
        }
    }

    /// How many queries stopped at the time limit since the start.
    pub fn timeouts(&self) -> u64 {
        self.timeouts.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn timed_out(&self, limit: Duration) -> QueryError {
        self.timeouts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        QueryError::Timeout(limit)
    }

    /// A copy of this engine where `name` is also a table for `project`:
    /// the result of `sql`, computed once and kept in memory. Used to run
    /// many queries over the same expensive projection (the board).
    ///
    /// Returns `Ok(None)` when the result has more than `max_rows` rows, so
    /// the caller falls back to querying the files directly and memory stays
    /// bounded.
    pub async fn snapshot(
        &self,
        project: &str,
        name: &str,
        sql: &str,
        max_rows: usize,
    ) -> Result<Option<QueryEngine>, QueryError> {
        let this = self.clone();
        let (project, name, sql) = (project.to_owned(), name.to_owned(), sql.to_owned());
        offload(async move { this.snapshot_here(&project, &name, &sql, max_rows).await }).await
    }

    async fn snapshot_here(
        &self,
        project: &str,
        name: &str,
        sql: &str,
        max_rows: usize,
    ) -> Result<Option<QueryEngine>, QueryError> {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
            return Err(QueryError::Rejected("invalid snapshot name".into()));
        }
        let run = async {
            let ctx = self.context(project).await?;
            let options = SQLOptions::new()
                .with_allow_ddl(false)
                .with_allow_dml(false)
                .with_allow_statements(false);
            let df = ctx
                .sql_with_options(sql, options)
                .await
                .map_err(|e| QueryError::Rejected(e.to_string()))?;
            let schema = Arc::new(df.schema().as_arrow().clone());
            let mut stream = df
                .execute_stream()
                .await
                .map_err(|e| QueryError::Rejected(e.to_string()))?;
            let mut batches = Vec::new();
            let mut rows = 0usize;
            while let Some(batch) = stream.next().await {
                let batch = batch.map_err(|e| QueryError::Rejected(e.to_string()))?;
                rows += batch.num_rows();
                if rows > max_rows {
                    return Ok(None);
                }
                batches.push(batch);
            }
            // The snapshot counts against the same memory cap as the
            // queries (#329): when the pool cannot hold it, read the files.
            let Some(runtime) = &self.runtime else {
                return Ok(None);
            };
            let bytes: usize = batches
                .iter()
                .map(datafusion::arrow::record_batch::RecordBatch::get_array_memory_size)
                .sum();
            let reservation =
                MemoryConsumer::new(format!("snapshot {name}")).register(&runtime.memory_pool);
            if reservation.try_grow(bytes).is_err() {
                tracing::debug!(
                    bytes,
                    "snapshot does not fit in the memory pool; reading the files"
                );
                return Ok(None);
            }
            let table = MemTable::try_new(schema, vec![batches])
                .map_err(|e| QueryError::Storage(e.to_string()))?;
            Ok(Some((table, reservation)))
        };
        let table = match tokio::time::timeout(self.limits.timeout, run).await {
            Ok(r) => r?,
            Err(_) => return Err(self.timed_out(self.limits.timeout)),
        };
        Ok(table.map(|(t, reservation)| {
            let mut snaps: Vec<_> = self
                .snapshots
                .iter()
                .filter(|(p, n, _, _)| !(p == project && n == name))
                .cloned()
                .collect();
            snaps.push((
                project.to_owned(),
                name.to_owned(),
                Arc::new(t),
                Arc::new(reservation),
            ));
            QueryEngine {
                snapshots: Arc::new(snaps),
                ..self.clone()
            }
        }))
    }

    async fn context(&self, project: &str) -> Result<SessionContext, QueryError> {
        let dir = openlens_storage::project_dir(&self.root, project)
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        std::fs::create_dir_all(&dir).map_err(|e| QueryError::Storage(e.to_string()))?;

        let ctx = self.session()?;

        let url = ListingTableUrl::parse(format!("{}/", dir.display()))
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        let options = ListingOptions::new(Arc::new(ParquetFormat::default()))
            .with_file_extension(".parquet")
            .with_table_partition_cols(vec![("date".to_owned(), DataType::Utf8)]);
        let table_config = ListingTableConfig::new(url)
            .with_listing_options(options)
            .with_schema(openlens_storage::event_schema());
        let table =
            ListingTable::try_new(table_config).map_err(|e| QueryError::Storage(e.to_string()))?;
        let table: Arc<ListingTable> = Arc::new(table);
        ctx.register_table("raw_events", table.clone())
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        let identity = self.identity(project, &ctx).await?;
        ctx.register_table("persons", identity.persons)
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        match identity.aliases {
            // Nobody to merge: `events` is the stored table, at no cost.
            None => {
                ctx.register_table("events", table)
                    .map_err(|e| QueryError::Storage(e.to_string()))?;
            }
            Some(aliases) => {
                ctx.register_table("person_aliases", aliases)
                    .map_err(|e| QueryError::Storage(e.to_string()))?;
                let view = ctx
                    .sql(&events_view_sql())
                    .await
                    .map_err(|e| QueryError::Storage(e.to_string()))?
                    .into_view();
                ctx.register_table("events", view)
                    .map_err(|e| QueryError::Storage(e.to_string()))?;
            }
        }
        for (p, name, t, _) in self.snapshots.iter() {
            if p == project {
                ctx.register_table(name.as_str(), t.clone())
                    .map_err(|e| QueryError::Storage(e.to_string()))?;
            }
        }
        Ok(ctx)
    }

    /// The `person_aliases` and `persons` tables of `project`, read at most
    /// every [`ALIAS_TTL`].
    async fn identity(&self, project: &str, ctx: &SessionContext) -> Result<Identity, QueryError> {
        if let Ok(cache) = self.aliases.lock()
            && let Some((at, t)) = cache.get(project)
            && at.elapsed() < ALIAS_TTL
        {
            return Ok(t.clone());
        }
        let (aliases, alias_rows) = collect(ctx, ALIAS_SQL).await?;
        let (persons, _) = collect(ctx, PERSONS_SQL).await?;
        let identity = Identity {
            aliases: (alias_rows > 0).then_some(aliases),
            persons,
        };
        if let Ok(mut cache) = self.aliases.lock() {
            cache.insert(project.to_owned(), (Instant::now(), identity.clone()));
        }
        Ok(identity)
    }

    /// Run one read-only SQL query on `project`'s events.
    pub async fn sql(&self, project: &str, sql: &str) -> Result<QueryResult, QueryError> {
        let this = self.clone();
        let (project, sql) = (project.to_owned(), sql.to_owned());
        offload(async move { this.sql_here(&project, &sql).await }).await
    }

    async fn sql_here(&self, project: &str, sql: &str) -> Result<QueryResult, QueryError> {
        self.timed(
            sql,
            Box::pin(async {
                let ctx = self.context(project).await?;
                drain(plan(&ctx, sql).await?, self.limits.max_rows).await
            }),
        )
        .await
    }

    /// Like [`QueryEngine::sql`], with `events` replaced by `source`, a
    /// `SELECT` over `events` aliased `AS events` (the feature map's
    /// projection, from `openlens_engine::source`).
    pub async fn sql_over(
        &self,
        project: &str,
        source: &str,
        sql: &str,
    ) -> Result<QueryResult, QueryError> {
        let this = self.clone();
        let (project, source, sql) = (project.to_owned(), source.to_owned(), sql.to_owned());
        offload(async move { this.sql_over_here(&project, &source, &sql).await }).await
    }

    async fn sql_over_here(
        &self,
        project: &str,
        source: &str,
        sql: &str,
    ) -> Result<QueryResult, QueryError> {
        self.timed(
            sql,
            Box::pin(async {
                let ctx = self.context(project).await?;
                if source != "events" {
                    let view = ctx
                        .sql(&format!("SELECT * FROM {source}"))
                        .await
                        .map_err(|e| QueryError::Storage(e.to_string()))?
                        .into_view();
                    ctx.deregister_table("events")
                        .map_err(|e| QueryError::Storage(e.to_string()))?;
                    ctx.register_table("events", view)
                        .map_err(|e| QueryError::Storage(e.to_string()))?;
                }
                drain(plan(&ctx, sql).await?, self.limits.max_rows).await
            }),
        )
        .await
    }

    /// Aggregate-only SQL (see the `aggregate` module): `events` has no
    /// person-level column, people are counted with `user_key` (keyed by
    /// `key`, one per project), the result must be an aggregate, and groups
    /// of fewer than [`MIN_USERS`] people are dropped.
    pub async fn sql_aggregate(
        &self,
        project: &str,
        source: &str,
        key: &[u8],
        sql: &str,
    ) -> Result<QueryResult, QueryError> {
        let this = self.clone();
        let (project, source, key, sql) = (
            project.to_owned(),
            source.to_owned(),
            key.to_vec(),
            sql.to_owned(),
        );
        offload(async move { this.sql_aggregate_here(&project, &source, &key, &sql).await }).await
    }

    async fn sql_aggregate_here(
        &self,
        project: &str,
        source: &str,
        key: &[u8],
        sql: &str,
    ) -> Result<QueryResult, QueryError> {
        self.timed(
            sql,
            Box::pin(async {
                let full = self.context(project).await?;
                full.register_udf(aggregate::pseudonym_udf(key.to_vec()));
                let view = full
                    .sql(&aggregate::view_sql(source))
                    .await
                    .map_err(|e| QueryError::Storage(e.to_string()))?
                    .into_view();
                let ctx = self.session()?;
                ctx.register_table("events", view)
                    .map_err(|e| QueryError::Storage(e.to_string()))?;
                let (state, plan) = plan(&ctx, sql).await?.into_parts();
                let plan = aggregate::guard(plan).map_err(QueryError::Rejected)?;
                drain(
                    datafusion::dataframe::DataFrame::new(state, plan),
                    self.limits.max_rows,
                )
                .await
            }),
        )
        .await
    }

    /// A context with the shared runtime and no table.
    fn session(&self) -> Result<SessionContext, QueryError> {
        let runtime = match &self.runtime {
            Some(r) => r.clone(),
            None => runtime_builder(self.limits.memory)
                .build_arc()
                .map_err(|e| QueryError::Storage(e.to_string()))?,
        };
        let config = SessionConfig::new()
            .with_target_partitions(2)
            .with_information_schema(false);
        Ok(SessionContext::new_with_config_rt(config, runtime))
    }

    /// Apply the size check, the time limit and the debug log to a query.
    async fn timed(
        &self,
        sql: &str,
        run: impl std::future::Future<Output = Result<QueryResult, QueryError>>,
    ) -> Result<QueryResult, QueryError> {
        if sql.len() > 20_000 {
            return Err(QueryError::Rejected("query is too long".into()));
        }
        let limit = self.limits.timeout;
        let started = Instant::now();
        let out = match tokio::time::timeout(limit, run).await {
            Ok(r) => r,
            Err(_) => Err(self.timed_out(limit)),
        };
        // `RUST_LOG=openlens_query=debug` shows what each query costs.
        tracing::debug!(ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), sql = %sql.chars().take(160).collect::<String>(), "query");
        out
    }
}

/// The runtime DataFusion work runs on, apart from the one that serves
/// HTTP and ingestion (#295): a board computation keeps its threads busy
/// for seconds, and `/capture` must not wait behind it. One per process,
/// never dropped; if it cannot be made, queries run where they are called.
fn query_runtime() -> Option<&'static tokio::runtime::Runtime> {
    static RT: std::sync::OnceLock<Option<tokio::runtime::Runtime>> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 8));
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(threads)
            .thread_name("openlens-query")
            .enable_all()
            .build()
            .ok()
    })
    .as_ref()
}

/// Run a query future on [`query_runtime`] and wait for it here.
async fn offload<T: Send + 'static>(
    fut: impl std::future::Future<Output = Result<T, QueryError>> + Send + 'static,
) -> Result<T, QueryError> {
    match query_runtime() {
        Some(rt) => rt
            .spawn(fut)
            .await
            .map_err(|e| QueryError::Storage(format!("query task: {e}")))?,
        None => fut.await,
    }
}

/// Plan read-only SQL: no DDL, DML or statements.
async fn plan(
    ctx: &SessionContext,
    sql: &str,
) -> Result<datafusion::dataframe::DataFrame, QueryError> {
    let options = SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false);
    ctx.sql_with_options(sql, options)
        .await
        .map_err(|e| QueryError::Rejected(e.to_string()))
}

/// Run a query and collect at most `max_rows` rows as JSON objects.
async fn drain(
    df: datafusion::dataframe::DataFrame,
    max_rows: usize,
) -> Result<QueryResult, QueryError> {
    let columns: Vec<String> = df
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let mut stream = df
        .execute_stream()
        .await
        .map_err(|e| QueryError::Rejected(e.to_string()))?;
    let mut rows = Vec::new();
    let mut truncated = false;
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| QueryError::Rejected(e.to_string()))?;
        let mut buf = Vec::new();
        {
            let mut w = arrow::json::ArrayWriter::new(&mut buf);
            w.write(&batch)
                .map_err(|e| QueryError::Storage(e.to_string()))?;
            w.finish().map_err(|e| QueryError::Storage(e.to_string()))?;
        }
        let parsed: Vec<serde_json::Map<String, serde_json::Value>> = if buf.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&buf).map_err(|e| QueryError::Storage(e.to_string()))?
        };
        for row in parsed {
            if rows.len() >= max_rows {
                truncated = true;
                break;
            }
            rows.push(row);
        }
        if truncated {
            break;
        }
    }
    Ok(QueryResult {
        columns,
        rows,
        truncated,
    })
}

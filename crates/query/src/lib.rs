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
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::memory_pool::FairSpillPool;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{SQLOptions, SessionConfig, SessionContext};
use futures::StreamExt;

mod aggregate;
pub mod functions;
mod host;
mod identity;
use identity::{AliasCache, person_of_udf};
pub use identity::{IdentityFn, IdentityRows, IdentitySource, PersonRow};
mod internal;
mod retry;
mod rollup;
pub use openlens_storage::DaySize;
pub use openlens_storage::rollup::last_closed;
pub use rollup::{DayFeatures, INTERACTION};
mod typed;
pub use aggregate::MIN_USERS;
use serde::Serialize;

/// How long the anonymous-id table of a project is reused before it is
/// read again. Events are flushed every ~10 s, so this stays fresh enough.
const ALIAS_TTL: Duration = Duration::from_secs(30);

/// `openlens_event::names::REQUEST_STATS` (#593), not user activity.
const REQUEST_STATS: &str = "$request_stats";
/// `openlens_event::names::RUNTIME_STATS` (#610), not user activity.
const RUNTIME_STATS: &str = "$runtime_stats";

/// The `events` table as callers see it: `raw_events` with every
/// anonymous id found in `person_aliases` replaced by its user, through
/// the scalar function `person_of` (a lookup in the cached alias map, not
/// a join: a join would build its hash table on the events), when `merge`
/// is set, and without server request and runtime stats (#593, #610).
/// Derived from the data, so it also fixes what was stored before, and
/// nothing is rewritten. The typed columns read the text on rows
/// stored before they existed (#449, see `typed`).
fn events_view_sql(merge: bool) -> String {
    let cols: Vec<String> = openlens_storage::event_schema()
        .fields()
        .iter()
        .map(|f| {
            if merge && f.name() == "distinct_id" {
                "person_of(r.distinct_id) AS distinct_id".to_owned()
            } else {
                typed::select(f.name())
            }
        })
        .collect();
    // Server request and runtime stats (#593, #610) are aggregates, not
    // user activity: the board, the rollups and every KPI read `events`,
    // so they never see them. Their readers query `raw_events`.
    format!(
        "SELECT {}, r.date FROM raw_events r WHERE r.event NOT IN ('{}', '{}')",
        cols.join(", "),
        REQUEST_STATS,
        RUNTIME_STATS
    )
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
    /// Another long (cold) query is running: one at a time (#326).
    #[error("another long query is running; try again in a few seconds")]
    Busy,
    /// The query needs more memory than the server gives queries (#326).
    #[error("this query needs more memory than the server allows: narrow the dates or the filters")]
    TooBig,
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

/// Per segment file: (size, modified, what it holds). A segment is never
/// edited once written, so a file with the same size and time is the same.
type Facts = HashMap<
    PathBuf,
    (
        u64,
        Option<std::time::SystemTime>,
        Arc<openlens_storage::SegmentFacts>,
    ),
>;
/// [`Facts`] per project: a board replaces only its own project's entry,
/// so projects never evict each other.
type FactsCache = Arc<std::sync::Mutex<HashMap<String, Facts>>>;

/// The stored rows of a project, counted without a query (#447).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawSummary {
    /// Rows in `raw_events`.
    pub rows: usize,
    /// Its distinct environments, sorted.
    pub environments: Vec<String>,
}

/// The empty directory a project with no stored events is read from.
const EMPTY_DIR: &str = "query-empty";

/// Runs read-only SQL against a data root.
#[derive(Debug, Clone)]
pub struct QueryEngine {
    /// What each segment holds, read once (#447).
    facts: FactsCache,
    /// Segments whose facts were read from disk, since the start.
    facts_read: Arc<std::sync::atomic::AtomicU64>,
    root: PathBuf,
    limits: Limits,
    /// One runtime for every query: a single memory cap for the process, and
    /// DataFusion's Parquet metadata cache is reused across queries and
    /// requests (segments are immutable, so cached footers never go stale).
    runtime: Option<Arc<datafusion::execution::runtime_env::RuntimeEnv>>,
    /// Per project: the identity tables and when they were read.
    aliases: AliasCache,
    /// The identity tables kept at ingestion (#450), when the caller has them.
    identity_source: Option<IdentitySource>,
    /// Times the identity tables were derived from the files (#450).
    identity_scans: Arc<std::sync::atomic::AtomicU64>,
    /// Bumped by every identity change, so a read that overlapped one is
    /// not cached (#450).
    identity_gen: Arc<std::sync::atomic::AtomicU64>,
    /// One long (cold) query at a time per engine, so the board and
    /// ingestion never wait behind two of them (#326). Shared by every copy
    /// of the engine: one per process in the server. Not a process-wide
    /// static, so engines in the same test binary do not take turns (#458).
    cold_turn: Arc<tokio::sync::Semaphore>,
    /// Queries stopped at the time limit, shared by every copy (#346).
    timeouts: Arc<std::sync::atomic::AtomicU64>,
    /// Values for the `$name` placeholders of this copy's queries (#310).
    params: Arc<openlens_sqlgen::Params>,
    /// Queries of this copy are cold: free-form SQL from Ask, the SQL page
    /// or the API, which may scan anything (#326).
    cold: bool,
}

impl QueryEngine {
    /// Engine over the data root written by `openlens-storage`.
    pub fn new(root: impl Into<PathBuf>, limits: Limits) -> Self {
        let runtime = runtime_builder(limits.memory).build_arc().ok();
        Self {
            root: root.into(),
            limits,
            runtime,
            facts: FactsCache::default(),
            facts_read: Arc::default(),
            aliases: AliasCache::default(),
            identity_source: None,
            identity_scans: Arc::default(),
            identity_gen: Arc::default(),
            cold_turn: Arc::new(tokio::sync::Semaphore::new(1)),
            timeouts: Arc::default(),
            params: Arc::default(),
            cold: false,
        }
    }

    /// This engine, for free-form queries that may scan any range (Ask,
    /// the SQL page, the API): at most one runs at a time, and another one
    /// fails at once with [`QueryError::Busy`] instead of waiting (#326).
    pub fn cold(&self) -> Self {
        Self {
            cold: true,
            ..self.clone()
        }
    }

    /// This engine, with `params` filling the `$name` placeholders of its
    /// queries (#310). Values never become SQL text.
    pub fn with_params(&self, params: &openlens_sqlgen::Params) -> Self {
        Self {
            params: Arc::new(params.clone()),
            ..self.clone()
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

    /// Register the project's stored events as `raw_events` on `ctx`.
    fn register_raw(
        &self,
        ctx: &SessionContext,
        project: &str,
    ) -> Result<Arc<ListingTable>, QueryError> {
        let mut dir = openlens_storage::project_dir(&self.root, project)
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        // A project with nothing stored reads an empty directory shared by
        // all, never its own: a query still running when the project is
        // deleted must not bring its directory back.
        if !dir.is_dir() {
            dir = self.root.join(EMPTY_DIR);
            std::fs::create_dir_all(&dir).map_err(|e| QueryError::Storage(e.to_string()))?;
        }
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
        Ok(table)
    }

    async fn context(&self, project: &str) -> Result<SessionContext, QueryError> {
        let ctx = self.session()?;
        functions::register(&ctx);
        self.register_raw(&ctx, project)?;
        let identity = self.identity(project, &ctx).await?;
        ctx.register_table("persons", identity.persons)
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        ctx.register_udf(internal::is_internal_udf(identity.internal));
        let merge = match identity.aliases {
            // Nobody to merge: no lookup per row.
            None => false,
            Some(aliases) => {
                ctx.register_udf(person_of_udf(identity.lookup));
                ctx.register_table("person_aliases", aliases)
                    .map_err(|e| QueryError::Storage(e.to_string()))?;
                true
            }
        };
        let view = ctx
            .sql(&events_view_sql(merge))
            .await
            .map_err(|e| QueryError::Storage(e.to_string()))?
            .into_view();
        ctx.register_table("events", view)
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        // The rollups of closed days (#322), read-only like the events.
        let rdir = openlens_storage::rollup::rollups_dir(&self.root, project)
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        for (name, schema) in openlens_storage::rollup::tables() {
            let url = ListingTableUrl::parse(format!("{}/", rdir.display()))
                .map_err(|e| QueryError::Storage(e.to_string()))?;
            let options = ListingOptions::new(Arc::new(ParquetFormat::default()))
                .with_file_extension(format!("{name}.parquet"))
                .with_table_partition_cols(vec![("date".to_owned(), DataType::Utf8)]);
            let config = ListingTableConfig::new(url)
                .with_listing_options(options)
                .with_schema(schema);
            let t =
                ListingTable::try_new(config).map_err(|e| QueryError::Storage(e.to_string()))?;
            ctx.register_table(name, Arc::new(t))
                .map_err(|e| QueryError::Storage(e.to_string()))?;
        }
        Ok(ctx)
    }

    /// The number of stored rows and the environments seen, as
    /// `SELECT count(*)` and `SELECT DISTINCT environment` over `raw_events`
    /// would answer, without scanning: each segment is read once (its footer
    /// and its `environment` column) and remembered, so a board only reads
    /// the segments written since the last one (#447).
    pub async fn raw_summary(&self, project: &str) -> Result<RawSummary, QueryError> {
        let (root, facts, project) = (self.root.clone(), self.facts.clone(), project.to_owned());
        let read = self.facts_read.clone();
        let work = move || -> Result<RawSummary, QueryError> {
            let storage = |e: &dyn std::fmt::Display| QueryError::Storage(e.to_string());
            let files =
                openlens_storage::segment_files(&root, &project).map_err(|e| storage(&e))?;
            let mut seen = HashMap::new();
            let mut summary = RawSummary::default();
            let mut envs = std::collections::BTreeSet::new();
            for path in files {
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue; // compacted away since the listing
                };
                let stamp = (meta.len(), meta.modified().ok());
                let cached = facts.lock().ok().and_then(|c| {
                    c.get(&project)?
                        .get(&path)
                        .filter(|(l, m, _)| (*l, *m) == stamp)
                        .cloned()
                });
                let f = match cached {
                    Some((_, _, f)) => f,
                    None => match openlens_storage::segment_facts(&path) {
                        Ok(f) => {
                            read.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            Arc::new(f)
                        }
                        Err(_) if !path.exists() => continue,
                        Err(e) => return Err(storage(&e)),
                    },
                };
                summary.rows += f.rows;
                envs.extend(f.environments.iter().cloned());
                seen.insert(path, (stamp.0, stamp.1, f));
            }
            summary.environments = envs.into_iter().collect();
            // Forget this project's segments that are gone.
            if let Ok(mut c) = facts.lock() {
                c.insert(project, seen);
            }
            Ok(summary)
        };
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|e| QueryError::Storage(format!("summary task: {e}")))?
    }

    /// What each stored UTC day of `project` from `from` to `to` weighs
    /// (#514): files, bytes and rows, from the file sizes and the Parquet
    /// footers, without a query (`openlens_storage::day_sizes`).
    pub async fn day_sizes(
        &self,
        project: &str,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
    ) -> Result<Vec<DaySize>, QueryError> {
        let (root, project) = (self.root.clone(), project.to_owned());
        tokio::task::spawn_blocking(move || {
            openlens_storage::day_sizes(&root, &project, from, to)
                .map_err(|e| QueryError::Storage(e.to_string()))
        })
        .await
        .map_err(|e| QueryError::Storage(format!("day sizes task: {e}")))?
    }

    /// The UTC days of `project` from `from` to `to` with a rollup but no
    /// events on disk (#489, `openlens_storage::days_without_events`):
    /// listings only.
    pub async fn days_without_events(
        &self,
        project: &str,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
    ) -> Result<Vec<chrono::NaiveDate>, QueryError> {
        let (root, project) = (self.root.clone(), project.to_owned());
        tokio::task::spawn_blocking(move || {
            openlens_storage::days_without_events(&root, &project, from, to)
                .map_err(|e| QueryError::Storage(e.to_string()))
        })
        .await
        .map_err(|e| QueryError::Storage(format!("stored days task: {e}")))?
    }

    /// Segments read by [`QueryEngine::raw_summary`] since the start: the
    /// others came from its cache.
    pub fn segments_read(&self) -> u64 {
        self.facts_read.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Run one read-only SQL query on `project`'s events.
    pub async fn sql(&self, project: &str, sql: &str) -> Result<QueryResult, QueryError> {
        let this = self.clone();
        let (project, sql) = (project.to_owned(), sql.to_owned());
        offload(async move { retry::once_more(|| this.sql_here(&project, &sql)).await }).await
    }

    async fn sql_here(&self, project: &str, sql: &str) -> Result<QueryResult, QueryError> {
        self.timed(
            sql,
            Box::pin(async {
                let ctx = self.context(project).await?;
                drain(
                    bind(plan(&ctx, sql).await?, &self.params)?,
                    self.limits.max_rows,
                )
                .await
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
        offload(
            async move { retry::once_more(|| this.sql_over_here(&project, &source, &sql)).await },
        )
        .await
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
                drain(
                    bind(plan(&ctx, sql).await?, &self.params)?,
                    self.limits.max_rows,
                )
                .await
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
        offload(async move {
            retry::once_more(|| this.sql_aggregate_here(&project, &source, &key, &sql)).await
        })
        .await
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
                let (state, plan) = bind(plan(&ctx, sql).await?, &self.params)?.into_parts();
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
            .with_target_partitions(host::query_threads())
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
        let _turn = if self.cold {
            Some(self.cold_turn.try_acquire().map_err(|_| QueryError::Busy)?)
        } else {
            None
        };
        let limit = self.limits.timeout;
        let started = Instant::now();
        let out = match tokio::time::timeout(limit, run).await {
            Ok(r) => r,
            Err(_) => Err(self.timed_out(limit)),
        }
        .map_err(friendly);
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
        let threads = host::query_threads();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(threads)
            .thread_name("openlens-query")
            .enable_all()
            .build()
            .ok()
    })
    .as_ref()
}

/// A query task that stops when nobody waits for it any more (#515): a
/// cancelled board job, a request that timed out or a client gone.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        // A no-op once the task is done.
        self.0.abort();
    }
}

/// Run a query future on [`query_runtime`] and wait for it here. Dropping
/// the returned future stops the query.
async fn offload<T: Send + 'static>(
    fut: impl std::future::Future<Output = Result<T, QueryError>> + Send + 'static,
) -> Result<T, QueryError> {
    match query_runtime() {
        Some(rt) => {
            let mut task = AbortOnDrop(rt.spawn(fut));
            (&mut task.0)
                .await
                .map_err(|e| QueryError::Storage(format!("query task: {e}")))?
        }
        None => fut.await,
    }
}

/// Plan read-only SQL: no DDL, DML or statements.
/// DataFusion's out-of-memory error, said plainly (#326).
fn friendly(e: QueryError) -> QueryError {
    match &e {
        QueryError::Rejected(m) | QueryError::Storage(m) if m.contains("Resources exhausted") => {
            QueryError::TooBig
        }
        _ => e,
    }
}

/// Fill the `$name` placeholders from `params` (#310).
fn bind(
    df: datafusion::dataframe::DataFrame,
    params: &openlens_sqlgen::Params,
) -> Result<datafusion::dataframe::DataFrame, QueryError> {
    if params.is_empty() {
        return Ok(df);
    }
    let values: HashMap<String, datafusion::scalar::ScalarValue> = params
        .iter()
        .map(|(k, v)| {
            (
                k.to_owned(),
                datafusion::scalar::ScalarValue::Utf8(Some(v.to_owned())),
            )
        })
        .collect();
    df.with_param_values(datafusion::common::ParamValues::from(values))
        .map_err(|e| QueryError::Rejected(e.to_string()))
}

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

#[cfg(test)]
mod tests;

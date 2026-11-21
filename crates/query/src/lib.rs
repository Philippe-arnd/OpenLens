//! Query layer on DataFusion: read-only SQL per project.
//!
//! Every query runs in a fresh session that only knows one project's
//! partition tree, registered as the table `events` (with `date` and `hour`
//! partition columns, so `WHERE date >= '…'` prunes directories). A query
//! therefore cannot see another project's data by construction.
//!
//! Guards: no DDL, DML or statements (so no `CREATE EXTERNAL TABLE` to read
//! arbitrary files), a memory pool cap, a wall-clock timeout and a row cap.
//!
//! Layer: L2 (see `docs/architecture.md`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arrow::datatypes::DataType;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::memory_pool::FairSpillPool;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{SQLOptions, SessionConfig, SessionContext};
use futures::StreamExt;
use serde::Serialize;

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
            memory: 512 * 1024 * 1024,
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

/// Runs read-only SQL against a data root.
#[derive(Debug, Clone)]
pub struct QueryEngine {
    root: PathBuf,
    limits: Limits,
}

impl QueryEngine {
    /// Engine over the data root written by `openlens-storage`.
    pub fn new(root: impl Into<PathBuf>, limits: Limits) -> Self {
        Self {
            root: root.into(),
            limits,
        }
    }

    async fn context(&self, project: &str) -> Result<SessionContext, QueryError> {
        let dir = openlens_storage::project_dir(&self.root, project)
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        std::fs::create_dir_all(&dir).map_err(|e| QueryError::Storage(e.to_string()))?;

        let runtime = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(FairSpillPool::new(self.limits.memory)))
            .build_arc()
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        let config = SessionConfig::new()
            .with_target_partitions(2)
            .with_information_schema(false);
        let ctx = SessionContext::new_with_config_rt(config, runtime);

        let url = ListingTableUrl::parse(format!("{}/", dir.display()))
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        let options = ListingOptions::new(Arc::new(ParquetFormat::default()))
            .with_file_extension(".parquet")
            .with_table_partition_cols(vec![
                ("date".to_owned(), DataType::Utf8),
                ("hour".to_owned(), DataType::Utf8),
            ]);
        let table_config = ListingTableConfig::new(url)
            .with_listing_options(options)
            .with_schema(openlens_storage::event_schema());
        let table =
            ListingTable::try_new(table_config).map_err(|e| QueryError::Storage(e.to_string()))?;
        ctx.register_table("events", Arc::new(table))
            .map_err(|e| QueryError::Storage(e.to_string()))?;
        Ok(ctx)
    }

    /// Run one read-only SQL query on `project`'s events.
    pub async fn sql(&self, project: &str, sql: &str) -> Result<QueryResult, QueryError> {
        if sql.len() > 20_000 {
            return Err(QueryError::Rejected("query is too long".into()));
        }
        let limits = self.limits;
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
                    if rows.len() >= limits.max_rows {
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
        };
        match tokio::time::timeout(limits.timeout, run).await {
            Ok(r) => r,
            Err(_) => Err(QueryError::Timeout(limits.timeout)),
        }
    }
}

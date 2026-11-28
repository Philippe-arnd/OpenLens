//! The identity tables of the query layer: `person_aliases` and `persons`,
//! the alias lookup read into memory, and the `person_of` function that
//! applies it (#445).
//!
//! Since #450 the server keeps both tables at ingestion, in its metadata
//! store, and hands them to the engine through an [`IdentitySource`]. The
//! engine derives them from the files ([`ALIAS_SQL`], [`PERSONS_SQL`]) only
//! when no source is set, or while the source is not ready yet (the
//! one-time backfill after an upgrade has not finished).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use arrow::array::{
    Array, BooleanArray, RecordBatch, StringArray, TimestampMicrosecondArray, cast::AsArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use datafusion::common::DataFusionError;
use datafusion::datasource::MemTable;
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility, create_udf};
use datafusion::prelude::SessionContext;

use crate::{ALIAS_TTL, QueryEngine, QueryError, internal};

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

/// `persons(person, first_seen, internal, signed_up)`: every identified
/// user, their first `$identify` over all time (not just a query's window,
/// so "first seen" is the real first visit of a returning user), whether
/// the app marked them as the team's own (`identify(id, { internal: true
/// })`), and the earliest account creation date it sent (`created_at`,
/// #707, D-111: from 1990 on here, not after now in [`person_rows`]).
/// A person's signup is `COALESCE(signed_up, first_seen)` (docs/kpis.md).
const PERSONS_SQL: &str = "SELECT person, min(first_seen) AS first_seen, \
     max(internal) = 1 AS internal, \
     arrow_cast(min(CASE WHEN created >= CAST('1990-01-01T00:00:00' AS TIMESTAMP) \
     THEN created END), 'Timestamp(Microsecond, Some(\"UTC\"))') AS signed_up FROM (\
     SELECT distinct_id AS person, timestamp AS first_seen, \
     CASE WHEN properties LIKE '%\"$internal\":true%' THEN 1 ELSE 0 END AS internal, \
     try_cast(regexp_match(properties, '\"\\$created_at\":\"([^\"]{1,64})\"')[1] AS TIMESTAMP) AS created \
     FROM raw_events WHERE event = '$identify') GROUP BY person";

/// One identified person, as `persons` holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonRow {
    /// The user id.
    pub person: String,
    /// Their first `$identify`, in microseconds since the Unix epoch (UTC).
    pub first_seen: i64,
    /// Marked as the team's own.
    pub internal: bool,
    /// The account's creation date the app sent (`created_at`, #707,
    /// D-111), in microseconds since the Unix epoch (UTC).
    pub signed_up: Option<i64>,
}

/// The identity tables of one project (#450).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityRows {
    /// `(anon, person)`: an anonymous id and the user it counts as.
    pub aliases: Vec<(String, String)>,
    /// Every identified person.
    pub persons: Vec<PersonRow>,
}

/// Reads a project's identity tables kept at ingestion. `Ok(None)` means
/// "not ready yet": the engine then derives them from the files, as it
/// did before #450. Called off the async threads (it may block).
pub type IdentityFn = dyn Fn(&str) -> Result<Option<IdentityRows>, String> + Send + Sync + 'static;

/// Where the engine reads identity from (see [`QueryEngine::with_identity`]).
#[derive(Clone)]
pub struct IdentitySource(pub Arc<IdentityFn>);

impl std::fmt::Debug for IdentitySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdentitySource")
    }
}

/// The identity tables of one project, read together.
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    /// `person_aliases`, empty when no anonymous id was linked.
    pub(crate) aliases: Arc<MemTable>,
    /// `persons`; may be empty.
    pub(crate) persons: Arc<MemTable>,
    /// The people of `persons` marked internal, for `is_internal`.
    pub(crate) internal: internal::Internal,
    /// The same aliases as a lookup, anon id to user, shared by every query.
    pub(crate) lookup: Arc<HashMap<String, String>>,
    /// Read from the [`IdentitySource`], not derived from the files.
    pub(crate) from_source: bool,
}

pub(crate) type AliasCache = Arc<std::sync::Mutex<HashMap<String, (Instant, Identity)>>>;

fn storage(e: impl std::fmt::Display) -> QueryError {
    QueryError::Storage(e.to_string())
}

/// Run `sql` and keep its rows.
async fn collect(ctx: &SessionContext, sql: &str) -> Result<Vec<RecordBatch>, QueryError> {
    ctx.sql(sql)
        .await
        .map_err(storage)?
        .collect()
        .await
        .map_err(storage)
}

fn alias_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("anon", DataType::Utf8, true),
        Field::new("person", DataType::Utf8, true),
    ]))
}

fn persons_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("person", DataType::Utf8, false),
        Field::new(
            "first_seen",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new("internal", DataType::Boolean, true),
        Field::new(
            "signed_up",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
    ]))
}

/// The `persons` rows of the batches [`PERSONS_SQL`] returns.
fn person_rows(batches: &[RecordBatch]) -> Vec<PersonRow> {
    let mut out = Vec::new();
    for b in batches {
        let (Some(p), Some(f), Some(i)) = (
            b.column_by_name("person"),
            b.column_by_name("first_seen"),
            b.column_by_name("internal"),
        ) else {
            continue;
        };
        let (Some(p), Some(f), Some(i)) = (
            p.as_string_opt::<i32>(),
            f.as_any().downcast_ref::<TimestampMicrosecondArray>(),
            i.as_any().downcast_ref::<BooleanArray>(),
        ) else {
            continue;
        };
        let s = b
            .column_by_name("signed_up")
            .and_then(|s| s.as_any().downcast_ref::<TimestampMicrosecondArray>());
        // A creation date in the future is none (#707, D-111).
        let now = chrono::Utc::now().timestamp_micros();
        for n in 0..b.num_rows() {
            if p.is_null(n) || f.is_null(n) {
                continue;
            }
            out.push(PersonRow {
                person: p.value(n).to_owned(),
                first_seen: f.value(n),
                internal: !i.is_null(n) && i.value(n),
                signed_up: s
                    .filter(|s| !s.is_null(n))
                    .map(|s| s.value(n))
                    .filter(|t| *t <= now),
            });
        }
    }
    out
}

/// Build the in-memory tables and lookups from the rows.
fn build(rows: &IdentityRows, from_source: bool) -> Result<Identity, QueryError> {
    let anon: StringArray = rows.aliases.iter().map(|(a, _)| Some(a.as_str())).collect();
    let person: StringArray = rows.aliases.iter().map(|(_, p)| Some(p.as_str())).collect();
    let schema = alias_schema();
    let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(anon), Arc::new(person)])
        .map_err(storage)?;
    let aliases = MemTable::try_new(schema, vec![vec![batch]]).map_err(storage)?;

    let names: StringArray = rows
        .persons
        .iter()
        .map(|p| Some(p.person.as_str()))
        .collect();
    let first: TimestampMicrosecondArray = rows
        .persons
        .iter()
        .map(|p| Some(p.first_seen))
        .collect::<TimestampMicrosecondArray>()
        .with_timezone("UTC");
    let flags: BooleanArray = rows.persons.iter().map(|p| Some(p.internal)).collect();
    let signed: TimestampMicrosecondArray = rows
        .persons
        .iter()
        .map(|p| p.signed_up)
        .collect::<TimestampMicrosecondArray>()
        .with_timezone("UTC");
    let schema = persons_schema();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(names),
            Arc::new(first),
            Arc::new(flags),
            Arc::new(signed),
        ],
    )
    .map_err(storage)?;
    let persons = MemTable::try_new(schema, vec![vec![batch]]).map_err(storage)?;

    let internal = Arc::new(
        rows.persons
            .iter()
            .filter(|p| p.internal)
            .map(|p| p.person.clone())
            .collect(),
    );
    let lookup: HashMap<String, String> = rows.aliases.iter().cloned().collect();
    Ok(Identity {
        aliases: Arc::new(aliases),
        persons: Arc::new(persons),
        internal,
        lookup: Arc::new(lookup),
        from_source,
    })
}

impl QueryEngine {
    /// This engine, reading the identity tables from `source` (#450)
    /// instead of deriving them from the files on every cache miss.
    pub fn with_identity(mut self, source: IdentitySource) -> Self {
        self.identity_source = Some(source);
        self
    }

    /// The identity of `project` changed at ingestion: the next query reads
    /// it again instead of waiting for [`ALIAS_TTL`]. Only a copy read from
    /// the source is dropped (a cheap read); one derived from the files is
    /// kept, so this never adds a scan of the events.
    pub fn identity_changed(&self, project: &str) {
        self.identity_gen.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut cache) = self.aliases.lock()
            && cache.get(project).is_some_and(|(_, t)| t.from_source)
        {
            cache.remove(project);
        }
    }

    /// Read `project`'s identity again on the next query, wherever the
    /// cached copy came from (the source just became ready).
    pub fn identity_reload(&self, project: &str) {
        self.identity_gen.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut cache) = self.aliases.lock() {
            cache.remove(project);
        }
    }

    /// How many times the identity tables were derived from the files
    /// (a scan of every `$identify`) since the start.
    pub fn identity_scans(&self) -> u64 {
        self.identity_scans.load(Ordering::Relaxed)
    }

    /// The identity tables of `project`, derived from its files: what the
    /// server stores once, after an upgrade, so that existing data keeps
    /// its merges (#450). Bounded by the engine's memory pool and time
    /// limit.
    pub async fn identity_from_files(&self, project: &str) -> Result<IdentityRows, QueryError> {
        let run = async {
            let ctx = self.session()?;
            self.register_raw(&ctx, project)?;
            self.scan_identity(&ctx).await
        };
        match tokio::time::timeout(self.limits.timeout, run).await {
            Ok(r) => r,
            Err(_) => Err(self.timed_out(self.limits.timeout)),
        }
    }

    /// Run [`ALIAS_SQL`] and [`PERSONS_SQL`] on `ctx` (`raw_events`).
    async fn scan_identity(&self, ctx: &SessionContext) -> Result<IdentityRows, QueryError> {
        self.identity_scans.fetch_add(1, Ordering::Relaxed);
        let aliases = alias_lookup(&collect(ctx, ALIAS_SQL).await?);
        let persons = person_rows(&collect(ctx, PERSONS_SQL).await?);
        let mut aliases: Vec<(String, String)> = aliases.into_iter().collect();
        aliases.sort();
        Ok(IdentityRows { aliases, persons })
    }

    /// The `person_aliases` and `persons` tables of `project`, read at most
    /// every [`ALIAS_TTL`]: from the source when it is ready, else from the
    /// files.
    pub(crate) async fn identity(
        &self,
        project: &str,
        ctx: &SessionContext,
    ) -> Result<Identity, QueryError> {
        if let Ok(cache) = self.aliases.lock()
            && let Some((at, t)) = cache.get(project)
            && at.elapsed() < ALIAS_TTL
        {
            return Ok(t.clone());
        }
        // A change made while this read runs must not be hidden behind a
        // copy cached from before it: such a copy is used once, not kept.
        let generation = self.identity_gen.load(Ordering::SeqCst);
        let kept = match &self.identity_source {
            Some(source) => {
                let (f, p) = (source.0.clone(), project.to_owned());
                match tokio::task::spawn_blocking(move || f(&p)).await {
                    Ok(Ok(rows)) => rows,
                    Ok(Err(err)) => {
                        tracing::warn!(%err, "identity tables unreadable; reading the files");
                        None
                    }
                    Err(err) => {
                        tracing::warn!(%err, "identity read crashed; reading the files");
                        None
                    }
                }
            }
            None => None,
        };
        let identity = match kept {
            Some(rows) => build(&rows, true)?,
            None => build(&self.scan_identity(ctx).await?, false)?,
        };
        if let Ok(mut cache) = self.aliases.lock()
            && self.identity_gen.load(Ordering::SeqCst) == generation
        {
            cache.insert(project.to_owned(), (Instant::now(), identity.clone()));
        }
        Ok(identity)
    }
}

/// The scalar function `person_of(distinct_id)`: the user an anonymous id
/// was identified as, or the id itself when it is not an alias.
pub(crate) fn person_of_udf(lookup: Arc<HashMap<String, String>>) -> ScalarUDF {
    create_udf(
        "person_of",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(move |args: &[ColumnarValue]| {
            let arrays = ColumnarValue::values_to_arrays(args)?;
            let ids = arrays
                .first()
                .map(|a| a.as_string::<i32>())
                .ok_or_else(|| DataFusionError::Internal("person_of takes one argument".into()))?;
            let out: StringArray = ids
                .iter()
                .map(|id| id.map(|id| lookup.get(id).map_or(id, String::as_str)))
                .collect();
            Ok(ColumnarValue::Array(Arc::new(out)))
        }),
    )
}

/// Read the `(anon, person)` rows of the alias table into a lookup.
fn alias_lookup(batches: &[RecordBatch]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for b in batches {
        let (Some(anon), Some(person)) = (b.column_by_name("anon"), b.column_by_name("person"))
        else {
            continue;
        };
        let (Some(anon), Some(person)) =
            (anon.as_string_opt::<i32>(), person.as_string_opt::<i32>())
        else {
            continue;
        };
        for (a, p) in anon.iter().zip(person.iter()) {
            if let (Some(a), Some(p)) = (a, p) {
                map.insert(a.to_owned(), p.to_owned());
            }
        }
    }
    map
}

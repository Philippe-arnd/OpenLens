//! The board, computed once per data generation and shared (#291).
//!
//! Computing a board runs about twenty queries over up to 90 days of
//! rollups and events: seconds at a few million events. Every page that
//! shows part of it used to compute it again. Here:
//!
//! - a result (the board and the revenue shown on it, #480) is kept per
//!   (project, filters, map version) and reused until
//!   new data lands in that project (its generation, bumped by that
//!   project's flush, compaction, erasure and retention, #420) and for at
//!   most [`MAX_AGE`] in any case;
//! - after new data, the previous result is still served for [`STALE`] while
//!   one recomputation runs in the background, so a captured event shows on
//!   the board within about 20 s (10 s flush + [`STALE`]);
//! - concurrent requests for the same key wait for the same computation;
//! - boards of different keys are computed side by side: none holds a
//!   copy of its events in memory any more (#502, D-051), so their number
//!   is not capped; only a background rollup pass waits for them, and
//!   they for it ([`BoardCache::exclusive`]);
//! - a date range entirely before today keeps its period blocks past new
//!   data, until a day it read is written ([`crate::closed_days`], #516);
//! - a board that failed is not kept, and the failure is logged (D-071):
//!   an intermittent failure (a query out of memory beside the others)
//!   looked like a cache that never served the second request. A board
//!   that failed for memory is first computed once more, alone (D-074).
//! - the default board page of each project (production, release window,
//!   no role or release) is computed ahead in the background
//!   ([`prewarm`], D-072): after startup, and after new data while its
//!   boards were asked for in the last [`WATCHED`], so the page is served
//!   from the cache instead of computing for seconds.
//! - how long each key took to compute last is kept ([`ready`], D-073),
//!   so a cold board page can say how long it will wait.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openlens_engine::{Board, Filters, Revenue};
use openlens_featuremap::FeatureMap;

use crate::AppState;

/// How long a result outlives new data while it is recomputed.
const STALE: Duration = Duration::from_secs(10);
/// Longest a result is reused, even with no new data (settings may change).
const MAX_AGE: Duration = Duration::from_secs(300);
/// Results kept, all projects and filters together.
const CAPACITY: usize = 64;
/// How long after a project's board was last asked for its default board
/// is still computed ahead of new data (D-072).
const WATCHED: Duration = Duration::from_secs(900);

type Key = (String, String, i64);

/// The revenue of the same filters, or why it could not be computed.
pub(crate) type RevenueResult = Result<Option<Revenue>, String>;

#[derive(Clone)]
struct Entry {
    generation: u64,
    at: Instant,
    board: Arc<Board>,
    /// Computed with the board, so a page that shows both is served from
    /// the cache as a whole (#480).
    revenue: Arc<RevenueResult>,
}

/// Shared board results; cheap to clone.
#[derive(Clone)]
pub struct BoardCache {
    inner: Arc<Inner>,
}

struct Inner {
    /// Data generation per project (#420); unknown projects are at 0.
    generations: Mutex<HashMap<String, u64>>,
    computed: AtomicU64,
    entries: Mutex<HashMap<Key, Entry>>,
    /// One lock per key being computed: the others wait on it.
    flights: Mutex<HashMap<Key, Arc<tokio::sync::Mutex<()>>>>,
    /// Keys refreshed in the background right now.
    refreshing: Mutex<std::collections::HashSet<Key>>,
    /// Read by every board being computed, written by a rollup pass
    /// ([`BoardCache::exclusive`]).
    quiet: tokio::sync::RwLock<()>,
    /// Period blocks of windows of closed days (#516, D-064).
    closed: crate::closed_days::ClosedDays,
    /// When each project's boards were last asked for (D-072).
    viewed: Mutex<HashMap<String, Instant>>,
    /// Projects whose default board was computed ahead once (D-072).
    prewarmed: Mutex<std::collections::HashSet<String>>,
    /// How long each key took to compute last (D-073): kept past its
    /// entry, so a cold page can show the wait to expect.
    took: Mutex<HashMap<Key, Duration>>,
}

impl Default for BoardCache {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                generations: Mutex::new(HashMap::new()),
                computed: AtomicU64::new(0),
                entries: Mutex::new(HashMap::new()),
                flights: Mutex::new(HashMap::new()),
                refreshing: Mutex::new(std::collections::HashSet::new()),
                quiet: tokio::sync::RwLock::new(()),
                closed: crate::closed_days::ClosedDays::default(),
                viewed: Mutex::new(HashMap::new()),
                prewarmed: Mutex::new(std::collections::HashSet::new()),
                took: Mutex::new(HashMap::new()),
            }),
        }
    }
}

impl std::fmt::Debug for BoardCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardCache")
            .field(
                "projects",
                &self.inner.generations.lock().map_or(0, |m| m.len()),
            )
            .field("computed", &self.computed())
            .finish_non_exhaustive()
    }
}

impl BoardCache {
    /// New data landed in `project` (flush, compaction, erasure,
    /// retention, rewrite): only its cached boards go stale.
    pub fn bump(&self, project: &str) {
        if let Ok(mut m) = self.inner.generations.lock() {
            *m.entry(project.to_owned()).or_default() += 1;
        }
    }

    /// The period blocks kept for windows of closed days, and the marks of
    /// the days written since (#516).
    pub fn closed(&self) -> &crate::closed_days::ClosedDays {
        &self.inner.closed
    }

    /// The current data generation of `project`.
    pub fn generation(&self, project: &str) -> u64 {
        self.inner
            .generations
            .lock()
            .map_or(0, |m| m.get(project).copied().unwrap_or(0))
    }

    /// Results kept now.
    pub fn len(&self) -> usize {
        self.inner.entries.lock().map_or(0, |m| m.len())
    }

    /// Whether no result is kept.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Wait until no board is being computed, and keep boards from
    /// starting until the returned guard drops. Background rollups run
    /// under it, so a rollup pass and a board never split the query memory
    /// pool between them (#444). The lock is fair: boards asked for after
    /// this call wait for the guard to drop, and this call is never
    /// starved by a stream of boards.
    pub async fn exclusive(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.inner.quiet.write().await
    }

    /// How many boards were actually computed since the start.
    pub fn computed(&self) -> u64 {
        self.inner.computed.load(Ordering::Relaxed)
    }

    fn get(&self, key: &Key) -> Option<Entry> {
        self.inner.entries.lock().ok()?.get(key).cloned()
    }

    fn put(&self, key: Key, entry: Entry) {
        let Ok(mut map) = self.inner.entries.lock() else {
            return;
        };
        if map.len() >= CAPACITY
            && !map.contains_key(&key)
            && let Some(oldest) = map.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
        map.insert(key, entry);
    }

    fn took(&self, key: &Key) -> Option<Duration> {
        self.inner.took.lock().ok()?.get(key).copied()
    }

    fn set_took(&self, key: Key, d: Duration) {
        let Ok(mut m) = self.inner.took.lock() else {
            return;
        };
        if m.len() >= CAPACITY * 4 && !m.contains_key(&key) {
            m.clear();
        }
        m.insert(key, d);
    }

    fn flight(&self, key: &Key) -> Arc<tokio::sync::Mutex<()>> {
        match self.inner.flights.lock() {
            Ok(mut m) => {
                if m.len() > CAPACITY * 4 {
                    m.retain(|_, l| Arc::strong_count(l) > 1);
                }
                m.entry(key.clone()).or_default().clone()
            }
            Err(_) => Arc::default(),
        }
    }
}

fn fresh(e: &Entry, generation: u64) -> bool {
    e.at.elapsed() < MAX_AGE && e.generation == generation
}

fn servable(e: &Entry, generation: u64) -> bool {
    fresh(e, generation) || (e.at.elapsed() < STALE && e.at.elapsed() < MAX_AGE)
}

/// The cache key of a board.
fn key_of(project: &str, f: &Filters, map: Option<(&FeatureMap, i64)>) -> Key {
    (
        project.to_owned(),
        format!("{f:?}"),
        map.map_or(0, |(_, v)| v),
    )
}

/// Compute the default board page of each project ahead (#452, D-072):
/// production, the release window (D-055), no role or release, in the
/// project's zone, as `GET /p/{id}` asks for it. A project's board is
/// computed once after startup, then after new data only while its boards
/// were asked for in the last [`WATCHED`]. It is skipped while a rollup
/// pass runs ([`BoardCache::exclusive`]) or while that board is already
/// being computed, and when it is fresh; the next call tries again.
/// Projects are taken one at a time. Run every few seconds by the
/// background jobs.
pub(crate) async fn prewarm(state: &AppState) {
    let meta = state.meta.clone();
    let projects = match tokio::task::spawn_blocking(move || meta.projects()).await {
        Ok(Ok(p)) => p,
        Ok(Err(err)) => {
            tracing::warn!(%err, "default boards not computed ahead");
            return;
        }
        Err(err) => {
            tracing::warn!(%err, "default boards not computed ahead");
            return;
        }
    };
    let cache = &state.boards;
    for p in projects {
        let once = cache
            .inner
            .prewarmed
            .lock()
            .is_ok_and(|s| s.contains(&p.id));
        let watched = cache
            .inner
            .viewed
            .lock()
            .ok()
            .and_then(|m| m.get(&p.id).copied())
            .is_some_and(|t| t.elapsed() < WATCHED);
        if once && !watched {
            continue;
        }
        let Ok(f) = crate::board_query::BoardQuery::default().filters(&p.timezone) else {
            continue;
        };
        let map = crate::load_map(state, &p.id).await;
        let m = map.as_ref().map(|(m, v)| (m, *v));
        let key = key_of(&p.id, &f, m);
        let generation = cache.generation(&p.id);
        let done = |cache: &BoardCache| {
            if let Ok(mut s) = cache.inner.prewarmed.lock() {
                s.insert(p.id.clone());
            }
        };
        if cache.get(&key).is_some_and(|e| fresh(&e, generation)) {
            done(cache);
            continue;
        }
        // Never beside a rollup pass: it has the lock, or waits for it.
        if cache.inner.quiet.try_read().is_err() {
            continue;
        }
        // Already being computed (a page, a refresh): nothing to add.
        let lock = cache.flight(&key);
        let Ok(_computing) = lock.try_lock() else {
            continue;
        };
        if let Err(err) = compute(state, &p.id, &f, m, generation, key).await {
            tracing::warn!(project = %p.id, %err, "default board not computed ahead");
        }
        done(cache);
    }
}

/// Whether a board page is served from the cache, and if not, how long
/// its board took to compute last time (D-073).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ready {
    /// Served from the cache now, without computing.
    pub hit: bool,
    /// The last compute time of this board, when it was computed before.
    pub estimate: Option<Duration>,
}

/// Whether the board for these filters can be served from the cache now,
/// and the last time it took to compute (#301: a cold one streams the page
/// shell first; D-073: with a waiting panel when it may last).
pub(crate) fn ready(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> Ready {
    let key = key_of(project, f, map);
    let generation = state.boards.generation(project);
    Ready {
        hit: state
            .boards
            .get(&key)
            .is_some_and(|e| servable(&e, generation)),
        estimate: state.boards.took(&key),
    }
}

/// Whether the board for these filters can be served from the cache now,
/// without computing.
pub(crate) fn warm(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> bool {
    ready(state, project, f, map).hit
}

/// The board of `project` for these filters, from the cache when possible,
/// with the environments the project hides left out of its selector.
pub(crate) async fn board(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> openlens_engine::Result<Board> {
    Ok(with_revenue(state, project, f, map).await?.0)
}

/// [`board`] and the revenue of the same filters (#231), from the cache
/// when possible: the revenue query reads 30 days of properties, so
/// running it on every board page made a cached board page as slow as a
/// cold one (#480).
pub(crate) async fn with_revenue(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> openlens_engine::Result<(Board, RevenueResult)> {
    let e = cached(state, project, f, map).await?;
    let mut b = (*e.board).clone();
    b.environments = crate::environments::visible(
        state,
        project,
        std::mem::take(&mut b.environments),
        &f.environment,
    );
    Ok((b, (*e.revenue).clone()))
}

async fn cached(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> openlens_engine::Result<Entry> {
    let cache = &state.boards;
    let key = key_of(project, f, map);
    if let Ok(mut m) = cache.inner.viewed.lock() {
        if m.len() > CAPACITY * 4 {
            m.retain(|_, t| t.elapsed() < WATCHED);
        }
        m.insert(project.to_owned(), Instant::now());
    }
    let generation = cache.generation(project);
    if let Some(e) = cache.get(&key)
        && servable(&e, generation)
    {
        if !fresh(&e, generation) {
            refresh_later(state, key, f.clone(), map.map(|(m, v)| (m.clone(), v)));
        }
        return Ok(e);
    }
    let lock = cache.flight(&key);
    let _computing = lock.lock().await;
    // Someone else may have computed it while we waited.
    let generation = cache.generation(project);
    if let Some(e) = cache.get(&key)
        && servable(&e, generation)
    {
        return Ok(e);
    }
    compute(state, project, f, map, generation, key).await
}

async fn compute(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
    generation: u64,
    key: Key,
) -> openlens_engine::Result<Entry> {
    let cache = &state.boards;
    let _quiet = cache.inner.quiet.read().await;
    // A date range before today: its period blocks may be kept (#516).
    let closed = f.window.closed_utc_days(&f.tz, chrono::Utc::now());
    let kept = closed.and_then(|days| cache.inner.closed.get(&key, days));
    let since = cache.inner.closed.mark();
    let started = Instant::now();
    let (board, revenue) = tokio::join!(
        Box::pin(openlens_engine::board_reusing(
            &state.query,
            project,
            f,
            map,
            kept.as_deref()
        )),
        openlens_engine::revenue(&state.query, project, map.map(|(m, _)| m), f)
    );
    let revenue = revenue.map_err(|err| {
        tracing::warn!(project, %err, "revenue not computed");
        err.to_string()
    });
    // A query that still ran out of memory (alone, or again after the
    // others, D-071) fails the board: it is computed once more, alone,
    // once every other board is done and before any new one starts
    // (D-074).
    let board = match board {
        Err(err) if out_of_memory(&err) => {
            tracing::info!(project, %err, "board ran out of memory; computing it again alone");
            drop(_quiet);
            let _alone = cache.exclusive().await;
            Box::pin(openlens_engine::board_reusing(
                &state.query,
                project,
                f,
                map,
                kept.as_deref(),
            ))
            .await
        }
        other => other,
    };
    if kept.is_none()
        && let (Some(days), Ok(b)) = (closed, &board)
    {
        let blocks = openlens_engine::PeriodBlocks::of(b);
        cache.inner.closed.put(key.clone(), days, since, blocks);
    }
    // A board that failed is not kept: the next page computes it again.
    // Said in the log, since the page alone shows it (D-071).
    let board = board.inspect_err(|err| {
        tracing::warn!(project, %err, "board not computed; not cached");
    })?;
    let entry = Entry {
        generation,
        at: Instant::now(),
        board: Arc::new(board),
        revenue: Arc::new(revenue),
    };
    cache.inner.computed.fetch_add(1, Ordering::Relaxed);
    let took = started.elapsed();
    cache.set_took(key.clone(), took);
    // How long a board took, computed for a page or ahead (D-072): the
    // `scale` job reads it, since the page may be served from the cache.
    tracing::info!(
        project,
        ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
        key = ?key,
        "board computed"
    );
    cache.put(key, entry.clone());
    Ok(entry)
}

/// Whether a board failed because a query needed more memory than the
/// pool gave it (D-074).
fn out_of_memory(err: &openlens_engine::EngineError) -> bool {
    match err {
        openlens_engine::EngineError::Query(openlens_query::QueryError::TooBig) => true,
        // Should `friendly` have missed it, DataFusion's own words.
        openlens_engine::EngineError::Query(
            openlens_query::QueryError::Rejected(m) | openlens_query::QueryError::Storage(m),
        ) => m.contains("Resources exhausted"),
        _ => false,
    }
}

/// Recompute a stale result in the background, once per key at a time.
fn refresh_later(state: &AppState, key: Key, f: Filters, map: Option<(FeatureMap, i64)>) {
    let Ok(mut busy) = state.boards.inner.refreshing.lock() else {
        return;
    };
    if !busy.insert(key.clone()) {
        return;
    }
    drop(busy);
    let state = state.clone();
    tokio::spawn(async move {
        let lock = state.boards.flight(&key);
        let _computing = lock.lock().await;
        let project = key.0.clone();
        let generation = state.boards.generation(&project);
        let done = match state.boards.get(&key) {
            Some(e) if fresh(&e, generation) => Ok(()),
            _ => compute(
                &state,
                &project,
                &f,
                map.as_ref().map(|(m, v)| (m, *v)),
                generation,
                key.clone(),
            )
            .await
            .map(|_| ()),
        };
        if let Err(err) = done {
            tracing::warn!(project, %err, "board refresh failed");
        }
        if let Ok(mut busy) = state.boards.inner.refreshing.lock() {
            busy.remove(&key);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::BoardCache;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_default_board_is_computed_ahead_once_never_beside_a_rollup_pass() {
        let dir = tempfile::tempdir().unwrap();
        let meta = openlens_meta::Meta::in_memory().unwrap();
        let p = meta.create_project("Ahead", None).unwrap();
        let (writer, _) =
            openlens_storage::Writer::open(dir.path(), openlens_storage::FlushPolicy::default())
                .unwrap();
        let state = crate::AppState {
            meta: Arc::new(meta),
            writer: Arc::new(Mutex::new(writer)),
            query: openlens_query::QueryEngine::new(dir.path(), openlens_query::Limits::default()),
            data_dir: dir.path().to_path_buf(),
            admin_token_hash: "x".into(),
            session_key: "k".into(),
            boards: Default::default(),
            flushing: Default::default(),
            metrics: Default::default(),
            disk_space: crate::disk_space,
            session_gen: Default::default(),
            caches: Default::default(),
        };
        // A rollup pass runs: the board waits for the next call.
        {
            let _pass = state.boards.exclusive().await;
            super::prewarm(&state).await;
            assert_eq!(state.boards.computed(), 0);
        }
        super::prewarm(&state).await;
        assert_eq!(state.boards.computed(), 1);
        // The board page's own request is served from the cache.
        let f = crate::board_query::BoardQuery::default()
            .filters(&p.timezone)
            .unwrap();
        assert!(super::warm(&state, &p.id, &f, None));
        // New data, but nobody asked for the board since: not again.
        state.boards.bump(&p.id);
        super::prewarm(&state).await;
        assert_eq!(state.boards.computed(), 1);
        // Asked for lately (as `cached` marks it): computed ahead of the
        // new data.
        state
            .boards
            .inner
            .viewed
            .lock()
            .unwrap()
            .insert(p.id.clone(), std::time::Instant::now());
        super::prewarm(&state).await;
        assert_eq!(state.boards.computed(), 2);
        assert!(super::warm(&state, &p.id, &f, None));
        // Its compute time is kept, for the next cold page (D-073).
        let r = super::ready(&state, &p.id, &f, None);
        assert!(r.hit && r.estimate.is_some(), "{r:?}");
    }

    /// D-074: only a board out of memory is computed again, alone.
    #[test]
    fn only_a_board_out_of_memory_is_computed_again() {
        use openlens_engine::EngineError;
        use openlens_query::QueryError;
        assert!(super::out_of_memory(&EngineError::Query(QueryError::TooBig)));
        assert!(super::out_of_memory(&EngineError::Query(
            QueryError::Rejected("Resources exhausted: Failed to allocate".into())
        )));
        assert!(!super::out_of_memory(&EngineError::Query(
            QueryError::Rejected("syntax error".into())
        )));
        assert!(!super::out_of_memory(&EngineError::Query(QueryError::Busy)));
        assert!(!super::out_of_memory(&EngineError::InvalidFilter("role")));
    }

    #[test]
    fn a_bump_only_ages_its_own_project() {
        let c = BoardCache::default();
        assert_eq!(c.generation("a"), 0);
        c.bump("a");
        c.bump("a");
        assert_eq!(c.generation("a"), 2);
        assert_eq!(c.generation("b"), 0, "project b is untouched");
    }
}

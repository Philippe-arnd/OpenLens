//! The board, computed once per data generation and shared (#291).
//!
//! Computing a board reads up to 90 days of events and runs about twenty
//! queries: seconds, and hundreds of MB at a few million events. Every page
//! that shows part of it used to compute it again. Here:
//!
//! - a result is kept per (project, filters, map version) and reused until
//!   new data lands in that project (its generation, bumped by that
//!   project's flush, compaction, erasure and retention, #420) and for at
//!   most [`MAX_AGE`] in any case;
//! - after new data, the previous result is still served for [`STALE`] while
//!   one recomputation runs in the background, so a captured event shows on
//!   the board within about 20 s (10 s flush + [`STALE`]);
//! - concurrent requests for the same key wait for the same computation;
//! - at most [`PARALLEL`] boards are computed at once, whatever the keys.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openlens_engine::{Board, Filters};
use openlens_featuremap::FeatureMap;

use crate::AppState;

/// How long a result outlives new data while it is recomputed.
const STALE: Duration = Duration::from_secs(10);
/// Longest a result is reused, even with no new data (settings may change).
const MAX_AGE: Duration = Duration::from_secs(300);
/// Results kept, all projects and filters together.
const CAPACITY: usize = 64;
/// Boards computed at the same time.
const PARALLEL: usize = 2;

type Key = (String, String, i64);

#[derive(Clone)]
struct Entry {
    generation: u64,
    at: Instant,
    board: Arc<Board>,
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
    permits: tokio::sync::Semaphore,
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
                permits: tokio::sync::Semaphore::new(PARALLEL),
            }),
        }
    }
}

impl std::fmt::Debug for BoardCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardCache")
            .field("projects", &self.inner.generations.lock().map_or(0, |m| m.len()))
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

/// Whether the board for these filters can be served from the cache now,
/// without computing (#301: a cold one streams the page shell first).
pub(crate) fn warm(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> bool {
    let key: Key = (
        project.to_owned(),
        format!("{f:?}"),
        map.map_or(0, |(_, v)| v),
    );
    let generation = state.boards.generation(project);
    state
        .boards
        .get(&key)
        .is_some_and(|e| servable(&e, generation))
}

/// The board of `project` for these filters, from the cache when possible.
pub(crate) async fn board(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> openlens_engine::Result<Board> {
    let cache = &state.boards;
    let key: Key = (
        project.to_owned(),
        format!("{f:?}"),
        map.map_or(0, |(_, v)| v),
    );
    let generation = cache.generation(project);
    if let Some(e) = cache.get(&key)
        && servable(&e, generation)
    {
        if !fresh(&e, generation) {
            refresh_later(state, key, f.clone(), map.map(|(m, v)| (m.clone(), v)));
        }
        return Ok((*e.board).clone());
    }
    let lock = cache.flight(&key);
    let _computing = lock.lock().await;
    // Someone else may have computed it while we waited.
    let generation = cache.generation(project);
    if let Some(e) = cache.get(&key)
        && servable(&e, generation)
    {
        return Ok((*e.board).clone());
    }
    let board = compute(state, project, f, map, generation, key).await?;
    Ok((*board).clone())
}

async fn compute(
    state: &AppState,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
    generation: u64,
    key: Key,
) -> openlens_engine::Result<Arc<Board>> {
    let cache = &state.boards;
    let _permit = cache.inner.permits.acquire().await.ok();
    let board = Arc::new(Box::pin(openlens_engine::board(&state.query, project, f, map)).await?);
    cache.inner.computed.fetch_add(1, Ordering::Relaxed);
    cache.put(
        key,
        Entry {
            generation,
            at: Instant::now(),
            board: board.clone(),
        },
    );
    Ok(board)
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
    use super::BoardCache;

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

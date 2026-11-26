//! The server's in-memory state outside the stores (#309): one [`Caches`]
//! in `AppState` instead of statics, so each test gets its own, `/metrics`
//! can count them, and every one has a hard size limit.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A map with a capacity and a time to live. Expired entries are never
/// returned, and inserting past the capacity drops the oldest entries.
pub(crate) struct Bounded<K, V> {
    cap: usize,
    ttl: Duration,
    inner: Mutex<Inner<K, V>>,
}

struct Inner<K, V> {
    map: HashMap<K, (Instant, V)>,
    /// Keys by insertion time; a key re-inserted appears again (the stale
    /// copy is skipped when it reaches the front).
    order: VecDeque<(Instant, K)>,
}

impl<K: Eq + Hash + Clone, V> Bounded<K, V> {
    pub(crate) fn new(cap: usize, ttl: Duration) -> Self {
        Self {
            cap: cap.max(1),
            ttl,
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    fn evict(&self, g: &mut Inner<K, V>, now: Instant) {
        while let Some((at, k)) = g.order.front().cloned() {
            let current = g.map.get(&k).map(|(t, _)| *t);
            if current != Some(at) {
                // Re-inserted since, or removed: a stale copy.
                g.order.pop_front();
            } else if g.map.len() > self.cap || now.duration_since(at) >= self.ttl {
                g.order.pop_front();
                g.map.remove(&k);
            } else {
                break;
            }
        }
        // Re-inserting one key over and over must not grow the queue.
        if g.order.len() > 2 * self.cap + 16 {
            let mut live: Vec<(Instant, K)> =
                g.map.iter().map(|(k, (t, _))| (*t, k.clone())).collect();
            live.sort_by_key(|(t, _)| *t);
            g.order = live.into();
        }
    }

    /// Store `v` under `k` (fresh for the time to live).
    pub(crate) fn insert(&self, k: K, v: V) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        let now = Instant::now();
        g.order.push_back((now, k.clone()));
        g.map.insert(k, (now, v));
        self.evict(&mut g, now);
    }

    /// Take the value out, if it is there and fresh.
    pub(crate) fn remove(&self, k: &K) -> Option<V> {
        let mut g = self.inner.lock().ok()?;
        let (at, v) = g.map.remove(k)?;
        (at.elapsed() < self.ttl).then_some(v)
    }

    /// Change the value under `k` in place (`None`: absent or expired) and
    /// return what `f` says; `f` returning `None` for the value removes it.
    pub(crate) fn update<R>(&self, k: K, f: impl FnOnce(Option<V>) -> (Option<V>, R)) -> Option<R> {
        let mut g = self.inner.lock().ok()?;
        let now = Instant::now();
        let old = g
            .map
            .remove(&k)
            .filter(|(at, _)| now.duration_since(*at) < self.ttl)
            .map(|(_, v)| v);
        let (new, out) = f(old);
        if let Some(v) = new {
            g.order.push_back((now, k.clone()));
            g.map.insert(k, (now, v));
        }
        self.evict(&mut g, now);
        Some(out)
    }

    /// Drop everything.
    pub(crate) fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.map.clear();
            g.order.clear();
        }
    }

    /// Entries held now (expired ones included until evicted).
    pub(crate) fn len(&self) -> usize {
        self.inner.lock().map_or(0, |g| g.map.len())
    }
}

impl<K: Eq + Hash + Clone, V: Clone> Bounded<K, V> {
    /// A copy of the value, if it is there and fresh.
    pub(crate) fn get(&self, k: &K) -> Option<V> {
        let g = self.inner.lock().ok()?;
        g.map
            .get(k)
            .filter(|(at, _)| at.elapsed() < self.ttl)
            .map(|(_, v)| v.clone())
    }
}

/// Every in-memory cache and limiter of the server (#309).
pub struct Caches {
    /// Ask: changes proposed by the model, waiting for a click (30 min).
    pub(crate) ask_pending: Bounded<String, crate::ask::Pending>,
    /// Ask: questions per user in the last hour.
    pub(crate) ask_rate: Bounded<String, Vec<Instant>>,
    /// Ask: the project brief and the map version it was built from (a day).
    pub(crate) ask_briefs: Bounded<String, (i64, String)>,
    /// Ask: the last request body sent to the model, per project.
    pub(crate) ask_last_sent: Bounded<String, serde_json::Value>,
    /// Ingestion: request tokens per client address.
    pub(crate) ip_buckets: Mutex<crate::capture::Buckets>,
    /// Ingestion: event tokens per project key.
    pub(crate) key_buckets: Mutex<crate::capture::Buckets>,
    /// When "disk almost full" was last logged.
    pub(crate) disk_logged: Mutex<Option<Instant>>,
    /// Cookieless mode: visitor → (session id, last seen, Unix s).
    pub(crate) cookieless: Bounded<String, (String, i64)>,
    /// Icons being fetched now.
    pub(crate) icons_in_flight: Bounded<String, ()>,
    /// OIDC: the provider's endpoints per issuer (an hour).
    pub(crate) oidc: Bounded<String, crate::oidc::Discovery>,
    /// Reworded insight cards (`None`: being asked) per card set.
    pub(crate) reword: Bounded<String, Option<Vec<crate::reword::Plain>>>,
    /// Rendered share-link pages (5 minutes).
    pub(crate) share_pages: Bounded<String, String>,
    /// Login emails: sends in the last hour, and per address the last one.
    pub(crate) login_emails: Bounded<String, Vec<Instant>>,
    /// Environment host rules per project (a minute; cleared on change).
    pub(crate) env_rules: Bounded<String, std::sync::Arc<crate::environments::ProjectEnv>>,
    /// Projects whose environment upgrade seed is running now.
    pub(crate) env_seeding: Bounded<String, ()>,
    /// The last two probes of each monitored environment (#410), keyed
    /// `project|env`.
    pub(crate) env_checks: Bounded<String, Vec<bool>>,
    /// Replay rate per project (#327): counted at most once a day.
    pub(crate) replay_rates: Bounded<String, crate::replay::RateEntry>,
    /// Projects whose identity backfill (#450) ran or is running.
    pub(crate) identity_checked: Bounded<String, ()>,
}

impl Default for Caches {
    fn default() -> Self {
        const MIN: Duration = Duration::from_secs(60);
        Self {
            ask_pending: Bounded::new(1_000, 30 * MIN),
            ask_rate: Bounded::new(10_000, 60 * MIN),
            ask_briefs: Bounded::new(1_000, 24 * 60 * MIN),
            ask_last_sent: Bounded::new(1_000, 7 * 24 * 60 * MIN),
            ip_buckets: Mutex::default(),
            key_buckets: Mutex::default(),
            disk_logged: Mutex::default(),
            cookieless: Bounded::new(100_000, 30 * MIN),
            icons_in_flight: Bounded::new(10_000, 5 * MIN),
            oidc: Bounded::new(16, 60 * MIN),
            reword: Bounded::new(1_000, crate::reword::TTL),
            share_pages: Bounded::new(1_000, 5 * MIN),
            login_emails: Bounded::new(10_000, 60 * MIN),
            env_rules: Bounded::new(10_000, MIN),
            env_seeding: Bounded::new(10_000, 10 * MIN),
            env_checks: Bounded::new(10_000, 10 * MIN),
            replay_rates: Bounded::new(10_000, 25 * 60 * MIN),
            identity_checked: Bounded::new(10_000, 10 * MIN),
        }
    }
}

impl Caches {
    /// Entries per cache, for `/metrics`.
    pub(crate) fn sizes(&self) -> Vec<(&'static str, u64)> {
        let n = |x: usize| x as u64;
        let buckets = |m: &Mutex<crate::capture::Buckets>| m.lock().map_or(0, |b| b.len());
        vec![
            ("ask_pending", n(self.ask_pending.len())),
            ("ask_rate", n(self.ask_rate.len())),
            ("ask_briefs", n(self.ask_briefs.len())),
            ("ask_last_sent", n(self.ask_last_sent.len())),
            ("ip_buckets", n(buckets(&self.ip_buckets))),
            ("key_buckets", n(buckets(&self.key_buckets))),
            ("cookieless", n(self.cookieless.len())),
            ("icons_in_flight", n(self.icons_in_flight.len())),
            ("oidc", n(self.oidc.len())),
            ("reword", n(self.reword.len())),
            ("share_pages", n(self.share_pages.len())),
            ("login_emails", n(self.login_emails.len())),
            ("env_rules", n(self.env_rules.len())),
            ("env_checks", n(self.env_checks.len())),
            ("replay_rates", n(self.replay_rates.len())),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Bounded;

    #[test]
    fn a_bounded_cache_never_exceeds_its_capacity() {
        let c = Bounded::new(100, Duration::from_secs(60));
        for i in 0..10_000 {
            c.insert(i, i);
            assert!(c.len() <= 100);
        }
        assert_eq!(c.get(&9_999), Some(9_999), "the newest stay");
        assert_eq!(c.get(&0), None, "the oldest went first");
        // One key re-inserted many times keeps the queue short too.
        for _ in 0..10_000 {
            c.insert(5, 5);
        }
        assert!(c.inner.lock().unwrap().order.len() <= 2 * 100 + 16);
    }

    #[test]
    fn expired_entries_are_never_returned() {
        let c = Bounded::new(10, Duration::from_millis(30));
        c.insert("k", 1);
        assert_eq!(c.get(&"k"), Some(1));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.get(&"k"), None);
        assert_eq!(c.update("k", |old| (old.map(|v| v + 1), ())), Some(()));
        assert_eq!(c.get(&"k"), None, "an expired value is not carried over");
    }
}

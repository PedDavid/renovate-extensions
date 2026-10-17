//! A small in-memory cache for upstream documents: entries live for a TTL,
//! concurrent misses for a key wait on one fetch, and when a refresh fails
//! the last good value is served (marked stale) rather than an error.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jiff::Timestamp;

/// A cached value and where it stands.
#[derive(Debug)]
pub struct Cached<V> {
    pub value: Arc<V>,
    pub fetched_at: Timestamp,
    /// Why the latest refresh failed, when `value` is an older copy.
    pub stale: Option<String>,
}

impl<V> Clone for Cached<V> {
    fn clone(&self) -> Self {
        Cached {
            value: self.value.clone(),
            fetched_at: self.fetched_at,
            stale: self.stale.clone(),
        }
    }
}

struct Entry<V> {
    value: Arc<V>,
    fetched_at: Timestamp,
    expires: Instant,
}

type Slot<V> = Arc<tokio::sync::Mutex<Option<Entry<V>>>>;

pub struct Cache<V> {
    ttl: Duration,
    /// Entries not asked for this long after they expire are dropped.
    forget_after: Option<Duration>,
    slots: Mutex<HashMap<String, Slot<V>>>,
}

impl<V> Cache<V> {
    pub fn new(ttl: Duration) -> Self {
        Cache {
            ttl,
            forget_after: None,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// For open-ended keys: entries left unused for `after` past their
    /// expiry are dropped, so the cache doesn't grow forever.
    pub fn forgetting_after(mut self, after: Duration) -> Self {
        self.forget_after = Some(after);
        self
    }

    /// Drops entries that expired more than `forget_after` ago. Slots in
    /// use (locked) are kept.
    fn forget_old(&self, slots: &mut HashMap<String, Slot<V>>) {
        let Some(after) = self.forget_after else {
            return;
        };
        let now = Instant::now();
        slots.retain(|_, slot| match slot.try_lock() {
            Ok(entry) => entry.as_ref().is_some_and(|e| e.expires + after > now),
            Err(_) => true,
        });
    }

    pub fn len(&self) -> usize {
        self.slots.lock().expect("cache lock poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The value for `key`, fetched when missing or expired. `fetch` gets the
    /// previous value, if any, to make a conditional request with.
    pub async fn get<E, F, Fut>(&self, key: &str, fetch: F) -> Result<Cached<V>, E>
    where
        E: std::fmt::Display,
        F: FnOnce(Option<Arc<V>>) -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        let slot = {
            let mut slots = self.slots.lock().expect("cache lock poisoned");
            if !slots.contains_key(key) {
                self.forget_old(&mut slots);
            }
            slots.entry(key.to_string()).or_default().clone()
        };
        let mut entry = slot.lock().await;
        if let Some(e) = entry.as_ref()
            && e.expires > Instant::now()
        {
            return Ok(Cached {
                value: e.value.clone(),
                fetched_at: e.fetched_at,
                stale: None,
            });
        }
        match fetch(entry.as_ref().map(|e| e.value.clone())).await {
            Ok(value) => {
                let fresh = Entry {
                    value: Arc::new(value),
                    fetched_at: Timestamp::now(),
                    expires: Instant::now() + self.ttl,
                };
                let cached = Cached {
                    value: fresh.value.clone(),
                    fetched_at: fresh.fetched_at,
                    stale: None,
                };
                *entry = Some(fresh);
                Ok(cached)
            }
            Err(e) => match entry.as_ref() {
                Some(old) => {
                    tracing::warn!(key, error = %e, "refresh failed, serving the cached copy");
                    Ok(Cached {
                        value: old.value.clone(),
                        fetched_at: old.fetched_at,
                        stale: Some(e.to_string()),
                    })
                }
                None => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn fetches_once_within_the_ttl() {
        let cache = Cache::new(Duration::from_secs(60));
        let calls = AtomicUsize::new(0);
        for _ in 0..3 {
            let v = cache
                .get("k", |_| async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(1)
                })
                .await
                .unwrap();
            assert_eq!(*v.value, 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_misses_share_one_fetch() {
        let cache = Arc::new(Cache::new(Duration::from_secs(60)));
        let calls = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let (cache, calls) = (cache.clone(), calls.clone());
                tokio::spawn(async move {
                    cache
                        .get("k", |_| async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            Ok::<_, String>(1)
                        })
                        .await
                        .unwrap()
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn forgets_entries_long_unused() {
        let cache = Cache::new(Duration::ZERO).forgetting_after(Duration::ZERO);
        cache
            .get("a", |_| async { Ok::<_, String>(1) })
            .await
            .unwrap();
        cache
            .get("b", |_| async { Ok::<_, String>(2) })
            .await
            .unwrap();
        // Adding "b" swept "a", which had expired.
        assert_eq!(cache.len(), 1);
        let kept = Cache::new(Duration::from_secs(60)).forgetting_after(Duration::ZERO);
        kept.get("a", |_| async { Ok::<_, String>(1) })
            .await
            .unwrap();
        kept.get("b", |_| async { Ok::<_, String>(2) })
            .await
            .unwrap();
        assert_eq!(kept.len(), 2);
    }

    #[tokio::test]
    async fn serves_the_old_value_when_a_refresh_fails() {
        let cache = Cache::new(Duration::ZERO);
        cache
            .get("k", |_| async { Ok::<_, String>(1) })
            .await
            .unwrap();
        let v = cache
            .get("k", |prev| async move {
                assert_eq!(prev.as_deref(), Some(&1));
                Err::<i32, _>("down".to_string())
            })
            .await
            .unwrap();
        assert_eq!(*v.value, 1);
        assert_eq!(v.stale.as_deref(), Some("down"));
        // Nothing cached yet: the error comes through.
        assert!(
            cache
                .get("other", |_| async { Err::<i32, _>("down".to_string()) })
                .await
                .is_err()
        );
    }
}

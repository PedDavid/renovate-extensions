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
    slots: Mutex<HashMap<String, Slot<V>>>,
}

impl<V> Cache<V> {
    pub fn new(ttl: Duration) -> Self {
        Cache {
            ttl,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// The value for `key`, fetched when missing or expired. `fetch` gets the
    /// previous value, if any, to make a conditional request with.
    pub async fn get<E, F, Fut>(&self, key: &str, fetch: F) -> Result<Cached<V>, E>
    where
        E: std::fmt::Display,
        F: FnOnce(Option<Arc<V>>) -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        let slot = self
            .slots
            .lock()
            .expect("cache lock poisoned")
            .entry(key.to_string())
            .or_default()
            .clone();
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

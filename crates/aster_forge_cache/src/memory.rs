//! In-memory cache backend implementation.
//!
//! The backend is intended for local development, tests, and fallback paths where durability is not
//! required. It layers explicit per-entry expiration and a reservation set over `moka` so common
//! cache operations keep the same semantics as the Redis backend.

use super::{CacheBackend, Result, reservation::ReservationSet};
use async_trait::async_trait;
use moka::future::Cache;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MEMORY_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// In-process cache backend backed by `moka`.
pub struct MemoryCache {
    cache: Cache<String, MemoryCacheValue>,
    default_ttl: u64,
    reservations: ReservationSet,
}

#[derive(Clone)]
struct MemoryCacheValue {
    bytes: Vec<u8>,
    expires_at: Instant,
}

impl MemoryCacheValue {
    fn new(bytes: Vec<u8>, ttl_secs: u64) -> Self {
        let now = Instant::now();
        Self {
            bytes,
            expires_at: now
                .checked_add(Duration::from_secs(ttl_secs))
                .unwrap_or(now),
        }
    }

    fn is_expired(&self) -> bool {
        self.expires_at <= Instant::now()
    }
}

/// Bridges each value's absolute `expires_at` into moka's per-entry expiration policy.
///
/// A builder-level `time_to_live(default_ttl)` would apply one duration to every entry and
/// silently evict entries whose per-entry TTL is longer than the default (and make the whole
/// cache write-only when `default_ttl` is 0). Computing the duration from the value's own
/// `expires_at` keeps per-entry TTLs exact, matching the Redis backend's SETEX semantics.
struct MemoryCacheExpiry;

impl moka::Expiry<String, MemoryCacheValue> for MemoryCacheExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &MemoryCacheValue,
        created_at: Instant,
    ) -> Option<Duration> {
        Some(value.expires_at.saturating_duration_since(created_at))
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &MemoryCacheValue,
        updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        Some(value.expires_at.saturating_duration_since(updated_at))
    }
}

impl MemoryCache {
    /// Creates a memory cache with the provided default TTL in seconds.
    pub fn new(default_ttl: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(MEMORY_CACHE_MAX_BYTES)
            .weigher(|key: &String, value: &MemoryCacheValue| {
                entry_weight(key.len(), value.bytes.len())
            })
            .expire_after(MemoryCacheExpiry)
            .build();
        Self {
            cache,
            default_ttl,
            reservations: ReservationSet::new(default_ttl),
        }
    }

    fn cache_value(&self, value: Vec<u8>, ttl_secs: Option<u64>) -> MemoryCacheValue {
        MemoryCacheValue::new(value, ttl_secs.unwrap_or(self.default_ttl))
    }
}

fn entry_weight(key_len: usize, value_len: usize) -> u32 {
    let total = key_len.saturating_add(value_len);
    u32::try_from(total).unwrap_or(u32::MAX)
}

#[async_trait]
impl CacheBackend for MemoryCache {
    fn backend_name(&self) -> &'static str {
        "memory"
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    async fn get_bytes(&self, key: &str) -> Option<Vec<u8>> {
        let value = self.cache.get(key).await?;
        if value.is_expired() {
            // The value expired in the sliver between moka's read and ours. moka's
            // per-entry expiry already hides the entry from future reads, so only the
            // reservation co-lifetime needs help here. Do NOT `cache.remove`: between
            // our get and that remove a concurrent `set_bytes` could insert a fresh
            // value, and the remove would silently delete it.
            self.reservations.remove(key);
            return None;
        }
        Some(value.bytes)
    }

    async fn take_bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.reservations.remove(key);
        let value = self.cache.remove(key).await?;
        if value.is_expired() {
            return None;
        }
        Some(value.bytes)
    }

    async fn set_bytes(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        self.cache
            .insert(key.to_string(), self.cache_value(value, ttl_secs))
            .await;
    }

    async fn set_bytes_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        if self.get_bytes(key).await.is_some() {
            return false;
        }
        let Some(guard) = self.reservations.reserve_guarded(key, ttl_secs) else {
            return false;
        };
        if self.get_bytes(key).await.is_some() {
            // Lost the race to a concurrently inserted value. Dropping the guard
            // releases our reservation: keeping it would falsely block every later
            // `set_bytes_if_absent` until its TTL expired — including after the
            // winning value itself is evicted (TTL eviction never touches the set).
            return false;
        }

        self.cache
            .insert(key.to_string(), self.cache_value(value, ttl_secs))
            .await;
        // The value is published; the reservation now lives for its co-lifetime
        // with the value instead of ending with this call.
        guard.commit();
        true
    }

    async fn delete(&self, key: &str) {
        self.reservations.remove(key);
        self.cache.remove(key).await;
    }

    async fn delete_many(&self, keys: &[String]) {
        for key in keys {
            self.delete(key).await;
        }
    }

    async fn invalidate_prefix(&self, prefix: &str) {
        self.reservations.invalidate_prefix(prefix);
        let keys: Vec<Arc<String>> = self
            .cache
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            self.cache.remove(key.as_ref()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheBackend, MemoryCache, entry_weight};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn entry_weight_counts_key_and_value_bytes() {
        assert_eq!(entry_weight(3, 5), 8);
    }

    #[test]
    fn entry_weight_saturates_at_u32_max() {
        assert_eq!(entry_weight(usize::MAX, usize::MAX), u32::MAX);
    }

    #[tokio::test]
    async fn set_bytes_if_absent_allows_one_concurrent_insert() {
        let cache = Arc::new(MemoryCache::new(60));
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let cache = cache.clone();
            tasks.push(tokio::spawn(async move {
                cache
                    .set_bytes_if_absent("nonce", Vec::new(), Some(60))
                    .await
            }));
        }

        let successes = futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|result| result.expect("reservation task should not panic"))
            .filter(|inserted| *inserted)
            .count();

        assert_eq!(successes, 1);
    }

    #[tokio::test]
    async fn set_bytes_if_absent_keeps_reservation_after_successful_insert() {
        let cache = MemoryCache::new(60);

        assert!(
            cache
                .set_bytes_if_absent("nonce", b"claimed".to_vec(), Some(60))
                .await
        );

        // The committed reservation outlives the call, co-living with the value.
        assert!(!cache.reservations.reserve("nonce", Some(60)));
    }

    #[tokio::test]
    async fn set_bytes_if_absent_takes_no_reservation_when_value_is_already_visible() {
        let cache = MemoryCache::new(60);
        cache.set_bytes("nonce", b"plain".to_vec(), Some(60)).await;

        assert!(
            !cache
                .set_bytes_if_absent("nonce", b"claimed".to_vec(), Some(60))
                .await
        );

        // The first visibility check lost before reserving, so the key stays
        // reservable. The lose-after-reserving path releases its reservation via
        // ReservationGuard (unit-tested in the reservation module).
        assert!(cache.reservations.reserve("nonce", Some(60)));
    }

    #[tokio::test]
    async fn set_bytes_if_absent_respects_existing_set_value() {
        let cache = MemoryCache::new(60);

        cache.set_bytes("nonce", b"first".to_vec(), Some(60)).await;

        assert!(
            !cache
                .set_bytes_if_absent("nonce", b"second".to_vec(), Some(60))
                .await
        );
        assert_eq!(cache.get_bytes("nonce").await, Some(b"first".to_vec()));
    }

    #[tokio::test]
    async fn set_bytes_respects_entry_ttl() {
        let cache = MemoryCache::new(60);

        cache.set_bytes("short", b"value".to_vec(), Some(0)).await;

        assert_eq!(cache.get_bytes("short").await, None);
    }

    #[test]
    fn expiry_derives_duration_from_absolute_expires_at() {
        use moka::Expiry;

        let expiry = super::MemoryCacheExpiry;
        let value = super::MemoryCacheValue::new(b"value".to_vec(), 60);
        let zero_ttl = super::MemoryCacheValue::new(b"value".to_vec(), 0);
        // moka passes its insertion instant, which always follows value construction.
        let created_at = std::time::Instant::now();

        let ttl = expiry
            .expire_after_create(&"key".to_string(), &value, created_at)
            .expect("entry should carry an expiration");
        assert!(ttl > Duration::from_secs(59) && ttl <= Duration::from_secs(60));

        // A zero-TTL value is already expired at insertion, so no lifetime remains.
        assert_eq!(
            expiry.expire_after_create(&"key".to_string(), &zero_ttl, created_at),
            Some(Duration::ZERO)
        );
    }

    #[tokio::test]
    async fn per_entry_ttl_outlives_shorter_default_ttl() {
        let cache = MemoryCache::new(1);
        cache.set_bytes("default", b"default".to_vec(), None).await;
        cache.set_bytes("long", b"long".to_vec(), Some(2)).await;

        // moka's expiration clock is real time, so this test must really wait.
        tokio::time::sleep(Duration::from_millis(1_100)).await;

        // The M1 bug: a builder-level time_to_live evicted "long" at the 1s default even
        // though its per-entry TTL is 2s.
        assert_eq!(cache.get_bytes("default").await, None);
        assert_eq!(cache.get_bytes("long").await, Some(b"long".to_vec()));

        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(cache.get_bytes("long").await, None);
    }

    #[tokio::test]
    async fn zero_default_ttl_still_stores_explicit_entry_ttl() {
        let cache = MemoryCache::new(0);

        cache
            .set_bytes("explicit", b"value".to_vec(), Some(60))
            .await;
        cache.set_bytes("implicit", b"value".to_vec(), None).await;

        // Before per-entry expiry, time_to_live(0) made the whole cache write-only.
        assert_eq!(cache.get_bytes("explicit").await, Some(b"value".to_vec()));
        assert_eq!(cache.get_bytes("implicit").await, None);
    }

    #[tokio::test]
    async fn set_bytes_if_absent_can_replace_expired_entry() {
        let cache = MemoryCache::new(60);

        cache.set_bytes("nonce", b"expired".to_vec(), Some(0)).await;

        assert!(
            cache
                .set_bytes_if_absent("nonce", b"fresh".to_vec(), Some(60))
                .await
        );
        assert_eq!(cache.get_bytes("nonce").await, Some(b"fresh".to_vec()));
    }

    #[tokio::test]
    async fn take_bytes_consumes_existing_entry_once() {
        let cache = MemoryCache::new(60);

        cache
            .set_bytes("challenge", b"value".to_vec(), Some(60))
            .await;

        assert_eq!(cache.take_bytes("challenge").await, Some(b"value".to_vec()));
        assert_eq!(cache.take_bytes("challenge").await, None);
        assert_eq!(cache.get_bytes("challenge").await, None);
    }

    #[tokio::test]
    async fn take_bytes_returns_none_for_missing_or_expired_entry() {
        let cache = MemoryCache::new(60);

        assert_eq!(cache.take_bytes("missing").await, None);
        cache.set_bytes("expired", b"value".to_vec(), Some(0)).await;

        assert_eq!(cache.take_bytes("expired").await, None);
        assert_eq!(cache.get_bytes("expired").await, None);
    }

    #[tokio::test]
    async fn take_bytes_allows_one_concurrent_consumer() {
        let cache = Arc::new(MemoryCache::new(60));
        cache
            .set_bytes("challenge", b"value".to_vec(), Some(60))
            .await;
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let cache = cache.clone();
            tasks.push(tokio::spawn(
                async move { cache.take_bytes("challenge").await },
            ));
        }

        let values = futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|result| result.expect("take task should not panic"))
            .collect::<Vec<_>>();

        assert_eq!(
            values
                .iter()
                .filter(|value| value.as_deref() == Some(b"value".as_slice()))
                .count(),
            1
        );
        assert_eq!(values.iter().filter(|value| value.is_none()).count(), 15);
    }

    #[tokio::test]
    async fn delete_many_removes_only_requested_entries() {
        let cache = MemoryCache::new(60);
        cache.set_bytes("remove:1", b"one".to_vec(), Some(60)).await;
        cache.set_bytes("remove:2", b"two".to_vec(), Some(60)).await;
        cache.set_bytes("keep", b"keep".to_vec(), Some(60)).await;

        cache
            .delete_many(&[
                "remove:1".to_string(),
                "remove:2".to_string(),
                "remove:2".to_string(),
                "missing".to_string(),
            ])
            .await;
        cache.delete_many(&[]).await;

        assert_eq!(cache.get_bytes("remove:1").await, None);
        assert_eq!(cache.get_bytes("remove:2").await, None);
        assert_eq!(cache.get_bytes("keep").await, Some(b"keep".to_vec()));
    }
}

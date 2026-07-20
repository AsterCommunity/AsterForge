//! Redis cache backend with local fallback support.
//!
//! Operations are bounded by short timeouts so a slow Redis instance does not stall request paths.
//! When Redis becomes unavailable, writes are mirrored into an in-memory fallback and availability
//! checks are rate-limited until the cooldown expires. Only connectivity failures and transient
//! server states open the fallback circuit; deterministic command errors (for example WRONGTYPE)
//! are logged and fall back for that single operation without degrading the backend.

use super::{CacheBackend, CacheError, Result, memory::MemoryCache};
use async_trait::async_trait;
use redis::{AsyncCommands, ExistenceCheck, SetExpiry, SetOptions};
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const REDIS_CACHE_OPERATION_TIMEOUT: Duration = Duration::from_millis(250);
const REDIS_CACHE_CONNECTION_TIMEOUT: Duration = Duration::from_millis(500);
const REDIS_CACHE_RECONNECT_MIN_DELAY: Duration = Duration::from_millis(100);
const REDIS_CACHE_RECONNECT_MAX_DELAY: Duration = Duration::from_millis(500);
const REDIS_CACHE_RECONNECT_RETRIES: usize = 1;
const REDIS_CACHE_FALLBACK_COOLDOWN: Duration = Duration::from_secs(5);

/// Redis cache backend with a short-lived local memory fallback.
pub struct RedisCache {
    inner: RedisCacheInner<RedisConnectionManager>,
}

struct RedisCacheInner<R> {
    redis: R,
    default_ttl: u64,
    local: MemoryCache,
    availability: RedisAvailability,
}

#[async_trait]
trait RedisClient: Send + Sync {
    async fn get(&self, key: &str) -> redis::RedisResult<Option<Vec<u8>>>;
    async fn take(&self, key: &str) -> redis::RedisResult<Option<Vec<u8>>>;
    async fn set_ex(&self, key: &str, value: Vec<u8>, ttl: u64) -> redis::RedisResult<()>;
    async fn set_nx_ex(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: u64,
    ) -> redis::RedisResult<Option<String>>;
    async fn delete(&self, key: &str) -> redis::RedisResult<()>;
    async fn scan_prefix(
        &self,
        cursor: u64,
        pattern: &str,
    ) -> redis::RedisResult<(u64, Vec<String>)>;
    async fn delete_keys(&self, keys: &[String]) -> redis::RedisResult<()>;
    async fn ping(&self) -> redis::RedisResult<String>;
}

struct RedisConnectionManager {
    conn: redis::aio::ConnectionManager,
}

impl RedisConnectionManager {
    fn new(conn: redis::aio::ConnectionManager) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl RedisClient for RedisConnectionManager {
    async fn get(&self, key: &str) -> redis::RedisResult<Option<Vec<u8>>> {
        let mut conn = self.conn.clone();
        conn.get::<_, Option<Vec<u8>>>(key).await
    }

    async fn take(&self, key: &str) -> redis::RedisResult<Option<Vec<u8>>> {
        let mut conn = self.conn.clone();
        redis::Script::new(
            r#"
            local value = redis.call("GET", KEYS[1])
            if value then
                redis.call("DEL", KEYS[1])
            end
            return value
            "#,
        )
        .key(key)
        .invoke_async::<Option<Vec<u8>>>(&mut conn)
        .await
    }

    async fn set_ex(&self, key: &str, value: Vec<u8>, ttl: u64) -> redis::RedisResult<()> {
        let mut conn = self.conn.clone();
        conn.set_ex::<_, _, ()>(key, value, ttl).await
    }

    async fn set_nx_ex(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: u64,
    ) -> redis::RedisResult<Option<String>> {
        let options = SetOptions::default()
            .conditional_set(ExistenceCheck::NX)
            .with_expiration(SetExpiry::EX(ttl));
        let mut conn = self.conn.clone();
        conn.set_options::<_, _, Option<String>>(key, value, options)
            .await
    }

    async fn delete(&self, key: &str) -> redis::RedisResult<()> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(key).await
    }

    async fn scan_prefix(
        &self,
        cursor: u64,
        pattern: &str,
    ) -> redis::RedisResult<(u64, Vec<String>)> {
        let mut conn = self.conn.clone();
        let mut scan_cmd = redis::cmd("SCAN");
        scan_cmd
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(100)
            .query_async::<(u64, Vec<String>)>(&mut conn)
            .await
    }

    async fn delete_keys(&self, keys: &[String]) -> redis::RedisResult<()> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(keys).await
    }

    async fn ping(&self) -> redis::RedisResult<String> {
        let mut conn = self.conn.clone();
        redis::cmd("PING").query_async::<String>(&mut conn).await
    }
}

impl RedisCache {
    /// Creates a Redis cache from a Redis URL and default TTL in seconds.
    pub async fn new(url: &str, default_ttl: u64) -> Result<Self> {
        let client = redis::Client::open(url)?;
        let manager_config = redis::aio::ConnectionManagerConfig::new()
            .set_response_timeout(Some(REDIS_CACHE_OPERATION_TIMEOUT))
            .set_connection_timeout(Some(REDIS_CACHE_CONNECTION_TIMEOUT))
            .set_min_delay(REDIS_CACHE_RECONNECT_MIN_DELAY)
            .set_max_delay(REDIS_CACHE_RECONNECT_MAX_DELAY)
            .set_number_of_retries(REDIS_CACHE_RECONNECT_RETRIES);
        let conn = redis::aio::ConnectionManager::new_with_config(client, manager_config).await?;
        Ok(Self {
            inner: RedisCacheInner::new(RedisConnectionManager::new(conn), default_ttl),
        })
    }
}

impl<R> RedisCacheInner<R>
where
    R: RedisClient,
{
    fn new(redis: R, default_ttl: u64) -> Self {
        Self {
            redis,
            default_ttl,
            local: MemoryCache::new(default_ttl),
            availability: RedisAvailability::default(),
        }
    }

    async fn get_local_bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.local.get_bytes(key).await
    }

    async fn set_local_bytes(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        self.local.set_bytes(key, value, ttl_secs).await;
    }

    async fn set_local_bytes_if_absent(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl_secs: Option<u64>,
    ) -> bool {
        self.local.set_bytes_if_absent(key, value, ttl_secs).await
    }

    async fn delete_local(&self, key: &str) {
        self.local.delete(key).await;
    }

    async fn invalidate_local_prefix(&self, prefix: &str) {
        self.local.invalidate_prefix(prefix).await;
    }

    async fn redis_operation<T, Fut>(&self, operation: &'static str, future: Fut) -> Option<T>
    where
        T: Send,
        Fut: Future<Output = redis::RedisResult<T>> + Send,
    {
        if let Some(remaining) = self.redis_unavailable_for() {
            tracing::trace!(
                operation,
                remaining_ms = duration_millis_u64(remaining),
                "redis cache circuit open; skipping redis operation"
            );
            return None;
        }

        match tokio::time::timeout(REDIS_CACHE_OPERATION_TIMEOUT, future).await {
            Ok(Ok(value)) => {
                self.mark_redis_success(operation);
                Some(value)
            }
            Ok(Err(error)) => {
                self.mark_redis_error(operation, &error);
                None
            }
            Err(_) => {
                self.mark_redis_timeout(operation);
                None
            }
        }
    }

    fn redis_unavailable_for(&self) -> Option<Duration> {
        self.availability.unavailable_for(Instant::now())
    }

    fn mark_redis_success(&self, operation: &'static str) {
        if self.availability.mark_success() {
            tracing::info!(operation, "redis cache recovered; closing fallback circuit");
        }
    }

    fn mark_redis_error(&self, operation: &'static str, error: &redis::RedisError) {
        if !redis_error_indicates_unavailability(error) {
            tracing::warn!(
                operation,
                error = %error,
                "redis cache command error; leaving fallback circuit closed"
            );
            return;
        }
        if self
            .availability
            .mark_failure(Instant::now(), REDIS_CACHE_FALLBACK_COOLDOWN)
        {
            tracing::warn!(
                operation,
                error = %error,
                cooldown_secs = REDIS_CACHE_FALLBACK_COOLDOWN.as_secs(),
                "redis cache unavailable; using local fallback temporarily"
            );
        } else {
            tracing::debug!(
                operation,
                error = %error,
                "redis cache operation failed while fallback circuit is already open"
            );
        }
    }

    fn mark_redis_timeout(&self, operation: &'static str) {
        if self
            .availability
            .mark_failure(Instant::now(), REDIS_CACHE_FALLBACK_COOLDOWN)
        {
            tracing::warn!(
                operation,
                timeout_ms = duration_millis_u64(REDIS_CACHE_OPERATION_TIMEOUT),
                cooldown_secs = REDIS_CACHE_FALLBACK_COOLDOWN.as_secs(),
                "redis cache operation timed out; using local fallback temporarily"
            );
        } else {
            tracing::debug!(
                operation,
                timeout_ms = duration_millis_u64(REDIS_CACHE_OPERATION_TIMEOUT),
                "redis cache operation timed out while fallback circuit is already open"
            );
        }
    }
}

impl<R> RedisCacheInner<R>
where
    R: RedisClient,
{
    async fn health_check(&self) -> Result<()> {
        if let Some(remaining) = self.redis_unavailable_for() {
            return Err(CacheError::RedisFallbackMode {
                remaining_ms: remaining.as_millis(),
            });
        }

        match tokio::time::timeout(REDIS_CACHE_OPERATION_TIMEOUT, self.redis.ping()).await {
            Ok(Ok(_)) => {
                self.mark_redis_success("health_check");
                Ok(())
            }
            Ok(Err(error)) => {
                self.mark_redis_error("health_check", &error);
                Err(CacheError::RedisHealthCheck(error.to_string()))
            }
            Err(_) => {
                self.mark_redis_timeout("health_check");
                Err(CacheError::RedisHealthCheckTimeout {
                    timeout_ms: REDIS_CACHE_OPERATION_TIMEOUT.as_millis(),
                })
            }
        }
    }

    async fn get_bytes(&self, key: &str) -> Option<Vec<u8>> {
        match self.redis_operation("get", self.redis.get(key)).await {
            Some(value) => value,
            None => self.get_local_bytes(key).await,
        }
    }

    async fn take_bytes(&self, key: &str) -> Option<Vec<u8>> {
        match self.redis_operation("take", self.redis.take(key)).await {
            Some(value) => {
                self.delete_local(key).await;
                value
            }
            None => self.local.take_bytes(key).await,
        }
    }

    async fn set_bytes(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        let ttl = ttl_secs.unwrap_or(self.default_ttl);
        if ttl == 0 {
            // Redis rejects SETEX with a zero TTL. An immediate delete produces the
            // documented "expires immediately" observable state without issuing an
            // invalid command.
            self.delete(key).await;
            return;
        }
        if self
            .redis_operation("set", self.redis.set_ex(key, value.clone(), ttl))
            .await
            .is_some()
        {
            self.delete_local(key).await;
        } else {
            self.set_local_bytes(key, value, ttl_secs).await;
        }
    }

    async fn set_bytes_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        let ttl = ttl_secs.unwrap_or(self.default_ttl);
        if ttl == 0 {
            // A zero-TTL insert expires immediately, so the outcome only depends on
            // whether a live value exists; nothing is retained either way.
            return match self
                .redis_operation("set_if_absent", self.redis.get(key))
                .await
            {
                Some(existing) => {
                    self.delete_local(key).await;
                    existing.is_none()
                }
                None => self.set_local_bytes_if_absent(key, value, Some(0)).await,
            };
        }
        match self
            .redis_operation(
                "set_if_absent",
                self.redis.set_nx_ex(key, value.clone(), ttl),
            )
            .await
        {
            Some(Some(_)) => {
                self.delete_local(key).await;
                true
            }
            Some(None) => {
                self.delete_local(key).await;
                false
            }
            None => self.set_local_bytes_if_absent(key, value, ttl_secs).await,
        }
    }

    async fn delete(&self, key: &str) {
        self.delete_local(key).await;
        let _: Option<()> = self.redis_operation("delete", self.redis.delete(key)).await;
    }

    async fn delete_many(&self, keys: &[String]) {
        for key in keys {
            self.delete_local(key).await;
        }
        if !keys.is_empty() {
            let _: Option<()> = self
                .redis_operation("delete_many", self.redis.delete_keys(keys))
                .await;
        }
    }

    async fn invalidate_prefix(&self, prefix: &str) {
        self.invalidate_local_prefix(prefix).await;

        let pattern = format!("{prefix}*");
        let mut cursor: u64 = 0;
        loop {
            let Some((next_cursor, keys)) = self
                .redis_operation(
                    "invalidate_prefix_scan",
                    self.redis.scan_prefix(cursor, &pattern),
                )
                .await
            else {
                break;
            };
            if !keys.is_empty()
                && self
                    .redis_operation("invalidate_prefix_delete", self.redis.delete_keys(&keys))
                    .await
                    .is_none()
            {
                break;
            }
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }
    }
}

#[async_trait]
impl CacheBackend for RedisCache {
    fn backend_name(&self) -> &'static str {
        "redis"
    }

    async fn health_check(&self) -> Result<()> {
        self.inner.health_check().await
    }

    async fn get_bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.get_bytes(key).await
    }

    async fn take_bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.take_bytes(key).await
    }

    async fn set_bytes(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        self.inner.set_bytes(key, value, ttl_secs).await;
    }

    async fn set_bytes_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        self.inner.set_bytes_if_absent(key, value, ttl_secs).await
    }

    async fn delete(&self, key: &str) {
        self.inner.delete(key).await;
    }

    async fn delete_many(&self, keys: &[String]) {
        self.inner.delete_many(keys).await;
    }

    async fn invalidate_prefix(&self, prefix: &str) {
        self.inner.invalidate_prefix(prefix).await;
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Returns whether a Redis error indicates the server is unreachable or in a transient
/// state, as opposed to a deterministic command, data, or configuration error that a
/// fallback circuit cannot fix.
fn redis_error_indicates_unavailability(error: &redis::RedisError) -> bool {
    match error.kind() {
        redis::ErrorKind::Io | redis::ErrorKind::ClusterConnectionNotFound => true,
        redis::ErrorKind::Server(kind) => matches!(
            kind,
            redis::ServerErrorKind::BusyLoading
                | redis::ServerErrorKind::TryAgain
                | redis::ServerErrorKind::ClusterDown
                | redis::ServerErrorKind::MasterDown
                | redis::ServerErrorKind::ReadOnly
        ),
        _ => false,
    }
}

#[derive(Default)]
struct RedisAvailability {
    unavailable_until: Mutex<Option<Instant>>,
}

impl RedisAvailability {
    fn unavailable_for(&self, now: Instant) -> Option<Duration> {
        let mut unavailable_until = self.lock_unavailable_until();
        match *unavailable_until {
            Some(deadline) if deadline > now => Some(deadline.duration_since(now)),
            Some(_) => {
                *unavailable_until = None;
                None
            }
            None => None,
        }
    }

    fn mark_failure(&self, now: Instant, cooldown: Duration) -> bool {
        let mut unavailable_until = self.lock_unavailable_until();
        let was_available = unavailable_until.is_none_or(|deadline| deadline <= now);
        *unavailable_until = now.checked_add(cooldown).or(Some(now));
        was_available
    }

    fn mark_success(&self) -> bool {
        self.lock_unavailable_until().take().is_some()
    }

    fn lock_unavailable_until(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.unavailable_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CacheBackend, REDIS_CACHE_FALLBACK_COOLDOWN, RedisAvailability, RedisCacheInner,
        RedisClient,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};
    use tokio::time::sleep;

    #[derive(Default)]
    struct FakeRedisClient {
        entries: Mutex<HashMap<String, Vec<u8>>>,
        scan_pages: Mutex<HashMap<u64, Vec<String>>>,
        fail_operations: AtomicBool,
        fail_command_errors: AtomicBool,
        next_scan_cursor: AtomicU64,
        get_calls: AtomicUsize,
        take_calls: AtomicUsize,
        set_calls: AtomicUsize,
        set_nx_calls: AtomicUsize,
        delete_calls: AtomicUsize,
        scan_calls: AtomicUsize,
        delete_keys_calls: AtomicUsize,
        ping_calls: AtomicUsize,
    }

    impl FakeRedisClient {
        fn set_fail_operations(&self, fail: bool) {
            self.fail_operations.store(fail, Ordering::SeqCst);
        }

        fn set_fail_command_errors(&self, fail: bool) {
            self.fail_command_errors.store(fail, Ordering::SeqCst);
        }

        fn insert(&self, key: &str, value: &[u8]) {
            self.entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(key.to_string(), value.to_vec());
        }

        fn contains_key(&self, key: &str) -> bool {
            self.entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(key)
        }

        fn get_call_count(&self) -> usize {
            self.get_calls.load(Ordering::SeqCst)
        }

        fn take_call_count(&self) -> usize {
            self.take_calls.load(Ordering::SeqCst)
        }

        fn set_call_count(&self) -> usize {
            self.set_calls.load(Ordering::SeqCst)
        }

        fn set_nx_call_count(&self) -> usize {
            self.set_nx_calls.load(Ordering::SeqCst)
        }

        fn delete_call_count(&self) -> usize {
            self.delete_calls.load(Ordering::SeqCst)
        }

        fn scan_call_count(&self) -> usize {
            self.scan_calls.load(Ordering::SeqCst)
        }

        fn delete_keys_call_count(&self) -> usize {
            self.delete_keys_calls.load(Ordering::SeqCst)
        }

        fn ping_call_count(&self) -> usize {
            self.ping_calls.load(Ordering::SeqCst)
        }

        fn maybe_fail(&self) -> redis::RedisResult<()> {
            if self.fail_operations.load(Ordering::SeqCst) {
                Err(redis::RedisError::from((
                    redis::ErrorKind::Io,
                    "fake redis unavailable",
                )))
            } else if self.fail_command_errors.load(Ordering::SeqCst) {
                Err(redis::RedisError::from((
                    redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
                    "ERR invalid expire time in 'setex' command",
                )))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl RedisClient for Arc<FakeRedisClient> {
        async fn get(&self, key: &str) -> redis::RedisResult<Option<Vec<u8>>> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(key)
                .cloned())
        }

        async fn take(&self, key: &str) -> redis::RedisResult<Option<Vec<u8>>> {
            self.take_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(key))
        }

        async fn set_ex(&self, key: &str, value: Vec<u8>, _ttl: u64) -> redis::RedisResult<()> {
            self.set_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            self.entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(key.to_string(), value);
            Ok(())
        }

        async fn set_nx_ex(
            &self,
            key: &str,
            value: Vec<u8>,
            _ttl: u64,
        ) -> redis::RedisResult<Option<String>> {
            self.set_nx_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if entries.contains_key(key) {
                Ok(None)
            } else {
                entries.insert(key.to_string(), value);
                Ok(Some("OK".to_string()))
            }
        }

        async fn delete(&self, key: &str) -> redis::RedisResult<()> {
            self.delete_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            self.entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(key);
            Ok(())
        }

        async fn scan_prefix(
            &self,
            cursor: u64,
            pattern: &str,
        ) -> redis::RedisResult<(u64, Vec<String>)> {
            self.scan_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            let prefix = pattern
                .strip_suffix('*')
                .expect("prefix invalidation should scan a trailing-star pattern");
            const PAGE_SIZE: usize = 2;
            let mut keys = if cursor == 0 {
                let mut keys: Vec<String> = self
                    .entries
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .keys()
                    .filter(|key| key.starts_with(prefix))
                    .cloned()
                    .collect();
                keys.sort();
                keys
            } else {
                self.scan_pages
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&cursor)
                    .unwrap_or_default()
            };
            if keys.is_empty() {
                return Ok((0, Vec::new()));
            }
            let page_len = PAGE_SIZE.min(keys.len());
            let remaining = keys.split_off(page_len);
            let page = keys;
            let next_cursor = if remaining.is_empty() {
                0
            } else {
                let next_cursor = self.next_scan_cursor.fetch_add(1, Ordering::SeqCst) + 1;
                self.scan_pages
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(next_cursor, remaining);
                next_cursor
            };
            Ok((next_cursor, page))
        }

        async fn delete_keys(&self, keys: &[String]) -> redis::RedisResult<()> {
            self.delete_keys_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for key in keys {
                entries.remove(key);
            }
            Ok(())
        }

        async fn ping(&self) -> redis::RedisResult<String> {
            self.ping_calls.fetch_add(1, Ordering::SeqCst);
            self.maybe_fail()?;
            Ok("PONG".to_string())
        }
    }

    fn cache_with_fake_redis(
        default_ttl: u64,
    ) -> (RedisCacheInner<Arc<FakeRedisClient>>, Arc<FakeRedisClient>) {
        let redis = Arc::new(FakeRedisClient::default());
        (RedisCacheInner::new(redis.clone(), default_ttl), redis)
    }

    fn open_fallback_circuit<R: RedisClient>(cache: &RedisCacheInner<R>) {
        assert!(
            cache
                .availability
                .mark_failure(Instant::now(), REDIS_CACHE_FALLBACK_COOLDOWN)
        );
    }

    #[test]
    fn redis_availability_skips_until_cooldown_expires() {
        let availability = RedisAvailability::default();
        let now = Instant::now();

        assert!(availability.unavailable_for(now).is_none());
        assert!(availability.mark_failure(now, Duration::from_secs(5)));
        assert_eq!(
            availability.unavailable_for(now + Duration::from_secs(2)),
            Some(Duration::from_secs(3))
        );
        assert!(
            availability
                .unavailable_for(now + Duration::from_secs(6))
                .is_none()
        );
    }

    #[test]
    fn redis_availability_reports_recovery_once() {
        let availability = RedisAvailability::default();
        let now = Instant::now();

        assert!(availability.mark_failure(now, Duration::from_secs(5)));
        assert!(availability.mark_success());
        assert!(!availability.mark_success());
    }

    #[test]
    fn redis_availability_repeated_failures_only_report_transition_once() {
        let availability = RedisAvailability::default();
        let now = Instant::now();

        assert!(availability.mark_failure(now, Duration::from_secs(5)));
        assert!(!availability.mark_failure(now + Duration::from_secs(1), Duration::from_secs(5)));
    }

    #[tokio::test]
    async fn fallback_set_and_get_round_trip_while_circuit_is_open() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);

        cache.set_bytes("ticket", b"local".to_vec(), Some(60)).await;

        assert_eq!(cache.get_bytes("ticket").await, Some(b"local".to_vec()));
        assert_eq!(
            redis.set_call_count(),
            0,
            "circuit-open set should skip Redis"
        );
        assert_eq!(
            redis.get_call_count(),
            0,
            "circuit-open get should skip Redis"
        );
    }

    #[tokio::test]
    async fn failed_redis_set_stores_value_in_local_fallback() {
        let (cache, redis) = cache_with_fake_redis(60);
        redis.set_fail_operations(true);

        cache
            .set_bytes("session", b"fallback".to_vec(), Some(60))
            .await;

        assert_eq!(redis.set_call_count(), 1);
        assert_eq!(cache.get_bytes("session").await, Some(b"fallback".to_vec()));
        assert_eq!(
            redis.get_call_count(),
            0,
            "first failed set opens the circuit, so later get should skip Redis"
        );
    }

    #[tokio::test]
    async fn redis_miss_does_not_return_stale_local_value_when_redis_is_available() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache
            .set_bytes("snapshot", b"stale-local".to_vec(), Some(60))
            .await;
        redis.set_fail_operations(false);
        cache.availability.mark_success();

        assert_eq!(cache.get_bytes("snapshot").await, None);
        assert_eq!(redis.get_call_count(), 1);
    }

    #[tokio::test]
    async fn successful_redis_set_clears_local_fallback_shadow() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache
            .set_bytes("profile", b"local-shadow".to_vec(), Some(60))
            .await;
        cache.availability.mark_success();

        cache
            .set_bytes("profile", b"redis-value".to_vec(), Some(60))
            .await;

        assert_eq!(redis.set_call_count(), 1);
        assert!(redis.contains_key("profile"));
        assert_eq!(cache.local.get_bytes("profile").await, None);
        assert_eq!(
            cache.get_bytes("profile").await,
            Some(b"redis-value".to_vec())
        );
    }

    #[tokio::test]
    async fn take_bytes_consumes_redis_entry_atomically() {
        let (cache, redis) = cache_with_fake_redis(60);
        redis.insert("challenge", b"value");

        assert_eq!(cache.take_bytes("challenge").await, Some(b"value".to_vec()));
        assert_eq!(cache.take_bytes("challenge").await, None);
        assert!(!redis.contains_key("challenge"));
        assert_eq!(redis.take_call_count(), 2);
        assert_eq!(
            redis.get_call_count(),
            0,
            "take should not read through a separate GET"
        );
    }

    #[tokio::test]
    async fn take_bytes_consumes_local_fallback_when_circuit_is_open() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache
            .set_bytes("challenge", b"local".to_vec(), Some(60))
            .await;

        assert_eq!(cache.take_bytes("challenge").await, Some(b"local".to_vec()));
        assert_eq!(cache.take_bytes("challenge").await, None);
        assert_eq!(
            redis.take_call_count(),
            0,
            "circuit-open take should skip Redis"
        );
    }

    #[tokio::test]
    async fn take_bytes_falls_back_to_local_without_dropping_value_on_redis_failure() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache
            .set_bytes("challenge", b"local".to_vec(), Some(60))
            .await;
        cache.availability.mark_success();
        redis.set_fail_operations(true);

        assert_eq!(cache.take_bytes("challenge").await, Some(b"local".to_vec()));
        assert_eq!(cache.take_bytes("challenge").await, None);
        assert_eq!(redis.take_call_count(), 1);
    }

    #[tokio::test]
    async fn fallback_set_if_absent_stores_value_and_rejects_second_insert() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);

        assert!(
            cache
                .set_bytes_if_absent("nonce", b"first".to_vec(), Some(60))
                .await
        );
        assert!(
            !cache
                .set_bytes_if_absent("nonce", b"second".to_vec(), Some(60))
                .await
        );

        assert_eq!(cache.get_bytes("nonce").await, Some(b"first".to_vec()));
        assert_eq!(redis.set_nx_call_count(), 0);
    }

    #[tokio::test]
    async fn fallback_set_if_absent_is_atomic_for_concurrent_callers() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        let cache = Arc::new(cache);
        let mut tasks = Vec::new();

        for index in 0..32 {
            let cache = cache.clone();
            tasks.push(tokio::spawn(async move {
                cache
                    .set_bytes_if_absent(
                        "concurrent-nonce",
                        format!("value-{index}").into_bytes(),
                        Some(60),
                    )
                    .await
            }));
        }

        let inserted = futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|result| result.expect("fallback reservation task should not panic"))
            .filter(|inserted| *inserted)
            .count();

        assert_eq!(inserted, 1);
        assert!(cache.get_bytes("concurrent-nonce").await.is_some());
        assert_eq!(redis.set_nx_call_count(), 0);
    }

    #[tokio::test]
    async fn fallback_entries_respect_zero_ttl_boundary() {
        let (cache, _redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);

        cache.set_bytes("expired", b"value".to_vec(), Some(0)).await;
        assert_eq!(cache.get_bytes("expired").await, None);

        assert!(
            cache
                .set_bytes_if_absent("zero-nonce", b"first".to_vec(), Some(0))
                .await
        );
        assert_eq!(cache.get_bytes("zero-nonce").await, None);
        assert!(
            cache
                .set_bytes_if_absent("zero-nonce", b"second".to_vec(), Some(0))
                .await
        );
    }

    #[tokio::test]
    async fn fallback_entries_expire_after_configured_ttl() {
        let (cache, _redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);

        cache
            .set_bytes("short-lived", b"value".to_vec(), Some(1))
            .await;
        assert_eq!(
            cache.get_bytes("short-lived").await,
            Some(b"value".to_vec())
        );

        sleep(Duration::from_millis(1_100)).await;

        assert_eq!(cache.get_bytes("short-lived").await, None);
    }

    #[tokio::test]
    async fn delete_clears_local_fallback_even_when_redis_is_unavailable() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache
            .set_bytes("delete-me", b"value".to_vec(), Some(60))
            .await;

        cache.delete("delete-me").await;

        assert_eq!(cache.get_bytes("delete-me").await, None);
        assert_eq!(
            redis.delete_call_count(),
            0,
            "circuit-open delete should skip Redis"
        );
    }

    #[tokio::test]
    async fn delete_many_removes_requested_redis_and_local_entries_in_one_batch() {
        let (cache, redis) = cache_with_fake_redis(60);
        redis.insert("remove:1", b"one");
        redis.insert("remove:2", b"two");
        redis.insert("keep", b"keep");
        open_fallback_circuit(&cache);
        cache
            .set_bytes("remove:local", b"local".to_vec(), Some(60))
            .await;
        cache.availability.mark_success();

        cache
            .delete_many(&[
                "remove:1".to_string(),
                "remove:2".to_string(),
                "remove:local".to_string(),
                "missing".to_string(),
            ])
            .await;
        cache.delete_many(&[]).await;

        assert!(!redis.contains_key("remove:1"));
        assert!(!redis.contains_key("remove:2"));
        assert!(redis.contains_key("keep"));
        assert_eq!(cache.local.get_bytes("remove:local").await, None);
        assert_eq!(redis.delete_keys_call_count(), 1);
    }

    #[tokio::test]
    async fn delete_many_clears_local_only_when_circuit_is_open() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache
            .set_bytes("remove:local", b"local".to_vec(), Some(60))
            .await;

        cache.delete_many(&["remove:local".to_string()]).await;

        assert_eq!(cache.get_bytes("remove:local").await, None);
        assert_eq!(
            redis.delete_keys_call_count(),
            0,
            "circuit-open batch delete should skip Redis"
        );
    }

    #[tokio::test]
    async fn invalidate_prefix_clears_local_fallback_even_when_redis_is_unavailable() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache.set_bytes("folder:1", b"one".to_vec(), Some(60)).await;
        cache.set_bytes("folder:2", b"two".to_vec(), Some(60)).await;
        cache.set_bytes("other:1", b"keep".to_vec(), Some(60)).await;

        cache.invalidate_prefix("folder:").await;

        assert_eq!(cache.get_bytes("folder:1").await, None);
        assert_eq!(cache.get_bytes("folder:2").await, None);
        assert_eq!(cache.get_bytes("other:1").await, Some(b"keep".to_vec()));
        assert_eq!(
            redis.scan_call_count(),
            0,
            "circuit-open prefix invalidation should skip Redis"
        );
    }

    #[tokio::test]
    async fn invalidate_prefix_deletes_matching_redis_keys_and_local_shadow() {
        let (cache, redis) = cache_with_fake_redis(60);
        redis.insert("folder:1", b"one");
        redis.insert("folder:2", b"two");
        redis.insert("other:1", b"keep");
        open_fallback_circuit(&cache);
        cache
            .set_bytes("folder:local", b"local".to_vec(), Some(60))
            .await;
        cache.availability.mark_success();

        cache.invalidate_prefix("folder:").await;

        assert!(!redis.contains_key("folder:1"));
        assert!(!redis.contains_key("folder:2"));
        assert!(redis.contains_key("other:1"));
        assert_eq!(cache.local.get_bytes("folder:local").await, None);
        assert_eq!(redis.scan_call_count(), 1);
        assert_eq!(redis.delete_keys_call_count(), 1);
    }

    #[tokio::test]
    async fn invalidate_prefix_scans_and_deletes_multiple_redis_pages() {
        let (cache, redis) = cache_with_fake_redis(60);
        for index in 0..5 {
            redis.insert(&format!("folder:{index}"), b"value");
        }
        redis.insert("other:1", b"keep");

        cache.invalidate_prefix("folder:").await;

        for index in 0..5 {
            assert!(!redis.contains_key(&format!("folder:{index}")));
        }
        assert!(redis.contains_key("other:1"));
        assert_eq!(redis.scan_call_count(), 3);
        assert_eq!(redis.delete_keys_call_count(), 3);
    }

    #[tokio::test]
    async fn health_check_reports_fallback_without_pinging_redis_while_circuit_is_open() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);

        let error = cache
            .health_check()
            .await
            .expect_err("open fallback circuit should report degraded Redis health");

        assert!(
            error
                .to_string()
                .contains("redis cache is in fallback mode")
        );
        assert_eq!(redis.ping_call_count(), 0);
    }

    #[tokio::test]
    async fn zero_ttl_set_deletes_key_instead_of_issuing_set_ex() {
        let (cache, redis) = cache_with_fake_redis(60);
        redis.insert("ephemeral", b"old");

        cache.set_bytes("ephemeral", b"new".to_vec(), Some(0)).await;

        assert_eq!(
            redis.set_call_count(),
            0,
            "zero-TTL set must not issue SETEX"
        );
        assert_eq!(redis.delete_call_count(), 1);
        assert!(!redis.contains_key("ephemeral"));
        assert!(
            cache.availability.unavailable_for(Instant::now()).is_none(),
            "zero-TTL set must not open the fallback circuit"
        );
        assert_eq!(cache.get_bytes("ephemeral").await, None);
        assert_eq!(
            redis.get_call_count(),
            1,
            "circuit stays closed, so the read reaches Redis"
        );
    }

    #[tokio::test]
    async fn zero_ttl_set_if_absent_reports_absence_without_storing() {
        let (cache, redis) = cache_with_fake_redis(60);

        assert!(
            cache
                .set_bytes_if_absent("nonce", b"v".to_vec(), Some(0))
                .await
        );
        assert_eq!(
            redis.set_nx_call_count(),
            0,
            "zero-TTL insert must not issue SET NX EX"
        );
        assert!(!redis.contains_key("nonce"));
        assert_eq!(cache.get_bytes("nonce").await, None);
        assert!(
            cache
                .set_bytes_if_absent("nonce", b"v2".to_vec(), Some(0))
                .await,
            "nothing is retained, so a second zero-TTL insert also succeeds"
        );

        redis.insert("live", b"real");
        assert!(
            !cache
                .set_bytes_if_absent("live", b"v".to_vec(), Some(0))
                .await,
            "an existing live value rejects the insert"
        );
        assert_eq!(cache.get_bytes("live").await, Some(b"real".to_vec()));
    }

    #[tokio::test]
    async fn zero_ttl_set_if_absent_with_open_circuit_uses_local_semantics() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);

        assert!(
            cache
                .set_bytes_if_absent("nonce", b"v".to_vec(), Some(0))
                .await
        );
        assert_eq!(
            redis.get_call_count(),
            0,
            "circuit-open existence check should skip Redis"
        );
        assert_eq!(cache.get_bytes("nonce").await, None);
        assert!(
            cache
                .set_bytes_if_absent("nonce", b"v2".to_vec(), Some(0))
                .await,
            "local zero-TTL entries expire immediately and stay insertable"
        );
    }

    #[tokio::test]
    async fn zero_ttl_set_with_open_circuit_only_clears_local_shadow() {
        let (cache, redis) = cache_with_fake_redis(60);
        open_fallback_circuit(&cache);
        cache.set_bytes("shadow", b"local".to_vec(), Some(60)).await;

        cache.set_bytes("shadow", b"gone".to_vec(), Some(0)).await;

        assert_eq!(
            redis.delete_call_count(),
            0,
            "circuit-open zero-TTL set should skip the Redis delete"
        );
        assert_eq!(cache.local.get_bytes("shadow").await, None);
        assert_eq!(cache.get_bytes("shadow").await, None);
    }

    #[tokio::test]
    async fn zero_default_ttl_treats_missing_ttl_as_immediate_expiry() {
        let (cache, redis) = cache_with_fake_redis(0);

        cache.set_bytes("key", b"value".to_vec(), None).await;

        assert_eq!(redis.set_call_count(), 0);
        assert_eq!(redis.delete_call_count(), 1);
        assert_eq!(cache.get_bytes("key").await, None);
    }

    #[tokio::test]
    async fn command_error_falls_back_for_single_operation_without_opening_circuit() {
        let (cache, redis) = cache_with_fake_redis(60);
        redis.set_fail_command_errors(true);

        cache
            .set_bytes("session", b"fallback".to_vec(), Some(60))
            .await;

        assert_eq!(redis.set_call_count(), 1);
        assert_eq!(
            cache.local.get_bytes("session").await,
            Some(b"fallback".to_vec()),
            "the failed operation still falls back locally"
        );
        assert!(
            cache.availability.unavailable_for(Instant::now()).is_none(),
            "command errors must not open the fallback circuit"
        );

        redis.set_fail_command_errors(false);
        redis.insert("session", b"redis-value");
        assert_eq!(
            cache.get_bytes("session").await,
            Some(b"redis-value".to_vec()),
            "later operations keep reaching Redis"
        );
    }

    #[tokio::test]
    async fn transient_server_error_opens_fallback_circuit() {
        let (cache, redis) = cache_with_fake_redis(60);
        redis.set_fail_operations(true);

        cache
            .set_bytes("session", b"value".to_vec(), Some(60))
            .await;

        assert!(
            cache.availability.unavailable_for(Instant::now()).is_some(),
            "I/O errors still open the fallback circuit"
        );
    }

    #[test]
    fn redis_error_indicates_unavailability_classifies_error_kinds() {
        use redis::{ErrorKind, ServerErrorKind};

        fn error(kind: ErrorKind) -> redis::RedisError {
            redis::RedisError::from((kind, "fake error"))
        }

        for kind in [
            ErrorKind::Io,
            ErrorKind::ClusterConnectionNotFound,
            ErrorKind::Server(ServerErrorKind::BusyLoading),
            ErrorKind::Server(ServerErrorKind::TryAgain),
            ErrorKind::Server(ServerErrorKind::ClusterDown),
            ErrorKind::Server(ServerErrorKind::MasterDown),
            ErrorKind::Server(ServerErrorKind::ReadOnly),
        ] {
            assert!(
                super::redis_error_indicates_unavailability(&error(kind)),
                "{kind:?} should indicate unavailability"
            );
        }

        for kind in [
            ErrorKind::Server(ServerErrorKind::ResponseError),
            ErrorKind::Server(ServerErrorKind::ExecAbort),
            ErrorKind::Server(ServerErrorKind::NoScript),
            ErrorKind::Server(ServerErrorKind::Moved),
            ErrorKind::Server(ServerErrorKind::Ask),
            ErrorKind::Server(ServerErrorKind::CrossSlot),
            ErrorKind::Server(ServerErrorKind::NotBusy),
            ErrorKind::Server(ServerErrorKind::NoSub),
            ErrorKind::Server(ServerErrorKind::NoPerm),
            ErrorKind::AuthenticationFailed,
            ErrorKind::InvalidClientConfig,
            ErrorKind::UnexpectedReturnType,
            ErrorKind::Client,
            ErrorKind::Extension,
            ErrorKind::RESP3NotSupported,
            ErrorKind::Parse,
        ] {
            assert!(
                !super::redis_error_indicates_unavailability(&error(kind)),
                "{kind:?} should not indicate unavailability"
            );
        }
    }
}

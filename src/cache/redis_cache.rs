//! 缓存实现：`redis_cache`。

use super::{CacheBackend, reservation::ReservationSet};
use crate::errors::AsterError;
use crate::utils::numbers::u128_to_u64;
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

pub struct RedisCache {
    conn: redis::aio::ConnectionManager,
    default_ttl: u64,
    reservations: ReservationSet,
    availability: RedisAvailability,
}

impl RedisCache {
    pub async fn new(url: &str, default_ttl: u64) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        let manager_config = redis::aio::ConnectionManagerConfig::new()
            .set_response_timeout(Some(REDIS_CACHE_OPERATION_TIMEOUT))
            .set_connection_timeout(Some(REDIS_CACHE_CONNECTION_TIMEOUT))
            .set_min_delay(REDIS_CACHE_RECONNECT_MIN_DELAY)
            .set_max_delay(REDIS_CACHE_RECONNECT_MAX_DELAY)
            .set_number_of_retries(REDIS_CACHE_RECONNECT_RETRIES);
        let conn = redis::aio::ConnectionManager::new_with_config(client, manager_config).await?;
        Ok(Self {
            conn,
            default_ttl,
            reservations: ReservationSet::new(default_ttl),
            availability: RedisAvailability::default(),
        })
    }

    async fn redis_operation<T, Fut>(&self, operation: &'static str, future: Fut) -> Option<T>
    where
        T: Send,
        Fut: Future<Output = redis::RedisResult<T>> + Send,
    {
        if let Some(remaining) = self.redis_unavailable_for() {
            tracing::trace!(
                operation,
                remaining_ms = u128_to_u64(
                    remaining.as_millis(),
                    "redis fallback remaining milliseconds",
                )
                .unwrap_or(u64::MAX),
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
                timeout_ms = u128_to_u64(
                    REDIS_CACHE_OPERATION_TIMEOUT.as_millis(),
                    "redis cache operation timeout milliseconds",
                )
                .unwrap_or(u64::MAX),
                cooldown_secs = REDIS_CACHE_FALLBACK_COOLDOWN.as_secs(),
                "redis cache operation timed out; using local fallback temporarily"
            );
        } else {
            tracing::debug!(
                operation,
                timeout_ms = u128_to_u64(
                    REDIS_CACHE_OPERATION_TIMEOUT.as_millis(),
                    "redis cache operation timeout milliseconds",
                )
                .unwrap_or(u64::MAX),
                "redis cache operation timed out while fallback circuit is already open"
            );
        }
    }
}

#[async_trait]
impl CacheBackend for RedisCache {
    fn backend_name(&self) -> &'static str {
        "redis"
    }

    async fn health_check(&self) -> crate::errors::Result<()> {
        if let Some(remaining) = self.redis_unavailable_for() {
            return Err(AsterError::internal_error(format!(
                "redis cache is in fallback mode for another {}ms",
                remaining.as_millis()
            )));
        }

        let mut conn = self.conn.clone();
        let ping_cmd = redis::cmd("PING");
        let ping = ping_cmd.query_async::<String>(&mut conn);
        match tokio::time::timeout(REDIS_CACHE_OPERATION_TIMEOUT, ping).await {
            Ok(Ok(_)) => {
                self.mark_redis_success("health_check");
                Ok(())
            }
            Ok(Err(error)) => {
                self.mark_redis_error("health_check", &error);
                Err(AsterError::internal_error(format!(
                    "redis cache health check: {error}"
                )))
            }
            Err(_) => {
                self.mark_redis_timeout("health_check");
                Err(AsterError::internal_error(format!(
                    "redis cache health check timed out after {}ms",
                    REDIS_CACHE_OPERATION_TIMEOUT.as_millis()
                )))
            }
        }
    }

    async fn get_bytes(&self, key: &str) -> Option<Vec<u8>> {
        let mut conn = self.conn.clone();
        self.redis_operation("get", conn.get::<_, Option<Vec<u8>>>(key))
            .await?
    }

    async fn set_bytes(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        let ttl = ttl_secs.unwrap_or(self.default_ttl);
        let mut conn = self.conn.clone();
        let _: Option<()> = self
            .redis_operation("set", conn.set_ex::<_, _, ()>(key, value, ttl))
            .await;
    }

    async fn set_bytes_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        let ttl = ttl_secs.unwrap_or(self.default_ttl);
        if !self.reservations.reserve(key, ttl_secs) {
            return false;
        }

        let options = SetOptions::default()
            .conditional_set(ExistenceCheck::NX)
            .with_expiration(SetExpiry::EX(ttl));
        let mut conn = self.conn.clone();
        let result = conn.set_options::<_, _, Option<String>>(key, value, options);
        match self.redis_operation("set_if_absent", result).await {
            Some(Some(_)) => true,
            Some(None) => {
                self.reservations.remove(key);
                false
            }
            None => true,
        }
    }

    async fn delete(&self, key: &str) {
        self.reservations.remove(key);
        let mut conn = self.conn.clone();
        let _: Option<()> = self.redis_operation("delete", conn.del::<_, ()>(key)).await;
    }

    async fn invalidate_prefix(&self, prefix: &str) {
        self.reservations.invalidate_prefix(prefix);
        let mut conn = self.conn.clone();
        let pattern = format!("{prefix}*");
        let mut cursor: u64 = 0;
        loop {
            let mut scan_cmd = redis::cmd("SCAN");
            let scan = scan_cmd
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async::<(u64, Vec<String>)>(&mut conn);
            let Some((next_cursor, keys)) =
                self.redis_operation("invalidate_prefix_scan", scan).await
            else {
                break;
            };
            if !keys.is_empty()
                && self
                    .redis_operation("invalidate_prefix_delete", conn.del::<_, ()>(&keys))
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
    use super::RedisAvailability;
    use std::time::{Duration, Instant};

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
}

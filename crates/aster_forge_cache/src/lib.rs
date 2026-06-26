//! Shared cache abstractions and backend constructors for Aster services.
//!
//! The public API is byte-oriented so cache backends can remain object-safe and easy to wrap in
//! `Arc<dyn CacheBackend>`. JSON convenience methods are provided as an extension trait for common
//! application values, while concrete memory and Redis implementations live behind feature-gated
//! modules.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

mod health;
#[cfg(feature = "memory")]
mod memory;
#[cfg(feature = "redis")]
mod redis_cache;
#[cfg(feature = "memory")]
mod reservation;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;

pub use health::{
    CACHE_COMPONENT, CACHE_HEALTH_CHECK, CACHE_HEALTH_CHECK_TIMEOUT, cache_health_options,
    check_cache_component, register_cache_health_check,
};
#[cfg(feature = "memory")]
pub use memory::MemoryCache;
#[cfg(feature = "redis")]
pub use redis_cache::RedisCache;

/// Result type returned by cache operations.
pub type Result<T> = std::result::Result<T, CacheError>;

/// Errors returned by cache construction and health checks.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// Redis could not be reached or initialized.
    #[error("redis cache connection: {0}")]
    RedisConnection(String),
    /// Redis is temporarily unavailable and the local fallback circuit is open.
    #[error("redis cache is in fallback mode for another {remaining_ms}ms")]
    RedisFallbackMode {
        /// Remaining fallback-circuit duration in milliseconds.
        remaining_ms: u128,
    },
    /// Redis health check returned an error.
    #[error("redis cache health check: {0}")]
    RedisHealthCheck(String),
    /// Redis health check timed out.
    #[error("redis cache health check timed out after {timeout_ms}ms")]
    RedisHealthCheckTimeout {
        /// Health-check timeout in milliseconds.
        timeout_ms: u128,
    },
}

#[cfg(feature = "redis")]
impl From<redis::RedisError> for CacheError {
    fn from(value: redis::RedisError) -> Self {
        Self::RedisConnection(value.to_string())
    }
}

const DEFAULT_CACHE_BACKEND: &str = "memory";
const DEFAULT_CACHE_TTL_SECS: u64 = 3600;

/// Configuration used to construct a cache backend.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CacheConfig {
    /// Backend name. Currently `memory` and `redis` are recognized.
    #[serde(default = "CacheConfig::default_backend")]
    pub backend: String,
    /// Redis connection URL used when `backend` is `redis`.
    #[serde(default)]
    pub redis_url: String,
    /// Default time-to-live, in seconds, for entries that do not specify an explicit TTL.
    #[serde(default = "CacheConfig::default_ttl")]
    pub default_ttl: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            backend: Self::default_backend(),
            redis_url: String::new(),
            default_ttl: Self::default_ttl(),
        }
    }
}

impl CacheConfig {
    fn default_backend() -> String {
        DEFAULT_CACHE_BACKEND.to_string()
    }

    const fn default_ttl() -> u64 {
        DEFAULT_CACHE_TTL_SECS
    }
}

fn redis_backend_target(redis_url: &str) -> String {
    let Some((scheme, rest)) = redis_url.split_once("://") else {
        return "configured".to_string();
    };

    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or(authority);
    if host.is_empty() {
        format!("{scheme}://configured")
    } else {
        format!("{scheme}://{host}")
    }
}

/// Object-safe cache backend trait that exposes a common byte-oriented API.
#[async_trait]
pub trait CacheBackend: Send + Sync {
    /// Returns the stable backend name.
    fn backend_name(&self) -> &'static str;
    /// Performs a lightweight backend health check.
    async fn health_check(&self) -> Result<()>;
    /// Reads a raw byte value by key.
    async fn get_bytes(&self, key: &str) -> Option<Vec<u8>>;
    /// Atomically reads and removes a raw byte value by key when supported.
    async fn take_bytes(&self, key: &str) -> Option<Vec<u8>>;
    /// Writes a raw byte value by key with an optional TTL in seconds.
    async fn set_bytes(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>);
    /// Writes a raw byte value only when the key is absent.
    async fn set_bytes_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool;
    /// Removes a key from the cache.
    async fn delete(&self, key: &str);
    /// Removes multiple keys from the cache.
    async fn delete_many(&self, keys: &[String]) {
        for key in keys {
            self.delete(key).await;
        }
    }
    /// Invalidates every key with the given prefix.
    async fn invalidate_prefix(&self, prefix: &str);
}

/// Convenience methods for JSON serialization and deserialization.
pub trait CacheExt {
    /// Reads and deserializes a JSON value from the cache.
    fn get<T: DeserializeOwned + Send>(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = Option<T>> + Send;

    /// Serializes and writes a JSON value to the cache.
    fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: Option<u64>,
    ) -> impl std::future::Future<Output = ()> + Send;

    /// Atomically reads, removes, and deserializes a JSON value from the cache.
    fn take<T: DeserializeOwned + Send>(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = Option<T>> + Send;
}

impl CacheExt for dyn CacheBackend {
    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Option<T> {
        let bytes = self.get_bytes(key).await?;
        serde_json::from_slice(&bytes).ok()
    }

    async fn set<T: Serialize + Send + Sync>(&self, key: &str, value: &T, ttl_secs: Option<u64>) {
        if let Ok(bytes) = serde_json::to_vec(value) {
            self.set_bytes(key, bytes, ttl_secs).await;
        }
    }

    async fn take<T: DeserializeOwned + Send>(&self, key: &str) -> Option<T> {
        let bytes = self.take_bytes(key).await?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Creates a cache backend from configuration.
pub async fn create_cache(config: &CacheConfig) -> Arc<dyn CacheBackend> {
    match config.backend.as_str() {
        #[cfg(feature = "redis")]
        "redis" => {
            match redis_cache::RedisCache::new(&config.redis_url, config.default_ttl).await {
                Ok(cache) => {
                    tracing::info!(
                        target = %redis_backend_target(&config.redis_url),
                        "cache backend: redis"
                    );
                    Arc::new(cache)
                }
                Err(e) => {
                    tracing::warn!("redis connection failed: {e}, falling back to memory cache");
                    create_memory_cache(config.default_ttl)
                }
            }
        }
        _ => {
            tracing::info!("cache backend: memory (ttl={}s)", config.default_ttl);
            create_memory_cache(config.default_ttl)
        }
    }
}

#[cfg(feature = "memory")]
fn create_memory_cache(default_ttl: u64) -> Arc<dyn CacheBackend> {
    Arc::new(memory::MemoryCache::new(default_ttl))
}

#[cfg(test)]
mod tests {
    use super::{CacheConfig, CacheError, create_cache, redis_backend_target};

    #[test]
    fn cache_config_default_uses_memory_backend() {
        let config = CacheConfig::default();

        assert_eq!(config.backend, "memory");
        assert_eq!(config.redis_url, "");
        assert_eq!(config.default_ttl, 3600);
    }

    #[test]
    fn cache_config_deserializes_missing_fields_with_defaults() {
        let config: CacheConfig =
            serde_json::from_str("{}").expect("empty cache config should use field defaults");

        assert_eq!(config, CacheConfig::default());
    }

    #[tokio::test]
    async fn create_cache_uses_memory_for_unknown_backend() {
        let cache = create_cache(&CacheConfig {
            backend: "unknown".to_string(),
            redis_url: "redis://127.0.0.1/".to_string(),
            default_ttl: 5,
        })
        .await;

        assert_eq!(cache.backend_name(), "memory");
        cache.health_check().await.expect("memory cache is healthy");
    }

    #[test]
    fn redis_backend_target_strips_credentials() {
        assert_eq!(
            redis_backend_target("redis://user:secret@example.com:6379/0"),
            "redis://example.com:6379"
        );
    }

    #[test]
    fn redis_backend_target_keeps_host_without_credentials() {
        assert_eq!(
            redis_backend_target("rediss://cache.internal:6380/1"),
            "rediss://cache.internal:6380"
        );
    }

    #[test]
    fn redis_backend_target_handles_malformed_or_empty_hosts() {
        assert_eq!(redis_backend_target("not-a-url"), "configured");
        assert_eq!(redis_backend_target("redis:///0"), "redis://configured");
    }

    #[test]
    fn cache_error_display_messages_are_stable() {
        assert_eq!(
            CacheError::RedisConnection("refused".to_string()).to_string(),
            "redis cache connection: refused"
        );
        assert_eq!(
            CacheError::RedisFallbackMode { remaining_ms: 25 }.to_string(),
            "redis cache is in fallback mode for another 25ms"
        );
        assert_eq!(
            CacheError::RedisHealthCheck("PONG missing".to_string()).to_string(),
            "redis cache health check: PONG missing"
        );
        assert_eq!(
            CacheError::RedisHealthCheckTimeout { timeout_ms: 250 }.to_string(),
            "redis cache health check timed out after 250ms"
        );
    }
}

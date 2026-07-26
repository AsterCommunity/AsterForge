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

#[cfg(feature = "bloom")]
pub mod bloom;
#[cfg(feature = "runtime-component")]
mod health;
#[cfg(feature = "memory")]
mod memory;
#[cfg(feature = "redis")]
mod redis_cache;
#[cfg(feature = "memory")]
mod reservation;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use std::borrow::Cow;
#[cfg(feature = "memory")]
use std::sync::Arc;

#[cfg(feature = "runtime-component")]
pub use health::{
    CACHE_COMPONENT, CACHE_HEALTH_CHECK, CACHE_HEALTH_CHECK_TIMEOUT, CacheHealthComponent,
    cache_health_component, cache_health_options, check_cache_component,
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
    /// Redis configuration is invalid before a connection attempt can begin.
    #[error("redis cache configuration: {0}")]
    RedisConfiguration(String),
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
#[derive(Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CacheConfig {
    /// Backend name. Currently `memory` and `redis` are recognized.
    #[serde(default = "CacheConfig::default_backend")]
    pub backend: String,
    /// Backend endpoint. Redis uses a Redis connection URL.
    #[serde(default, alias = "redis_url")]
    pub endpoint: String,
    /// Optional raw credentials for a credential-free Redis endpoint.
    ///
    /// When set, `endpoint` must be empty so complete URLs and separated credentials cannot be
    /// selected through an implicit precedence rule.
    #[serde(default)]
    pub raw_redis_credentials: Option<RedisCredentials>,
    /// Default time-to-live, in seconds, for entries that do not specify an explicit TTL.
    #[serde(default = "CacheConfig::default_ttl")]
    pub default_ttl: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            backend: Self::default_backend(),
            endpoint: String::new(),
            raw_redis_credentials: None,
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

    /// Returns the normalized backend name used by construction, validation, and health checks.
    pub fn normalized_backend(&self) -> Cow<'_, str> {
        let backend = self.backend.trim();
        if backend.eq_ignore_ascii_case("memory") {
            Cow::Borrowed("memory")
        } else if backend.eq_ignore_ascii_case("redis") {
            Cow::Borrowed("redis")
        } else if backend.bytes().all(|byte| !byte.is_ascii_uppercase()) {
            Cow::Borrowed(backend)
        } else {
            Cow::Owned(backend.to_ascii_lowercase())
        }
    }

    #[cfg(feature = "redis")]
    fn resolved_redis_url(&self) -> Result<String> {
        match &self.raw_redis_credentials {
            Some(credentials) => {
                if !self.endpoint.is_empty() {
                    return Err(CacheError::RedisConfiguration(
                        "cache.endpoint and cache.raw_redis_credentials cannot both be configured"
                            .to_string(),
                    ));
                }
                aster_forge_utils::url::url_with_credentials(
                    &credentials.base_url,
                    credentials.username.as_deref(),
                    credentials.password.as_deref(),
                    "cache.raw_redis_credentials.base_url",
                )
                .map(|url| url.to_string())
                .map_err(|error| CacheError::RedisConfiguration(error.to_string()))
            }
            None => Ok(self.endpoint.clone()),
        }
    }
}

impl std::fmt::Debug for CacheConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CacheConfig")
            .field("backend", &self.backend)
            .field("connection", &"<redacted>")
            .field("default_ttl", &self.default_ttl)
            .finish()
    }
}

/// Raw Redis credentials for a credential-free cache endpoint.
#[derive(Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct RedisCredentials {
    /// Absolute Redis URL without userinfo.
    pub base_url: String,
    /// Raw Redis username, when ACL authentication is used.
    #[serde(default)]
    pub username: Option<String>,
    /// Raw Redis password.
    #[serde(default)]
    pub password: Option<String>,
}

impl std::fmt::Debug for RedisCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RedisCredentials(<redacted>)")
    }
}

#[cfg(feature = "redis")]
fn redis_backend_target(endpoint: &str) -> String {
    let Some((scheme, rest)) = endpoint.split_once("://") else {
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
    ///
    /// `None` uses the backend default TTL. A TTL of `0` expires immediately, so the
    /// observable result matches deleting the key (Redis rejects `SETEX 0`; backends
    /// normalize the contract instead of issuing an invalid command).
    async fn set_bytes(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>);
    /// Writes a raw byte value only when the key is absent.
    ///
    /// With a TTL of `0` the value expires immediately, so the call reports whether a
    /// live value existed and retains nothing either way.
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
#[cfg(feature = "memory")]
pub async fn create_cache(config: &CacheConfig) -> Arc<dyn CacheBackend> {
    match config.normalized_backend().as_ref() {
        #[cfg(feature = "redis")]
        "redis" => match config.resolved_redis_url() {
            Ok(url) => match redis_cache::RedisCache::new(&url, config.default_ttl).await {
                Ok(cache) => {
                    tracing::info!(
                        target = %redis_backend_target(&url),
                        "cache backend: redis"
                    );
                    Arc::new(cache)
                }
                Err(error) => {
                    tracing::warn!(%error, "redis connection failed, falling back to memory cache");
                    create_memory_cache(config.default_ttl)
                }
            },
            Err(error) => {
                tracing::warn!(%error, "invalid Redis cache configuration, falling back to memory cache");
                create_memory_cache(config.default_ttl)
            }
        },
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
    use super::{CacheConfig, CacheError};

    #[test]
    fn cache_config_default_uses_memory_backend() {
        let config = CacheConfig::default();

        assert_eq!(config.backend, "memory");
        assert_eq!(config.endpoint, "");
        assert_eq!(config.default_ttl, 3600);
    }

    #[test]
    fn cache_config_deserializes_missing_fields_with_defaults() {
        let config: CacheConfig =
            serde_json::from_str("{}").expect("empty cache config should use field defaults");

        assert_eq!(config, CacheConfig::default());
    }

    #[test]
    fn cache_config_deserializes_endpoint_field() {
        let config: CacheConfig = serde_json::from_str(
            r#"{"backend":"redis","endpoint":"redis://127.0.0.1/","default_ttl":30}"#,
        )
        .expect("cache config should accept the endpoint field");

        assert_eq!(config.backend, "redis");
        assert_eq!(config.endpoint, "redis://127.0.0.1/");
        assert_eq!(config.default_ttl, 30);
    }

    #[test]
    fn cache_config_accepts_legacy_redis_url_alias() {
        let config: CacheConfig = serde_json::from_str(
            r#"{"backend":"redis","redis_url":"redis://127.0.0.1/","default_ttl":30}"#,
        )
        .expect("cache config should accept legacy redis_url config files");

        assert_eq!(config.backend, "redis");
        assert_eq!(config.endpoint, "redis://127.0.0.1/");
        assert_eq!(config.default_ttl, 30);
    }

    #[test]
    fn cache_backend_normalization_trims_and_folds_ascii_case() {
        for (backend, expected) in [
            (" memory ", "memory"),
            (" ReDiS ", "redis"),
            (" CUSTOM-BACKEND ", "custom-backend"),
            (" \n\t", ""),
        ] {
            let config = CacheConfig {
                backend: backend.to_string(),
                ..CacheConfig::default()
            };
            assert_eq!(config.normalized_backend(), expected);
        }
    }

    #[cfg(feature = "memory")]
    #[tokio::test]
    async fn create_cache_uses_normalized_memory_backend() {
        let cache = super::create_cache(&CacheConfig {
            backend: " MeMoRy ".to_string(),
            ..CacheConfig::default()
        })
        .await;

        assert_eq!(cache.backend_name(), "memory");
    }

    #[cfg(feature = "memory")]
    #[tokio::test]
    async fn create_cache_uses_memory_for_unknown_backend() {
        let cache = super::create_cache(&CacheConfig {
            backend: "unknown".to_string(),
            endpoint: "redis://127.0.0.1/".to_string(),
            raw_redis_credentials: None,
            default_ttl: 5,
        })
        .await;

        assert_eq!(cache.backend_name(), "memory");
        cache.health_check().await.expect("memory cache is healthy");
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_backend_target_strips_credentials() {
        assert_eq!(
            super::redis_backend_target("redis://user:secret@example.com:6379/0"),
            "redis://example.com:6379"
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_backend_target_keeps_host_without_credentials() {
        assert_eq!(
            super::redis_backend_target("rediss://cache.internal:6380/1"),
            "rediss://cache.internal:6380"
        );
    }

    #[cfg(feature = "redis")]
    #[test]
    fn redis_backend_target_handles_malformed_or_empty_hosts() {
        assert_eq!(super::redis_backend_target("not-a-url"), "configured");
        assert_eq!(
            super::redis_backend_target("redis:///0"),
            "redis://configured"
        );
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

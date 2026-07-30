use serde::{Deserialize, Serialize};
#[cfg(feature = "redis-pubsub")]
use std::sync::Arc;

use crate::{ConfigCoreError, Result};

#[cfg(feature = "redis-pubsub")]
use super::notifier::RedisConfigChangeNotifier;
#[cfg(feature = "redis-pubsub")]
use super::notifier::SharedConfigChangeNotifier;
use super::runtime::ConfigSyncRuntime;

/// Disabled config-sync backend name.
pub const CONFIG_SYNC_BACKEND_DISABLED: &str = "disabled";
/// Redis pub/sub config-sync backend name.
pub const CONFIG_SYNC_BACKEND_REDIS: &str = "redis";

/// Config-sync broker endpoint input.
///
/// Existing string endpoints remain valid. The structured form carries a base URL without
/// userinfo and raw credentials that the selected transport injects safely.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigSyncEndpoint {
    /// A complete broker URL.
    Url(String),
    /// A base broker URL without userinfo plus raw credentials.
    Credentials {
        /// Absolute broker URL without username or password.
        base_url: String,
        /// Raw broker username.
        #[serde(default, skip_serializing)]
        username: Option<String>,
        /// Raw broker password.
        #[serde(default, skip_serializing)]
        password: Option<String>,
    },
}

impl ConfigSyncEndpoint {
    /// Creates a complete-URL endpoint.
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }

    /// Creates a base URL plus raw credentials endpoint.
    pub fn credentials(
        base_url: impl Into<String>,
        username: Option<String>,
        password: Option<String>,
    ) -> Self {
        Self::Credentials {
            base_url: base_url.into(),
            username,
            password,
        }
    }
}

impl Default for ConfigSyncEndpoint {
    fn default() -> Self {
        Self::Url(String::new())
    }
}

impl From<String> for ConfigSyncEndpoint {
    fn from(url: String) -> Self {
        Self::Url(url)
    }
}

impl From<&str> for ConfigSyncEndpoint {
    fn from(url: &str) -> Self {
        Self::Url(url.to_string())
    }
}

impl std::fmt::Debug for ConfigSyncEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(_) => formatter.write_str("ConfigSyncEndpoint::Url(<redacted>)"),
            Self::Credentials { .. } => {
                formatter.write_str("ConfigSyncEndpoint::Credentials(<redacted>)")
            }
        }
    }
}

/// Static configuration for cross-process config reload synchronization.
///
/// The field names describe a generic broker contract instead of a Redis-only
/// shape. Current services can map `backend = "redis"` to Redis pub/sub, while
/// future `RabbitMQ`, NATS, or other transports can reuse the same product config
/// surface and add backend-specific interpretation behind the notifier factory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSyncConfig {
    /// Transport backend name, for example `disabled` or `redis`.
    #[serde(default = "ConfigSyncConfig::default_backend")]
    pub backend: String,
    /// Broker endpoint URL. Redis uses a Redis URL.
    #[serde(default)]
    pub endpoint: ConfigSyncEndpoint,
    /// Logical reload topic. Transports may map this to a channel, exchange,
    /// subject, or routing key.
    #[serde(default = "ConfigSyncConfig::default_topic")]
    pub topic: String,
}

impl Default for ConfigSyncConfig {
    fn default() -> Self {
        Self {
            backend: Self::default_backend(),
            endpoint: ConfigSyncEndpoint::default(),
            topic: Self::default_topic(),
        }
    }
}

impl ConfigSyncConfig {
    /// Returns the default disabled backend name.
    #[must_use]
    pub fn default_backend() -> String {
        CONFIG_SYNC_BACKEND_DISABLED.to_string()
    }

    /// Returns the default logical reload topic.
    #[must_use]
    pub fn default_topic() -> String {
        "aster.config_reload".to_string()
    }

    /// Returns whether cross-process sync is enabled.
    #[must_use]
    pub fn enabled(&self) -> bool {
        !matches!(
            self.backend.trim().to_ascii_lowercase().as_str(),
            "" | "disabled" | "none"
        )
    }
}

/// Returns the conventional config-sync topic for a product namespace.
#[must_use]
pub fn default_config_sync_topic(namespace: &str) -> String {
    format!("{}.config_reload", namespace.trim())
}

/// Builds a namespaced config-sync runtime from static config.
///
/// This common backend factory owns backend dispatch, runtime ID generation, and
/// transport-specific topic mapping. Product crates only pass their namespace
/// and provide their reload callback to [`ConfigSyncRuntime::run_reload_subscription`].
///
/// # Errors
///
/// Returns [`ConfigError`] when the sync backend, endpoint, topic, credentials, or runtime id is invalid.
pub fn build_config_sync_runtime(
    config: &ConfigSyncConfig,
    namespace: &str,
) -> Result<ConfigSyncRuntime> {
    build_config_sync_runtime_with_runtime_id(
        config,
        namespace,
        aster_forge_utils::id::new_runtime_id(),
    )
}

/// Builds a namespaced config-sync runtime with an explicit runtime ID.
///
/// Products normally use [`build_config_sync_runtime`]. This variant is useful when the product
/// already has a stable process identity or when tests need deterministic self-origin filtering.
///
/// # Errors
///
/// Returns [`ConfigError`] when the sync backend, endpoint, topic, credentials, or runtime id is invalid.
pub fn build_config_sync_runtime_with_runtime_id(
    config: &ConfigSyncConfig,
    namespace: &str,
    runtime_id: impl Into<String>,
) -> Result<ConfigSyncRuntime> {
    let namespace = namespace.trim();
    let runtime_id = runtime_id.into();
    let topic = config_sync_topic(config, namespace);
    match config.backend.trim().to_ascii_lowercase().as_str() {
        "" | "disabled" | "none" => Ok(ConfigSyncRuntime::disabled_with_runtime_id(
            namespace, runtime_id,
        )),
        CONFIG_SYNC_BACKEND_REDIS => {
            build_redis_config_sync_runtime(config, namespace, runtime_id, &topic)
        }
        backend => Err(ConfigCoreError::invalid_value(format!(
            "unsupported config_sync.backend '{backend}'"
        ))),
    }
}
fn config_sync_topic(config: &ConfigSyncConfig, namespace: &str) -> String {
    let topic = config.topic.trim();
    if topic.is_empty() || topic == ConfigSyncConfig::default_topic() {
        default_config_sync_topic(namespace)
    } else {
        topic.to_string()
    }
}

#[cfg(feature = "redis-pubsub")]
fn build_redis_config_sync_runtime(
    config: &ConfigSyncConfig,
    namespace: &str,
    runtime_id: String,
    topic: &str,
) -> Result<ConfigSyncRuntime> {
    let channel = redis_channel_from_topic(topic);
    let notifier = match &config.endpoint {
        ConfigSyncEndpoint::Url(endpoint) => {
            if endpoint.trim().is_empty() {
                return Err(ConfigCoreError::invalid_value(
                    "config_sync.endpoint is required when config_sync.backend is redis",
                ));
            }
            RedisConfigChangeNotifier::from_url(endpoint.trim(), channel)?
        }
        ConfigSyncEndpoint::Credentials {
            base_url,
            username,
            password,
        } => {
            if base_url.trim().is_empty() {
                return Err(ConfigCoreError::invalid_value(
                    "config_sync.endpoint base_url is required when config_sync.backend is redis",
                ));
            }
            RedisConfigChangeNotifier::from_credentials(
                base_url.trim(),
                username.as_deref(),
                password.as_deref(),
                channel,
            )?
        }
    };
    Ok(ConfigSyncRuntime::new(
        namespace,
        runtime_id,
        Arc::new(notifier) as SharedConfigChangeNotifier,
    ))
}

#[cfg(not(feature = "redis-pubsub"))]
fn build_redis_config_sync_runtime(
    _config: &ConfigSyncConfig,
    _namespace: &str,
    _runtime_id: String,
    _topic: &str,
) -> Result<ConfigSyncRuntime> {
    Err(ConfigCoreError::invalid_value(
        "config_sync.backend 'redis' requires the redis-pubsub feature",
    ))
}

#[cfg(any(feature = "redis-pubsub", test))]
pub(super) fn redis_channel_from_topic(topic: &str) -> String {
    topic.trim().replace('.', ":")
}

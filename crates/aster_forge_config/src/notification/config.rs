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

/// Static configuration for cross-process config reload synchronization.
///
/// The field names describe a generic broker contract instead of a Redis-only
/// shape. Current services can map `backend = "redis"` to Redis pub/sub, while
/// future RabbitMQ, NATS, or other transports can reuse the same product config
/// surface and add backend-specific interpretation behind the notifier factory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSyncConfig {
    /// Transport backend name, for example `disabled` or `redis`.
    #[serde(default = "ConfigSyncConfig::default_backend")]
    pub backend: String,
    /// Broker endpoint URL. Redis uses a Redis URL.
    #[serde(default)]
    pub endpoint: String,
    /// Logical reload topic. Transports may map this to a channel, exchange,
    /// subject, or routing key.
    #[serde(default = "ConfigSyncConfig::default_topic")]
    pub topic: String,
}

impl Default for ConfigSyncConfig {
    fn default() -> Self {
        Self {
            backend: Self::default_backend(),
            endpoint: String::new(),
            topic: Self::default_topic(),
        }
    }
}

impl ConfigSyncConfig {
    /// Returns the default disabled backend name.
    pub fn default_backend() -> String {
        CONFIG_SYNC_BACKEND_DISABLED.to_string()
    }

    /// Returns the default logical reload topic.
    pub fn default_topic() -> String {
        "aster.config_reload".to_string()
    }

    /// Returns whether cross-process sync is enabled.
    pub fn enabled(&self) -> bool {
        !matches!(
            self.backend.trim().to_ascii_lowercase().as_str(),
            "" | "disabled" | "none"
        )
    }
}

/// Returns the conventional config-sync topic for a product namespace.
pub fn default_config_sync_topic(namespace: &str) -> String {
    format!("{}.config_reload", namespace.trim())
}

/// Builds a namespaced config-sync runtime from static config.
///
/// This common backend factory owns backend dispatch, runtime ID generation, and
/// transport-specific topic mapping. Product crates only pass their namespace
/// and provide their reload callback to [`ConfigSyncRuntime::run_reload_subscription`].
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
    if config.endpoint.trim().is_empty() {
        return Err(ConfigCoreError::invalid_value(
            "config_sync.endpoint is required when config_sync.backend is redis",
        ));
    }
    let notifier = RedisConfigChangeNotifier::from_url(
        config.endpoint.trim(),
        redis_channel_from_topic(topic),
    )?;
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

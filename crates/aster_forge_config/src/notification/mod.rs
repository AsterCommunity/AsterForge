//! Runtime configuration reload notifications.
//!
//! A running service can apply a local configuration change immediately, but
//! other processes need a lightweight signal telling them to reload from their
//! authoritative store. This module defines that signal and provides both an
//! in-memory notifier for tests/single-process deployments and an optional
//! Redis pub/sub transport for multi-process deployments.
//!
//! Subscription loops are supervised: subscribe failures, transport stream
//! endings, and local broadcast lag all trigger a bounded backoff, a fresh
//! subscription, and one authoritative reconcile, so a Redis hiccup or a slow
//! reload handler cannot permanently kill cross-process synchronization. The
//! loop only exits when its shutdown token is cancelled.

mod config;
mod message;
mod notifier;
mod runtime;
mod supervisor;

pub use config::{
    CONFIG_SYNC_BACKEND_DISABLED, CONFIG_SYNC_BACKEND_REDIS, ConfigSyncConfig,
    build_config_sync_runtime, build_config_sync_runtime_with_runtime_id,
    default_config_sync_topic,
};
pub use message::{
    ConfigChangeEvent, ConfigNotificationSource, ConfigReloadDecision, ConfigReloadMessage,
    ConfigReloadWorkerConfig, decode_config_reload_transport_payload,
    handle_config_reload_notification,
};
#[cfg(feature = "redis-pubsub")]
pub use notifier::RedisConfigChangeNotifier;
pub use notifier::{
    ConfigChangeNotifier, ConfigNotification, InMemoryConfigNotifier, SharedConfigChangeNotifier,
};
pub use runtime::ConfigSyncRuntime;
pub use supervisor::{
    ConfigReloadObservation, ConfigReloadObserver, ConfigSyncConnectionObservation,
    ConfigSyncConnectionObserver, ConfigSyncConnectionState, run_config_reload_supervisor,
    run_config_reload_supervisor_with_observers, run_config_reload_worker,
    run_config_reload_worker_with_observer,
};

#[cfg(test)]
use crate::Result;
#[cfg(test)]
use config::redis_channel_from_topic;
#[cfg(test)]
use supervisor::{
    ConfigReloadReconnectPolicy, config_reload_reconnect_delay, duration_millis_u64,
    run_config_reload_supervisor_inner,
};

#[cfg(test)]
mod tests;

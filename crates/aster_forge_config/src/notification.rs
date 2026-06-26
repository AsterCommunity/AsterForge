//! Runtime configuration reload notifications.
//!
//! A running service can apply a local configuration change immediately, but
//! other processes need a lightweight signal telling them to reload from their
//! authoritative store. This module defines that signal and provides both an
//! in-memory notifier for tests/single-process deployments and an optional
//! Redis pub/sub transport for multi-process deployments.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::{ConfigCoreError, Result};

/// Disabled config-sync backend name.
pub const CONFIG_SYNC_BACKEND_DISABLED: &str = "disabled";
/// Redis pub/sub config-sync backend name.
pub const CONFIG_SYNC_BACKEND_REDIS: &str = "redis";

/// Source that emitted a configuration reload notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigNotificationSource {
    /// Notification was emitted by an API mutation.
    Api,
    /// Notification was emitted by a CLI operation.
    Cli,
    /// Notification was emitted by a startup/bootstrap path.
    Startup,
    /// Notification was emitted by an unspecified or product-specific source.
    Other(String),
}

/// Notification payload published after configuration changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigReloadMessage {
    /// Product or service namespace, for example `aster_yggdrasil`.
    pub namespace: String,
    /// Runtime instance ID that emitted the message. Receivers can use this to ignore
    /// their own message after already applying the local change.
    pub origin_runtime_id: String,
    /// Changed keys. Empty means receivers should reload all runtime config.
    pub keys: Vec<String>,
    /// Source of the change.
    pub source: ConfigNotificationSource,
}

impl ConfigReloadMessage {
    /// Creates a reload message and sorts/deduplicates keys.
    pub fn new(
        namespace: impl Into<String>,
        origin_runtime_id: impl Into<String>,
        keys: impl IntoIterator<Item = impl Into<String>>,
        source: ConfigNotificationSource,
    ) -> Self {
        let mut keys = keys.into_iter().map(Into::into).collect::<Vec<_>>();
        keys.sort();
        keys.dedup();
        Self {
            namespace: namespace.into(),
            origin_runtime_id: origin_runtime_id.into(),
            keys,
            source,
        }
    }

    /// Serializes the message for transport.
    pub fn encode(&self) -> Result<String> {
        serde_json::to_string(self).map_err(Into::into)
    }

    /// Decodes a transport payload.
    pub fn decode(payload: &str) -> Result<Self> {
        serde_json::from_str(payload).map_err(Into::into)
    }
}

/// Local event delivered by a notifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigChangeEvent {
    /// Receivers should reload from storage.
    Reload(ConfigReloadMessage),
}

impl ConfigChangeEvent {
    /// Returns the reload message carried by this event.
    pub const fn reload_message(&self) -> &ConfigReloadMessage {
        match self {
            Self::Reload(message) => message,
        }
    }
}

/// Subscription returned by config notifiers.
pub struct ConfigNotification {
    receiver: broadcast::Receiver<ConfigChangeEvent>,
}

impl ConfigNotification {
    fn new(receiver: broadcast::Receiver<ConfigChangeEvent>) -> Self {
        Self { receiver }
    }

    /// Waits for the next notification.
    pub async fn recv(&mut self) -> Result<ConfigChangeEvent> {
        self.receiver
            .recv()
            .await
            .map_err(|error| ConfigCoreError::notification(error.to_string()))
    }
}

/// Result of handling one reload notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigReloadDecision {
    /// The notification matched this process and triggered a reload.
    Reloaded,
    /// The notification belongs to another product namespace.
    IgnoredNamespace,
    /// The notification came from this process and should not be replayed.
    IgnoredOrigin,
}

/// Runtime reload worker configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReloadWorkerConfig {
    /// Product or service namespace accepted by this worker.
    pub namespace: String,
    /// Runtime instance ID for the current process.
    pub runtime_id: String,
}

impl ConfigReloadWorkerConfig {
    /// Creates a worker config.
    pub fn new(namespace: impl Into<String>, runtime_id: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            runtime_id: runtime_id.into(),
        }
    }

    /// Returns whether a message belongs to this worker namespace.
    pub fn accepts_namespace(&self, message: &ConfigReloadMessage) -> bool {
        message.namespace == self.namespace
    }

    /// Returns whether a message was emitted by this process.
    pub fn is_local_origin(&self, message: &ConfigReloadMessage) -> bool {
        message.origin_runtime_id == self.runtime_id
    }
}

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
    let namespace = namespace.trim();
    let runtime_id = aster_forge_utils::id::new_runtime_id();
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
fn redis_channel_from_topic(topic: &str) -> String {
    topic.trim().replace('.', ":")
}

/// Handles one reload notification by filtering namespace/origin and invoking `reload`.
pub async fn handle_config_reload_notification<F, Fut>(
    config: &ConfigReloadWorkerConfig,
    message: ConfigReloadMessage,
    reload: F,
) -> Result<ConfigReloadDecision>
where
    F: FnOnce(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    if !config.accepts_namespace(&message) {
        return Ok(ConfigReloadDecision::IgnoredNamespace);
    }
    if config.is_local_origin(&message) {
        return Ok(ConfigReloadDecision::IgnoredOrigin);
    }

    reload(message).await?;
    Ok(ConfigReloadDecision::Reloaded)
}

/// Runs a reload subscription loop until `shutdown` is cancelled.
///
/// The loop never carries configuration values over pub/sub. A matching
/// notification only tells this process to reload from its authoritative store.
/// Reload errors are logged and the loop keeps listening, because one failed DB
/// read should not permanently break cross-process synchronization.
pub async fn run_config_reload_worker<N, F, Fut>(
    notifier: Arc<N>,
    config: ConfigReloadWorkerConfig,
    shutdown: CancellationToken,
    mut reload: F,
) -> Result<()>
where
    N: ConfigChangeNotifier + ?Sized,
    F: FnMut(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut subscription = notifier.subscribe().await?;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                return Ok(());
            }
            event = subscription.recv() => {
                let event = event?;
                match event {
                    ConfigChangeEvent::Reload(message) => {
                        match handle_config_reload_notification(&config, message, &mut reload).await {
                            Ok(ConfigReloadDecision::Reloaded) => {
                                tracing::debug!("runtime config reloaded after remote notification");
                            }
                            Ok(ConfigReloadDecision::IgnoredNamespace | ConfigReloadDecision::IgnoredOrigin) => {}
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    "failed to reload runtime config after remote notification"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Transport used to publish and subscribe to reload notifications.
#[async_trait]
pub trait ConfigChangeNotifier: Send + Sync {
    /// Publishes a reload notification.
    async fn publish_reload(&self, message: ConfigReloadMessage) -> Result<()>;

    /// Subscribes to future reload notifications.
    async fn subscribe(&self) -> Result<ConfigNotification>;
}

/// Shared notifier object used by runtime services.
pub type SharedConfigChangeNotifier = Arc<dyn ConfigChangeNotifier>;

/// Namespaced runtime handle for cross-process config synchronization.
///
/// This type is the product-facing boundary for config sync. It keeps the
/// namespace, runtime identity, backend notifier, publish helper, and subscription
/// worker wiring together so product crates only provide their authoritative
/// reload callback.
#[derive(Clone)]
pub struct ConfigSyncRuntime {
    namespace: String,
    runtime_id: String,
    notifier: Option<SharedConfigChangeNotifier>,
}

impl ConfigSyncRuntime {
    /// Creates an enabled runtime from a namespace, runtime ID, and notifier.
    pub fn new(
        namespace: impl Into<String>,
        runtime_id: impl Into<String>,
        notifier: impl Into<SharedConfigChangeNotifier>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            runtime_id: runtime_id.into(),
            notifier: Some(notifier.into()),
        }
    }

    /// Creates a disabled runtime with a generated runtime ID.
    pub fn disabled(namespace: impl Into<String>) -> Self {
        Self::disabled_with_runtime_id(namespace, aster_forge_utils::id::new_runtime_id())
    }

    /// Creates a disabled runtime with an explicit runtime ID.
    pub fn disabled_with_runtime_id(
        namespace: impl Into<String>,
        runtime_id: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            runtime_id: runtime_id.into(),
            notifier: None,
        }
    }

    /// Creates a disabled runtime for tests and single-process defaults.
    pub fn disabled_for_test(namespace: impl Into<String>) -> Self {
        Self::disabled_with_runtime_id(namespace, "test-runtime")
    }

    /// Creates an enabled runtime from an explicit notifier for tests.
    pub fn with_notifier_for_test(
        namespace: impl Into<String>,
        runtime_id: impl Into<String>,
        notifier: impl Into<SharedConfigChangeNotifier>,
    ) -> Self {
        Self::new(namespace, runtime_id, notifier)
    }

    /// Returns the product namespace this runtime accepts and publishes.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the process runtime ID.
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    /// Returns the configured notifier, if cross-process sync is enabled.
    pub fn notifier(&self) -> Option<&SharedConfigChangeNotifier> {
        self.notifier.as_ref()
    }

    /// Returns whether config sync is enabled.
    pub fn enabled(&self) -> bool {
        self.notifier.is_some()
    }

    /// Converts this runtime into the reload-worker filter configuration.
    pub fn worker_config(&self) -> ConfigReloadWorkerConfig {
        ConfigReloadWorkerConfig::new(self.namespace(), self.runtime_id())
    }

    /// Publishes a reload hint after a local config mutation.
    pub async fn publish_reload(
        &self,
        keys: impl IntoIterator<Item = impl Into<String>>,
        source: ConfigNotificationSource,
    ) -> Result<()> {
        let Some(notifier) = self.notifier() else {
            return Ok(());
        };

        notifier
            .publish_reload(ConfigReloadMessage::new(
                self.namespace(),
                self.runtime_id(),
                keys,
                source,
            ))
            .await
    }

    /// Runs this runtime's reload subscription worker until shutdown.
    ///
    /// Disabled runtimes simply wait for shutdown, which lets callers spawn the
    /// same task unconditionally if that is more convenient.
    pub async fn run_reload_subscription<F, Fut>(
        &self,
        shutdown: CancellationToken,
        reload: F,
    ) -> Result<()>
    where
        F: FnMut(ConfigReloadMessage) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let Some(notifier) = self.notifier().cloned() else {
            shutdown.cancelled().await;
            return Ok(());
        };
        run_config_reload_worker(notifier, self.worker_config(), shutdown, reload).await
    }
}

/// In-memory notifier for single-process deployments and tests.
#[derive(Debug, Clone)]
pub struct InMemoryConfigNotifier {
    sender: broadcast::Sender<ConfigChangeEvent>,
}

impl InMemoryConfigNotifier {
    /// Creates a notifier with the given broadcast channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }
}

impl Default for InMemoryConfigNotifier {
    fn default() -> Self {
        Self::new(128)
    }
}

#[async_trait]
impl ConfigChangeNotifier for InMemoryConfigNotifier {
    async fn publish_reload(&self, message: ConfigReloadMessage) -> Result<()> {
        self.sender
            .send(ConfigChangeEvent::Reload(message))
            .map(|_| ())
            .map_err(|error| ConfigCoreError::notification(error.to_string()))
    }

    async fn subscribe(&self) -> Result<ConfigNotification> {
        Ok(ConfigNotification::new(self.sender.subscribe()))
    }
}

#[cfg(feature = "redis-pubsub")]
mod redis_transport {
    use futures::StreamExt;
    use redis::AsyncCommands;
    use tokio::sync::broadcast;

    use super::{ConfigChangeEvent, ConfigChangeNotifier, ConfigNotification, ConfigReloadMessage};
    use crate::{ConfigCoreError, Result};

    /// Redis pub/sub publisher for configuration reload messages.
    #[derive(Clone)]
    pub struct RedisConfigChangeNotifier {
        client: redis::Client,
        channel: String,
    }

    impl RedisConfigChangeNotifier {
        /// Creates a Redis notifier for `channel`.
        pub fn new(client: redis::Client, channel: impl Into<String>) -> Self {
            Self {
                client,
                channel: channel.into(),
            }
        }

        /// Creates a Redis notifier from a Redis connection URL.
        pub fn from_url(url: &str, channel: impl Into<String>) -> Result<Self> {
            let client = redis::Client::open(url).map_err(|error| {
                ConfigCoreError::notification(format!("open Redis URL: {error}"))
            })?;
            Ok(Self::new(client, channel))
        }
    }

    #[async_trait::async_trait]
    impl ConfigChangeNotifier for RedisConfigChangeNotifier {
        async fn publish_reload(&self, message: ConfigReloadMessage) -> Result<()> {
            let payload = message.encode()?;
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_err(|error| {
                    ConfigCoreError::notification(format!("connect to Redis: {error}"))
                })?;
            let _: usize = connection
                .publish(&self.channel, payload)
                .await
                .map_err(|error| {
                    ConfigCoreError::notification(format!("publish config reload: {error}"))
                })?;
            Ok(())
        }

        async fn subscribe(&self) -> Result<ConfigNotification> {
            let listener =
                RedisConfigReloadListener::spawn(self.client.clone(), self.channel.clone(), 128)
                    .await?;
            Ok(listener.into_notification())
        }
    }

    /// Background Redis pub/sub listener.
    pub struct RedisConfigReloadListener {
        receiver: broadcast::Receiver<ConfigChangeEvent>,
    }

    impl RedisConfigReloadListener {
        /// Starts listening for reload notifications on `channel`.
        pub async fn spawn(
            client: redis::Client,
            channel: impl Into<String>,
            capacity: usize,
        ) -> Result<Self> {
            let channel = channel.into();
            let mut pubsub = client.get_async_pubsub().await.map_err(|error| {
                ConfigCoreError::notification(format!("connect Redis pubsub: {error}"))
            })?;
            pubsub.subscribe(&channel).await.map_err(|error| {
                ConfigCoreError::notification(format!(
                    "subscribe Redis channel '{channel}': {error}"
                ))
            })?;

            let (sender, receiver) = broadcast::channel(capacity.max(1));
            tokio::spawn(async move {
                let mut stream = pubsub.on_message();
                while let Some(message) = stream.next().await {
                    let payload = match message.get_payload::<String>() {
                        Ok(payload) => payload,
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "failed to decode Redis config reload payload"
                            );
                            continue;
                        }
                    };
                    let decoded = match ConfigReloadMessage::decode(&payload) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "failed to parse Redis config reload message"
                            );
                            continue;
                        }
                    };
                    let _ = sender.send(ConfigChangeEvent::Reload(decoded));
                }
            });

            Ok(Self { receiver })
        }

        /// Converts this listener into a notification receiver.
        pub fn into_notification(self) -> ConfigNotification {
            ConfigNotification::new(self.receiver)
        }
    }
}

#[cfg(feature = "redis-pubsub")]
pub use redis_transport::{RedisConfigChangeNotifier, RedisConfigReloadListener};

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "redis-pubsub"))]
    use super::CONFIG_SYNC_BACKEND_REDIS;
    use super::{
        CONFIG_SYNC_BACKEND_DISABLED, ConfigChangeEvent, ConfigChangeNotifier,
        ConfigNotificationSource, ConfigReloadDecision, ConfigReloadMessage,
        ConfigReloadWorkerConfig, ConfigSyncConfig, ConfigSyncRuntime, InMemoryConfigNotifier,
        SharedConfigChangeNotifier, build_config_sync_runtime, default_config_sync_topic,
        handle_config_reload_notification, redis_channel_from_topic, run_config_reload_worker,
    };
    use crate::ConfigCoreError;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;

    async fn wait_for_subscriber(notifier: &InMemoryConfigNotifier) {
        timeout(Duration::from_secs(1), async {
            while notifier.sender.receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn in_memory_notifier_broadcasts_reload_messages() {
        let notifier = InMemoryConfigNotifier::default();
        let mut subscription = notifier.subscribe().await.unwrap();

        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "runtime-a",
                ["b", "a", "a"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();

        let event = subscription.recv().await.unwrap();
        let ConfigChangeEvent::Reload(message) = event;
        assert_eq!(message.namespace, "aster_test");
        assert_eq!(message.origin_runtime_id, "runtime-a");
        assert_eq!(message.keys, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn reload_message_round_trips_json() {
        let message = ConfigReloadMessage::new(
            "aster_test",
            "runtime-a",
            ["feature_enabled"],
            ConfigNotificationSource::Cli,
        );

        let encoded = message.encode().unwrap();
        let decoded = ConfigReloadMessage::decode(&encoded).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn config_sync_config_defaults_to_disabled_generic_topic() {
        let config = ConfigSyncConfig::default();

        assert!(!config.enabled());
        assert_eq!(config.backend, CONFIG_SYNC_BACKEND_DISABLED);
        assert_eq!(config.endpoint, "");
        assert_eq!(config.topic, "aster.config_reload");
    }

    #[test]
    fn config_sync_runtime_holds_namespace_runtime_id_and_notifier() {
        let disabled = ConfigSyncRuntime::disabled_with_runtime_id("aster_test", "runtime-a");
        assert_eq!(disabled.namespace(), "aster_test");
        assert_eq!(disabled.runtime_id(), "runtime-a");
        assert!(!disabled.enabled());
        assert!(disabled.notifier().is_none());

        let notifier: SharedConfigChangeNotifier = Arc::new(InMemoryConfigNotifier::default());
        let enabled = ConfigSyncRuntime::new("aster_test", "runtime-b", notifier);
        assert_eq!(enabled.namespace(), "aster_test");
        assert_eq!(enabled.runtime_id(), "runtime-b");
        assert!(enabled.enabled());
        assert!(enabled.notifier().is_some());
        assert_eq!(
            enabled.worker_config(),
            ConfigReloadWorkerConfig::new("aster_test", "runtime-b")
        );
    }

    #[test]
    fn config_sync_runtime_builds_disabled_defaults() {
        let runtime =
            build_config_sync_runtime(&ConfigSyncConfig::default(), "aster_test").unwrap();

        assert!(!runtime.enabled());
        assert_eq!(runtime.namespace(), "aster_test");
        assert!(runtime.runtime_id().starts_with("runtime-"));
        assert_eq!(
            default_config_sync_topic("aster_test"),
            "aster_test.config_reload"
        );
    }

    #[test]
    fn config_sync_topic_maps_to_redis_channel_shape() {
        assert_eq!(
            redis_channel_from_topic(&default_config_sync_topic("aster_test")),
            "aster_test:config_reload"
        );
        assert_eq!(
            redis_channel_from_topic("custom.config.reload"),
            "custom:config:reload"
        );
    }

    #[cfg(not(feature = "redis-pubsub"))]
    #[test]
    fn redis_config_sync_backend_requires_feature() {
        let result = build_config_sync_runtime(
            &ConfigSyncConfig {
                backend: CONFIG_SYNC_BACKEND_REDIS.to_string(),
                endpoint: "redis://127.0.0.1:6379/0".to_string(),
                ..ConfigSyncConfig::default()
            },
            "aster_test",
        );
        let Err(error) = result else {
            panic!("redis config sync without redis-pubsub feature should fail");
        };

        assert!(
            error
                .to_string()
                .contains("requires the redis-pubsub feature")
        );
    }

    #[tokio::test]
    async fn config_sync_runtime_publishes_namespaced_reload_messages() {
        let notifier = Arc::new(InMemoryConfigNotifier::default());
        let mut subscription = notifier.subscribe().await.unwrap();
        let runtime = ConfigSyncRuntime::with_notifier_for_test(
            "aster_test",
            "runtime-a",
            notifier as SharedConfigChangeNotifier,
        );

        runtime
            .publish_reload(["b", "a", "a"], ConfigNotificationSource::Api)
            .await
            .unwrap();

        let message = subscription.recv().await.unwrap().reload_message().clone();
        assert_eq!(message.namespace, "aster_test");
        assert_eq!(message.origin_runtime_id, "runtime-a");
        assert_eq!(message.keys, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(message.source, ConfigNotificationSource::Api);
    }

    #[tokio::test]
    async fn config_sync_runtime_runs_reload_subscription_with_runtime_filter() {
        let notifier = Arc::new(InMemoryConfigNotifier::default());
        let runtime = ConfigSyncRuntime::with_notifier_for_test(
            "aster_test",
            "runtime-a",
            notifier.clone() as SharedConfigChangeNotifier,
        );
        let shutdown = CancellationToken::new();
        let observed = Arc::new(AtomicUsize::new(0));
        let observed_reload = observed.clone();
        let worker_shutdown = shutdown.clone();

        let worker = tokio::spawn(async move {
            runtime
                .run_reload_subscription(worker_shutdown, move |_| {
                    let observed_reload = observed_reload.clone();
                    async move {
                        observed_reload.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                })
                .await
                .unwrap();
        });
        wait_for_subscriber(&notifier).await;

        notifier
            .publish_reload(ConfigReloadMessage::new(
                "other",
                "runtime-b",
                ["ignored"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();
        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "runtime-a",
                ["ignored"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();
        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "runtime-b",
                ["accepted"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();

        timeout(Duration::from_secs(1), async {
            while observed.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        shutdown.cancel();
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn reload_handler_filters_namespace_and_origin() {
        let config = ConfigReloadWorkerConfig::new("aster_test", "runtime-a");
        let reloads = Arc::new(AtomicUsize::new(0));

        let decision = handle_config_reload_notification(
            &config,
            ConfigReloadMessage::new(
                "other",
                "runtime-b",
                ["feature"],
                ConfigNotificationSource::Api,
            ),
            |_| async {
                reloads.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(decision, ConfigReloadDecision::IgnoredNamespace);

        let decision = handle_config_reload_notification(
            &config,
            ConfigReloadMessage::new(
                "aster_test",
                "runtime-a",
                ["feature"],
                ConfigNotificationSource::Api,
            ),
            |_| async {
                reloads.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(decision, ConfigReloadDecision::IgnoredOrigin);

        let decision = handle_config_reload_notification(
            &config,
            ConfigReloadMessage::new(
                "aster_test",
                "runtime-b",
                ["feature"],
                ConfigNotificationSource::Api,
            ),
            |_| async {
                reloads.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(decision, ConfigReloadDecision::Reloaded);
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reload_worker_reloads_matching_remote_messages_until_cancelled() {
        let notifier = Arc::new(InMemoryConfigNotifier::default());
        let shutdown = CancellationToken::new();
        let reloads = Arc::new(AtomicUsize::new(0));
        let observed = reloads.clone();
        let worker_shutdown = shutdown.clone();
        let worker_notifier = notifier.clone();

        let worker = tokio::spawn(async move {
            run_config_reload_worker(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                worker_shutdown,
                move |_| {
                    let observed = observed.clone();
                    async move {
                        observed.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
            )
            .await
        });

        wait_for_subscriber(&notifier).await;

        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "node-a",
                ["local"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();
        notifier
            .publish_reload(ConfigReloadMessage::new(
                "other",
                "node-b",
                ["foreign"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();
        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "node-b",
                ["remote"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();

        timeout(Duration::from_secs(1), async {
            while reloads.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        shutdown.cancel();
        worker.await.unwrap().unwrap();
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reload_worker_keeps_listening_after_reload_error() {
        let notifier = Arc::new(InMemoryConfigNotifier::default());
        let shutdown = CancellationToken::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let worker_shutdown = shutdown.clone();
        let worker_notifier = notifier.clone();

        let worker = tokio::spawn(async move {
            run_config_reload_worker(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                worker_shutdown,
                move |_| {
                    let observed = observed.clone();
                    async move {
                        let attempt = observed.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err(ConfigCoreError::store("temporary reload failure"))
                        } else {
                            Ok(())
                        }
                    }
                },
            )
            .await
        });

        wait_for_subscriber(&notifier).await;

        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "node-b",
                ["first"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();
        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "node-c",
                ["second"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();

        timeout(Duration::from_secs(1), async {
            while attempts.load(Ordering::SeqCst) != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        shutdown.cancel();
        worker.await.unwrap().unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}

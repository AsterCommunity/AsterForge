//! Runtime configuration reload notifications.
//!
//! A running service can apply a local configuration change immediately, but
//! other processes need a lightweight signal telling them to reload from their
//! authoritative store. This module defines that signal and provides both an
//! in-memory notifier for tests/single-process deployments and an optional
//! Redis pub/sub transport for multi-process deployments.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{ConfigCoreError, Result};

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
    /// Node ID that emitted the message. Receivers can use this to ignore
    /// their own message after already applying the local change.
    pub origin_node_id: String,
    /// Changed keys. Empty means receivers should reload all runtime config.
    pub keys: Vec<String>,
    /// Source of the change.
    pub source: ConfigNotificationSource,
}

impl ConfigReloadMessage {
    /// Creates a reload message and sorts/deduplicates keys.
    pub fn new(
        namespace: impl Into<String>,
        origin_node_id: impl Into<String>,
        keys: impl IntoIterator<Item = impl Into<String>>,
        source: ConfigNotificationSource,
    ) -> Self {
        let mut keys = keys.into_iter().map(Into::into).collect::<Vec<_>>();
        keys.sort();
        keys.dedup();
        Self {
            namespace: namespace.into(),
            origin_node_id: origin_node_id.into(),
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

/// Transport used to publish and subscribe to reload notifications.
#[async_trait]
pub trait ConfigChangeNotifier: Send + Sync {
    /// Publishes a reload notification.
    async fn publish_reload(&self, message: ConfigReloadMessage) -> Result<()>;

    /// Subscribes to future reload notifications.
    async fn subscribe(&self) -> Result<ConfigNotification>;
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

#[cfg(feature = "redis")]
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

#[cfg(feature = "redis")]
pub use redis_transport::{RedisConfigChangeNotifier, RedisConfigReloadListener};

#[cfg(test)]
mod tests {
    use super::{
        ConfigChangeEvent, ConfigChangeNotifier, ConfigNotificationSource, ConfigReloadMessage,
        InMemoryConfigNotifier,
    };

    #[tokio::test]
    async fn in_memory_notifier_broadcasts_reload_messages() {
        let notifier = InMemoryConfigNotifier::default();
        let mut subscription = notifier.subscribe().await.unwrap();

        notifier
            .publish_reload(ConfigReloadMessage::new(
                "aster_test",
                "node-a",
                ["b", "a", "a"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();

        let event = subscription.recv().await.unwrap();
        let ConfigChangeEvent::Reload(message) = event;
        assert_eq!(message.namespace, "aster_test");
        assert_eq!(message.origin_node_id, "node-a");
        assert_eq!(message.keys, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn reload_message_round_trips_json() {
        let message = ConfigReloadMessage::new(
            "aster_test",
            "node-a",
            ["feature_enabled"],
            ConfigNotificationSource::Cli,
        );

        let encoded = message.encode().unwrap();
        let decoded = ConfigReloadMessage::decode(&encoded).unwrap();

        assert_eq!(decoded, message);
    }
}

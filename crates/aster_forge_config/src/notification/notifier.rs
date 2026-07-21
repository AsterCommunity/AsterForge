use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::{ConfigCoreError, Result};

#[cfg(feature = "redis-pubsub")]
use super::message::decode_config_reload_transport_payload;
use super::message::{ConfigChangeEvent, ConfigReloadMessage};

/// Subscription returned by config notifiers.
pub struct ConfigNotification {
    receiver: ConfigNotificationReceiver,
}

enum ConfigNotificationReceiver {
    InMemory(broadcast::Receiver<ConfigChangeEvent>),
    #[cfg(feature = "redis-pubsub")]
    Redis(aster_forge_events::RedisEventSubscription),
}

impl ConfigNotification {
    pub(super) fn new(receiver: broadcast::Receiver<ConfigChangeEvent>) -> Self {
        Self {
            receiver: ConfigNotificationReceiver::InMemory(receiver),
        }
    }

    #[cfg(feature = "redis-pubsub")]
    fn from_redis(subscription: aster_forge_events::RedisEventSubscription) -> Self {
        Self {
            receiver: ConfigNotificationReceiver::Redis(subscription),
        }
    }

    /// Waits for the next notification.
    pub async fn recv(&mut self) -> Result<ConfigChangeEvent> {
        match &mut self.receiver {
            ConfigNotificationReceiver::InMemory(receiver) => receiver
                .recv()
                .await
                .map_err(|error| ConfigCoreError::notification(error.to_string())),
            #[cfg(feature = "redis-pubsub")]
            ConfigNotificationReceiver::Redis(subscription) => loop {
                let payload = subscription
                    .receive()
                    .await
                    .map_err(|error| ConfigCoreError::notification(error.to_string()))?;
                match decode_config_reload_transport_payload(&payload) {
                    Ok(event) => return Ok(event),
                    Err(error) => {
                        tracing::warn!(%error, "failed to parse Redis config reload message");
                    }
                }
            },
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

/// In-memory notifier for single-process deployments and tests.
#[derive(Debug, Clone)]
pub struct InMemoryConfigNotifier {
    pub(super) sender: broadcast::Sender<ConfigChangeEvent>,
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
    use super::{ConfigChangeNotifier, ConfigNotification, ConfigReloadMessage};
    use crate::{ConfigCoreError, Result};

    /// Redis pub/sub publisher for configuration reload messages.
    #[derive(Clone)]
    pub struct RedisConfigChangeNotifier {
        bus: aster_forge_events::RedisEventBus,
    }

    impl RedisConfigChangeNotifier {
        /// Creates a Redis notifier for `channel`.
        pub fn new(client: redis::Client, channel: impl Into<String>) -> Self {
            Self {
                bus: aster_forge_events::RedisEventBus::from_client(client, channel),
            }
        }

        /// Creates a Redis notifier from a Redis connection URL.
        pub fn from_url(url: &str, channel: impl Into<String>) -> Result<Self> {
            let bus = aster_forge_events::RedisEventBus::from_url(url, channel)
                .map_err(|error| ConfigCoreError::notification(error.to_string()))?;
            Ok(Self { bus })
        }
    }

    #[async_trait::async_trait]
    impl ConfigChangeNotifier for RedisConfigChangeNotifier {
        async fn publish_reload(&self, message: ConfigReloadMessage) -> Result<()> {
            let payload = message.encode()?;
            self.bus
                .publish(payload)
                .await
                .map_err(|error| ConfigCoreError::notification(error.to_string()))
        }

        async fn subscribe(&self) -> Result<ConfigNotification> {
            let subscription = self
                .bus
                .subscribe()
                .await
                .map_err(|error| ConfigCoreError::notification(error.to_string()))?;
            Ok(ConfigNotification::from_redis(subscription))
        }
    }
}

#[cfg(feature = "redis-pubsub")]
pub use redis_transport::RedisConfigChangeNotifier;

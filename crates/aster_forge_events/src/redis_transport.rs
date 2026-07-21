use async_trait::async_trait;
use futures::StreamExt;
use redis::AsyncCommands;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    EventConnectionObservation, EventReconnectPolicy, EventSubscriptionSource,
    EventSubscriptionUpdate, supervise_event_subscription,
};

/// Redis event transport errors returned while creating a publisher.
#[derive(Debug, thiserror::Error)]
pub enum RedisEventBusError {
    /// The Redis client could not be created from the configured URL.
    #[error("open Redis event URL: {0}")]
    Open(String),
    /// A configured event topic is empty.
    #[error("event topic must not be empty")]
    EmptyTopic,
    /// Publishing failed.
    #[error("publish event payload: {0}")]
    Publish(String),
    /// Opening a subscription failed.
    #[error("subscribe to event topic: {0}")]
    Subscribe(String),
    /// An active subscription ended unexpectedly.
    #[error("event subscription stream ended")]
    StreamEnded,
}

/// Receives subscriber lifecycle observations.
pub trait EventConnectionObserver: Send + Sync {
    /// Records one connection transition.
    fn observe_event_connection(&self, observation: EventConnectionObservation);
}

impl<F> EventConnectionObserver for F
where
    F: Fn(EventConnectionObservation) + Send + Sync,
{
    fn observe_event_connection(&self, observation: EventConnectionObservation) {
        self(observation);
    }
}

/// Backwards-compatible name for the shared event reconnect policy.
pub type RedisEventReconnectPolicy = EventReconnectPolicy;

/// Redis-backed transient event publisher and reconnecting subscriber.
#[derive(Clone)]
pub struct RedisEventBus {
    client: redis::Client,
    topic: String,
    reconnect_policy: EventReconnectPolicy,
}

/// One active Redis Pub/Sub subscription.
pub struct RedisEventSubscription {
    pubsub: redis::aio::PubSub,
}

impl RedisEventBus {
    /// Creates a bus from a Redis URL and logical topic.
    pub fn from_url(url: &str, topic: impl Into<String>) -> Result<Self, RedisEventBusError> {
        let topic = topic.into();
        if topic.trim().is_empty() {
            return Err(RedisEventBusError::EmptyTopic);
        }
        let client = redis::Client::open(url)
            .map_err(|error| RedisEventBusError::Open(error.to_string()))?;
        Ok(Self {
            client,
            topic,
            reconnect_policy: EventReconnectPolicy::default(),
        })
    }

    /// Creates a bus from an existing Redis client.
    pub fn from_client(client: redis::Client, topic: impl Into<String>) -> Self {
        Self {
            client,
            topic: topic.into(),
            reconnect_policy: EventReconnectPolicy::default(),
        }
    }

    /// Overrides the reconnect policy.
    pub fn with_reconnect_policy(mut self, policy: RedisEventReconnectPolicy) -> Self {
        self.reconnect_policy = policy;
        self
    }

    /// Returns the configured logical topic.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Publishes one opaque payload. Payload interpretation belongs to the product layer.
    pub async fn publish(&self, payload: impl Into<String>) -> Result<(), RedisEventBusError> {
        let mut connection = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| RedisEventBusError::Publish(error.to_string()))?;
        let _: usize = connection
            .publish(&self.topic, payload.into())
            .await
            .map_err(|error| RedisEventBusError::Publish(error.to_string()))?;
        Ok(())
    }

    /// Opens one Redis subscription attempt.
    pub async fn subscribe(&self) -> Result<RedisEventSubscription, RedisEventBusError> {
        let mut pubsub = self
            .client
            .get_async_pubsub()
            .await
            .map_err(|error| RedisEventBusError::Subscribe(error.to_string()))?;
        pubsub
            .subscribe(&self.topic)
            .await
            .map_err(|error| RedisEventBusError::Subscribe(error.to_string()))?;
        Ok(RedisEventSubscription { pubsub })
    }

    /// Runs a reconnecting subscription until shutdown is cancelled.
    ///
    /// Malformed Redis payloads are logged and skipped. The callback is responsible for decoding
    /// the product payload and deciding whether an event belongs to the current runtime.
    pub async fn run_subscription<F, Fut>(
        &self,
        shutdown: CancellationToken,
        observer: Option<&dyn EventConnectionObserver>,
        mut on_payload: F,
    ) where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = ()>,
    {
        let (updates_tx, mut updates_rx) = mpsc::channel(1);
        let supervisor = supervise_event_subscription(
            Arc::new(self.clone()),
            self.reconnect_policy,
            shutdown.clone(),
            updates_tx,
        );
        tokio::pin!(supervisor);

        loop {
            let update = tokio::select! {
                () = shutdown.cancelled() => return,
                () = &mut supervisor => return,
                update = updates_rx.recv() => update,
            };
            match update {
                Some(EventSubscriptionUpdate::Connection(observation)) => {
                    if let Some(observer) = observer {
                        observer.observe_event_connection(observation);
                    }
                }
                Some(EventSubscriptionUpdate::Item(payload)) => on_payload(payload).await,
                None => return,
            }
        }
    }
}

impl RedisEventSubscription {
    /// Receives one raw payload. Redis payload conversion failures are logged and skipped.
    pub async fn receive(&mut self) -> Result<String, RedisEventBusError> {
        loop {
            let mut stream = self.pubsub.on_message();
            let Some(message) = stream.next().await else {
                return Err(RedisEventBusError::StreamEnded);
            };
            match message.get_payload::<String>() {
                Ok(payload) => return Ok(payload),
                Err(error) => tracing::warn!(%error, "failed to decode Redis event payload"),
            }
        }
    }
}

#[async_trait]
impl EventSubscriptionSource for RedisEventBus {
    type Item = String;
    type Subscription = RedisEventSubscription;
    type Error = RedisEventBusError;

    async fn subscribe(&self) -> Result<Self::Subscription, Self::Error> {
        RedisEventBus::subscribe(self).await
    }

    async fn receive(
        &self,
        subscription: &mut Self::Subscription,
    ) -> Result<Self::Item, Self::Error> {
        subscription.receive().await
    }
}

#[cfg(test)]
mod tests {
    use super::{RedisEventBus, RedisEventBusError};

    #[test]
    fn rejects_empty_topics() {
        assert!(matches!(
            RedisEventBus::from_url("redis://127.0.0.1", "  "),
            Err(RedisEventBusError::EmptyTopic)
        ));
    }
}

use std::fmt::Display;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Connection lifecycle state emitted by a reconnecting event subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventConnectionState {
    /// A first subscription became ready.
    Connected,
    /// An established subscription stopped or could not be opened.
    Disconnected,
    /// The subscriber is waiting before another connection attempt.
    Reconnecting,
    /// A subscription recovered after a previous disconnect.
    Recovered,
}

impl EventConnectionState {
    /// Returns the stable low-cardinality label for this state.
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Reconnecting => "reconnecting",
            Self::Recovered => "recovered",
        }
    }
}

/// Connection lifecycle observation emitted by a subscription supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventConnectionObservation {
    /// Current connection state.
    pub state: EventConnectionState,
    /// One-based reconnect attempt number, or zero for the first connection.
    pub reconnect_attempt: u32,
    /// Backoff selected for the next reconnect attempt.
    pub backoff: Duration,
}

/// Reconnect policy for a transient event subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventReconnectPolicy {
    /// Initial reconnect delay.
    pub initial_delay: Duration,
    /// Maximum reconnect delay.
    pub max_delay: Duration,
    /// Stable connection duration after which the attempt counter resets.
    pub stable_reset_after: Duration,
    /// Minimum jitter percentage applied to the selected delay.
    pub jitter_min_percent: u16,
    /// Maximum jitter percentage applied to the selected delay.
    pub jitter_max_percent: u16,
}

impl Default for EventReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(30),
            stable_reset_after: Duration::from_secs(30),
            jitter_min_percent: 80,
            jitter_max_percent: 120,
        }
    }
}

impl EventReconnectPolicy {
    /// Calculates the bounded jittered delay for a one-based reconnect attempt.
    pub fn reconnect_delay(self, reconnect_attempt: u32) -> Duration {
        let raw = aster_forge_utils::backoff::exponential_delay(
            self.initial_delay,
            reconnect_attempt.saturating_sub(1),
        );
        let capped = aster_forge_utils::backoff::cap_delay(raw, self.max_delay);
        let jittered = aster_forge_utils::backoff::randomized_jitter(
            capped,
            self.jitter_min_percent,
            self.jitter_max_percent,
        );
        aster_forge_utils::backoff::cap_delay(jittered, self.max_delay)
    }
}

/// One update emitted by a reconnecting subscription supervisor.
#[derive(Debug)]
pub enum EventSubscriptionUpdate<T> {
    /// The transport connection changed state.
    Connection(EventConnectionObservation),
    /// The transport delivered one product-owned item.
    Item(T),
}

/// A transport-specific source that can open and receive from one subscription.
///
/// The shared supervisor owns retries and lifecycle observations. Implementations own only one
/// connection attempt and one-item receive semantics.
#[async_trait]
pub trait EventSubscriptionSource: Send + Sync {
    /// Item emitted by the transport.
    type Item: Send;
    /// One active transport subscription.
    type Subscription: Send;
    /// Transport error returned by subscribe or receive.
    type Error: Display + Send + Sync;

    /// Opens one subscription attempt.
    async fn subscribe(&self) -> Result<Self::Subscription, Self::Error>;

    /// Receives one item from an active subscription.
    async fn receive(
        &self,
        subscription: &mut Self::Subscription,
    ) -> Result<Self::Item, Self::Error>;
}

/// Supervises one transient subscription until shutdown or receiver closure.
///
/// Consumers handle updates sequentially. `Connected`/`Recovered` is therefore observed before
/// the first item from that subscription, so products can reconcile authoritative state first.
pub async fn supervise_event_subscription<S>(
    source: Arc<S>,
    reconnect_policy: EventReconnectPolicy,
    shutdown: CancellationToken,
    updates: mpsc::Sender<EventSubscriptionUpdate<S::Item>>,
) where
    S: EventSubscriptionSource + ?Sized,
{
    let mut reconnect_attempt = 0_u32;

    loop {
        let subscription = tokio::select! {
            () = shutdown.cancelled() => return,
            result = source.subscribe() => result,
        };
        let mut subscription = match subscription {
            Ok(subscription) => subscription,
            Err(error) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                let delay = reconnect_policy.reconnect_delay(reconnect_attempt);
                if !send_connection_update(
                    &updates,
                    &shutdown,
                    EventConnectionState::Disconnected,
                    reconnect_attempt,
                    Duration::ZERO,
                )
                .await
                    || !send_connection_update(
                        &updates,
                        &shutdown,
                        EventConnectionState::Reconnecting,
                        reconnect_attempt,
                        delay,
                    )
                    .await
                {
                    return;
                }
                tracing::warn!(
                    reconnect_attempt,
                    backoff_ms = delay.as_millis(),
                    error = %error,
                    "event subscription attempt failed"
                );
                if sleep_or_shutdown(&shutdown, delay).await {
                    return;
                }
                continue;
            }
        };

        let connected_state = if reconnect_attempt == 0 {
            EventConnectionState::Connected
        } else {
            EventConnectionState::Recovered
        };
        if !send_connection_update(
            &updates,
            &shutdown,
            connected_state,
            reconnect_attempt,
            Duration::ZERO,
        )
        .await
        {
            return;
        }

        let connected_at = Instant::now();
        loop {
            let item = tokio::select! {
                () = shutdown.cancelled() => return,
                result = source.receive(&mut subscription) => result,
            };
            match item {
                Ok(item) => {
                    reconnect_attempt = 0;
                    if !send_update(&updates, &shutdown, EventSubscriptionUpdate::Item(item)).await
                    {
                        return;
                    }
                }
                Err(error) => {
                    if connected_at.elapsed() >= reconnect_policy.stable_reset_after {
                        reconnect_attempt = 0;
                    }
                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                    let delay = reconnect_policy.reconnect_delay(reconnect_attempt);
                    if !send_connection_update(
                        &updates,
                        &shutdown,
                        EventConnectionState::Disconnected,
                        reconnect_attempt,
                        Duration::ZERO,
                    )
                    .await
                        || !send_connection_update(
                            &updates,
                            &shutdown,
                            EventConnectionState::Reconnecting,
                            reconnect_attempt,
                            delay,
                        )
                        .await
                    {
                        return;
                    }
                    tracing::warn!(
                        reconnect_attempt,
                        backoff_ms = delay.as_millis(),
                        error = %error,
                        "event subscription disconnected"
                    );
                    if sleep_or_shutdown(&shutdown, delay).await {
                        return;
                    }
                    break;
                }
            }
        }
    }
}

async fn send_connection_update<T>(
    updates: &mpsc::Sender<EventSubscriptionUpdate<T>>,
    shutdown: &CancellationToken,
    state: EventConnectionState,
    reconnect_attempt: u32,
    backoff: Duration,
) -> bool {
    send_update(
        updates,
        shutdown,
        EventSubscriptionUpdate::Connection(EventConnectionObservation {
            state,
            reconnect_attempt,
            backoff,
        }),
    )
    .await
}

async fn send_update<T>(
    updates: &mpsc::Sender<EventSubscriptionUpdate<T>>,
    shutdown: &CancellationToken,
    update: EventSubscriptionUpdate<T>,
) -> bool {
    tokio::select! {
        () = shutdown.cancelled() => false,
        result = updates.send(update) => result.is_ok(),
    }
}

async fn sleep_or_shutdown(shutdown: &CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        () = shutdown.cancelled() => true,
        () = tokio::time::sleep(delay) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::EventReconnectPolicy;
    use std::time::Duration;

    #[test]
    fn reconnect_delay_doubles_and_caps_with_fixed_jitter() {
        let policy = EventReconnectPolicy {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(250),
            stable_reset_after: Duration::from_secs(1),
            jitter_min_percent: 100,
            jitter_max_percent: 100,
        };

        assert_eq!(policy.reconnect_delay(1), Duration::from_millis(100));
        assert_eq!(policy.reconnect_delay(2), Duration::from_millis(200));
        assert_eq!(policy.reconnect_delay(3), Duration::from_millis(250));
    }
}

use async_trait::async_trait;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{ConfigCoreError, Result};

use super::message::{
    ConfigChangeEvent, ConfigReloadDecision, ConfigReloadMessage, ConfigReloadWorkerConfig,
    handle_config_reload_notification,
};
use super::notifier::{ConfigChangeNotifier, ConfigNotification};

/// Observability event emitted after handling a config reload notification.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigReloadObservation {
    /// Source label suitable for low-cardinality metrics.
    pub source: &'static str,
    /// Handling decision.
    pub decision: ConfigReloadDecision,
    /// Whether the handling path succeeded.
    pub status: &'static str,
    /// Number of changed keys advertised by the reload hint.
    pub changed_keys: u64,
    /// Time spent handling this notification.
    pub duration_seconds: f64,
}

impl ConfigReloadObservation {
    fn new(
        source: &'static str,
        decision: ConfigReloadDecision,
        status: &'static str,
        changed_keys: usize,
        duration_seconds: f64,
    ) -> Self {
        Self {
            source,
            decision,
            status,
            changed_keys: u64::try_from(changed_keys).unwrap_or(u64::MAX),
            duration_seconds,
        }
    }
}

/// Connection lifecycle state for a config-sync subscription.
pub type ConfigSyncConnectionState = aster_forge_events::EventConnectionState;

/// Low-cardinality observation emitted for config-sync connection transitions.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigSyncConnectionObservation {
    /// Connection lifecycle state.
    pub state: ConfigSyncConnectionState,
    /// One-based reconnect attempt number, or zero outside reconnect attempts.
    pub reconnect_attempt: u32,
    /// Planned backoff for a reconnect attempt, or zero for other states.
    pub backoff_seconds: f64,
}

impl ConfigSyncConnectionObservation {
    fn new(state: ConfigSyncConnectionState, reconnect_attempt: u32, backoff: Duration) -> Self {
        Self {
            state,
            reconnect_attempt,
            backoff_seconds: backoff.as_secs_f64(),
        }
    }
}

/// Receives config reload observability events.
pub trait ConfigReloadObserver: Send + Sync {
    /// Records one reload observation.
    fn observe_config_reload(&self, observation: ConfigReloadObservation);
}

impl<F> ConfigReloadObserver for F
where
    F: Fn(ConfigReloadObservation) + Send + Sync,
{
    fn observe_config_reload(&self, observation: ConfigReloadObservation) {
        self(observation);
    }
}

/// Receives config-sync connection lifecycle observations.
pub trait ConfigSyncConnectionObserver: Send + Sync {
    /// Records one connection transition.
    fn observe_config_sync_connection(&self, observation: ConfigSyncConnectionObservation);
}

impl<F> ConfigSyncConnectionObserver for F
where
    F: Fn(ConfigSyncConnectionObservation) + Send + Sync,
{
    fn observe_config_sync_connection(&self, observation: ConfigSyncConnectionObservation) {
        self(observation);
    }
}

pub(super) type ConfigReloadReconnectPolicy = aster_forge_events::EventReconnectPolicy;

fn default_config_reload_reconnect_policy() -> ConfigReloadReconnectPolicy {
    ConfigReloadReconnectPolicy {
        initial_delay: Duration::from_millis(250),
        max_delay: Duration::from_secs(30),
        stable_reset_after: Duration::from_secs(30),
        jitter_min_percent: 50,
        jitter_max_percent: 100,
    }
}

/// Runs a reload subscription loop until `shutdown` is cancelled.
///
/// The loop never carries configuration values over pub/sub. A matching
/// notification only tells this process to reload from its authoritative store.
/// Reload errors are logged and the loop keeps listening, because one failed DB
/// read should not permanently break cross-process synchronization.
///
/// The loop is supervised like [`run_config_reload_supervisor`] with a no-op
/// reconcile: subscription failures, stream endings, and broadcast lag trigger
/// a bounded reconnect instead of exiting.
pub async fn run_config_reload_worker<N, F, Fut>(
    notifier: Arc<N>,
    config: ConfigReloadWorkerConfig,
    shutdown: CancellationToken,
    reload: F,
) -> Result<()>
where
    N: ConfigChangeNotifier + ?Sized,
    F: FnMut(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    run_config_reload_worker_with_observer(
        notifier,
        config,
        shutdown,
        reload,
        None::<&dyn ConfigReloadObserver>,
    )
    .await
}

/// Runs a reload subscription loop and reports low-cardinality observations.
pub async fn run_config_reload_worker_with_observer<N, F, Fut>(
    notifier: Arc<N>,
    config: ConfigReloadWorkerConfig,
    shutdown: CancellationToken,
    mut reload: F,
    observer: Option<&dyn ConfigReloadObserver>,
) -> Result<()>
where
    N: ConfigChangeNotifier + ?Sized,
    F: FnMut(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut reconcile = || async { Ok(()) };
    run_config_reload_supervisor_inner(
        notifier,
        config,
        default_config_reload_reconnect_policy(),
        shutdown,
        &mut reconcile,
        &mut reload,
        observer,
        None,
    )
    .await
}

/// Runs a reconnecting reload subscription with authoritative reconciliation.
///
/// `reconcile` runs after every successful subscription, including the initial
/// connection. This closes the race between the product's startup snapshot load
/// and the moment pub/sub begins receiving notifications. After a disconnect it
/// also repairs any changes missed while the transient transport was unavailable.
///
/// A disconnect is any of: subscribe failure, transport stream error or ending,
/// and local broadcast lag (the receiver fell behind and events were dropped).
/// Each one is observed, waited out with bounded exponential backoff (250 ms
/// initial, 30 s cap, jittered; the failure counter resets after a subscription
/// stays stable for 30 s), then followed by a fresh subscription and reconcile.
/// The loop only returns when `shutdown` is cancelled.
pub async fn run_config_reload_supervisor<N, R, RFut, F, Fut>(
    notifier: Arc<N>,
    config: ConfigReloadWorkerConfig,
    shutdown: CancellationToken,
    reconcile: R,
    reload: F,
) -> Result<()>
where
    N: ConfigChangeNotifier + ?Sized,
    R: FnMut() -> RFut,
    RFut: Future<Output = Result<()>>,
    F: FnMut(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    run_config_reload_supervisor_with_observers(
        notifier, config, shutdown, reconcile, reload, None, None,
    )
    .await
}

/// Runs a reconnecting reload subscription and reports reload and connection observations.
pub async fn run_config_reload_supervisor_with_observers<N, R, RFut, F, Fut>(
    notifier: Arc<N>,
    config: ConfigReloadWorkerConfig,
    shutdown: CancellationToken,
    mut reconcile: R,
    mut reload: F,
    reload_observer: Option<&dyn ConfigReloadObserver>,
    connection_observer: Option<&dyn ConfigSyncConnectionObserver>,
) -> Result<()>
where
    N: ConfigChangeNotifier + ?Sized,
    R: FnMut() -> RFut,
    RFut: Future<Output = Result<()>>,
    F: FnMut(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    run_config_reload_supervisor_inner(
        notifier,
        config,
        default_config_reload_reconnect_policy(),
        shutdown,
        &mut reconcile,
        &mut reload,
        reload_observer,
        connection_observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_config_reload_supervisor_inner<N, R, RFut, F, Fut>(
    notifier: Arc<N>,
    config: ConfigReloadWorkerConfig,
    reconnect_policy: ConfigReloadReconnectPolicy,
    shutdown: CancellationToken,
    reconcile: &mut R,
    reload: &mut F,
    reload_observer: Option<&dyn ConfigReloadObserver>,
    connection_observer: Option<&dyn ConfigSyncConnectionObserver>,
) -> Result<()>
where
    N: ConfigChangeNotifier + ?Sized,
    R: FnMut() -> RFut,
    RFut: Future<Output = Result<()>>,
    F: FnMut(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let source = Arc::new(ConfigNotifierSubscriptionSource { notifier });
    let (updates_tx, mut updates_rx) = mpsc::channel(1);
    let supervisor = aster_forge_events::supervise_event_subscription(
        source,
        reconnect_policy,
        shutdown.clone(),
        updates_tx,
    );
    tokio::pin!(supervisor);

    loop {
        let update = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = &mut supervisor => return Ok(()),
            update = updates_rx.recv() => update,
        };
        match update {
            Some(aster_forge_events::EventSubscriptionUpdate::Connection(observation)) => {
                observe_config_sync_connection(
                    connection_observer,
                    observation.state,
                    observation.reconnect_attempt,
                    observation.backoff,
                );
                match observation.state {
                    ConfigSyncConnectionState::Connected | ConfigSyncConnectionState::Recovered => {
                        if observation.state == ConfigSyncConnectionState::Recovered {
                            tracing::info!(
                                reconnect_attempt = observation.reconnect_attempt,
                                "config reload subscription recovered"
                            );
                        }
                        if let Err(error) = reconcile().await {
                            tracing::warn!(
                                error = %error,
                                "failed to reconcile runtime config after subscription connected"
                            );
                        } else {
                            tracing::debug!(
                                "runtime config reconciled after subscription connected"
                            );
                        }
                    }
                    ConfigSyncConnectionState::Disconnected => {
                        tracing::warn!(
                            reconnect_attempt = observation.reconnect_attempt,
                            "config reload subscription disconnected"
                        );
                    }
                    ConfigSyncConnectionState::Reconnecting => {
                        tracing::warn!(
                            reconnect_attempt = observation.reconnect_attempt,
                            backoff_ms = duration_millis_u64(observation.backoff),
                            "waiting before config reload subscription reconnect"
                        );
                    }
                }
            }
            Some(aster_forge_events::EventSubscriptionUpdate::Item(ConfigChangeEvent::Reload(
                message,
            ))) => {
                process_config_reload_message(&config, message, reload, reload_observer).await;
            }
            None => return Ok(()),
        }
    }
}

async fn process_config_reload_message<F, Fut>(
    config: &ConfigReloadWorkerConfig,
    message: ConfigReloadMessage,
    reload: &mut F,
    observer: Option<&dyn ConfigReloadObserver>,
) where
    F: FnMut(ConfigReloadMessage) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let changed_keys = message.keys.len();
    let started = Instant::now();
    match handle_config_reload_notification(config, message, reload).await {
        Ok(ConfigReloadDecision::Reloaded) => {
            observe_config_reload(
                observer,
                ConfigReloadDecision::Reloaded,
                "ok",
                changed_keys,
                started,
            );
            tracing::debug!("runtime config reloaded after remote notification");
        }
        Ok(
            decision @ (ConfigReloadDecision::IgnoredNamespace
            | ConfigReloadDecision::IgnoredOrigin),
        ) => {
            observe_config_reload(observer, decision, "ok", changed_keys, started);
        }
        Err(error) => {
            observe_config_reload(
                observer,
                ConfigReloadDecision::Reloaded,
                "error",
                changed_keys,
                started,
            );
            tracing::warn!(
                error = %error,
                "failed to reload runtime config after remote notification"
            );
        }
    }
}

#[cfg(test)]
pub(super) fn config_reload_reconnect_delay(
    policy: ConfigReloadReconnectPolicy,
    reconnect_attempt: u32,
) -> Duration {
    policy.reconnect_delay(reconnect_attempt)
}

pub(super) fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn observe_config_sync_connection(
    observer: Option<&dyn ConfigSyncConnectionObserver>,
    state: ConfigSyncConnectionState,
    reconnect_attempt: u32,
    backoff: Duration,
) {
    if let Some(observer) = observer {
        observer.observe_config_sync_connection(ConfigSyncConnectionObservation::new(
            state,
            reconnect_attempt,
            backoff,
        ));
    }
}

fn observe_config_reload(
    observer: Option<&dyn ConfigReloadObserver>,
    decision: ConfigReloadDecision,
    status: &'static str,
    changed_keys: usize,
    started: Instant,
) {
    if let Some(observer) = observer {
        observer.observe_config_reload(ConfigReloadObservation::new(
            "pubsub",
            decision,
            status,
            changed_keys,
            started.elapsed().as_secs_f64(),
        ));
    }
}
struct ConfigNotifierSubscriptionSource<N: ?Sized> {
    notifier: Arc<N>,
}

#[async_trait]
impl<N> aster_forge_events::EventSubscriptionSource for ConfigNotifierSubscriptionSource<N>
where
    N: ConfigChangeNotifier + ?Sized,
{
    type Item = ConfigChangeEvent;
    type Subscription = ConfigNotification;
    type Error = ConfigCoreError;

    async fn subscribe(&self) -> Result<Self::Subscription> {
        self.notifier.subscribe().await
    }

    async fn receive(&self, subscription: &mut Self::Subscription) -> Result<Self::Item> {
        subscription.recv().await
    }
}

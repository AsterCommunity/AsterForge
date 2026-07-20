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

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::time::sleep;
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
    _task: Option<ConfigNotificationTask>,
}

impl ConfigNotification {
    fn new(receiver: broadcast::Receiver<ConfigChangeEvent>) -> Self {
        Self {
            receiver,
            _task: None,
        }
    }

    #[cfg(feature = "redis-pubsub")]
    fn with_task(
        receiver: broadcast::Receiver<ConfigChangeEvent>,
        task: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            receiver,
            _task: Some(ConfigNotificationTask { task }),
        }
    }

    /// Waits for the next notification.
    pub async fn recv(&mut self) -> Result<ConfigChangeEvent> {
        self.receiver
            .recv()
            .await
            .map_err(|error| ConfigCoreError::notification(error.to_string()))
    }
}

struct ConfigNotificationTask {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ConfigNotificationTask {
    fn drop(&mut self) {
        self.task.abort();
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

impl ConfigReloadDecision {
    /// Returns the stable metrics label for this decision.
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Reloaded => "reloaded",
            Self::IgnoredNamespace => "ignored_namespace",
            Self::IgnoredOrigin => "ignored_origin",
        }
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSyncConnectionState {
    /// The initial subscription connected successfully.
    Connected,
    /// An initial connection attempt failed or an active subscription ended.
    Disconnected,
    /// The supervisor is waiting before another subscription attempt.
    Reconnecting,
    /// A subscription was re-established after at least one failure.
    Recovered,
}

impl ConfigSyncConnectionState {
    /// Returns the stable metrics label for this state.
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Reconnecting => "reconnecting",
            Self::Recovered => "recovered",
        }
    }
}

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

#[derive(Debug, Clone, Copy)]
struct ConfigReloadReconnectPolicy {
    initial_delay: Duration,
    max_delay: Duration,
    stable_reset_after: Duration,
}

impl Default for ConfigReloadReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(30),
            stable_reset_after: Duration::from_secs(30),
        }
    }
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

/// Decodes one transport payload into a config reload event.
///
/// Transport adapters should use this helper before forwarding data into the common notifier path.
/// Malformed payloads are returned as errors so listeners can log and continue instead of ending the
/// subscription loop.
pub fn decode_config_reload_transport_payload(payload: &str) -> Result<ConfigChangeEvent> {
    ConfigReloadMessage::decode(payload).map(ConfigChangeEvent::Reload)
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
        ConfigReloadReconnectPolicy::default(),
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
        ConfigReloadReconnectPolicy::default(),
        shutdown,
        &mut reconcile,
        &mut reload,
        reload_observer,
        connection_observer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_config_reload_supervisor_inner<N, R, RFut, F, Fut>(
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
    let mut consecutive_failures = 0_u32;

    loop {
        let subscription = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            subscription = notifier.subscribe() => subscription,
        };
        let mut subscription = match subscription {
            Ok(subscription) => subscription,
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                observe_config_sync_connection(
                    connection_observer,
                    ConfigSyncConnectionState::Disconnected,
                    consecutive_failures,
                    Duration::ZERO,
                );
                tracing::warn!(
                    error = %error,
                    reconnect_attempt = consecutive_failures,
                    "failed to subscribe to config reload notifications"
                );
                if wait_for_config_reload_reconnect(
                    reconnect_policy,
                    consecutive_failures,
                    &shutdown,
                    connection_observer,
                )
                .await
                {
                    return Ok(());
                }
                continue;
            }
        };

        if consecutive_failures == 0 {
            observe_config_sync_connection(
                connection_observer,
                ConfigSyncConnectionState::Connected,
                0,
                Duration::ZERO,
            );
        } else {
            observe_config_sync_connection(
                connection_observer,
                ConfigSyncConnectionState::Recovered,
                consecutive_failures,
                Duration::ZERO,
            );
            tracing::info!(
                reconnect_attempt = consecutive_failures,
                "config reload subscription recovered"
            );
        }

        if let Err(error) = reconcile().await {
            tracing::warn!(
                error = %error,
                "failed to reconcile runtime config after subscription connected"
            );
        } else {
            tracing::debug!("runtime config reconciled after subscription connected");
        }

        let subscribed_at = Instant::now();
        loop {
            let event = tokio::select! {
                () = shutdown.cancelled() => return Ok(()),
                event = subscription.recv() => event,
            };
            match event {
                Ok(ConfigChangeEvent::Reload(message)) => {
                    consecutive_failures = 0;
                    process_config_reload_message(&config, message, reload, reload_observer).await;
                }
                Err(error) => {
                    if subscribed_at.elapsed() >= reconnect_policy.stable_reset_after {
                        consecutive_failures = 0;
                    }
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    observe_config_sync_connection(
                        connection_observer,
                        ConfigSyncConnectionState::Disconnected,
                        consecutive_failures,
                        Duration::ZERO,
                    );
                    tracing::warn!(
                        error = %error,
                        reconnect_attempt = consecutive_failures,
                        "config reload subscription disconnected"
                    );
                    if wait_for_config_reload_reconnect(
                        reconnect_policy,
                        consecutive_failures,
                        &shutdown,
                        connection_observer,
                    )
                    .await
                    {
                        return Ok(());
                    }
                    break;
                }
            }
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

async fn wait_for_config_reload_reconnect(
    policy: ConfigReloadReconnectPolicy,
    reconnect_attempt: u32,
    shutdown: &CancellationToken,
    observer: Option<&dyn ConfigSyncConnectionObserver>,
) -> bool {
    let delay = config_reload_reconnect_delay(policy, reconnect_attempt);
    observe_config_sync_connection(
        observer,
        ConfigSyncConnectionState::Reconnecting,
        reconnect_attempt,
        delay,
    );
    tracing::warn!(
        reconnect_attempt,
        backoff_ms = duration_millis_u64(delay),
        "waiting before config reload subscription reconnect"
    );
    tokio::select! {
        () = shutdown.cancelled() => true,
        () = sleep(delay) => false,
    }
}

fn config_reload_reconnect_delay(
    policy: ConfigReloadReconnectPolicy,
    reconnect_attempt: u32,
) -> Duration {
    use aster_forge_utils::backoff::{cap_delay, exponential_delay, randomized_jitter};

    let retry_index = reconnect_attempt.saturating_sub(1);
    let capped = cap_delay(
        exponential_delay(policy.initial_delay, retry_index),
        policy.max_delay,
    );
    cap_delay(randomized_jitter(capped, 50, 100), policy.max_delay)
}

fn duration_millis_u64(duration: Duration) -> u64 {
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

    /// Runs this runtime's reload subscription worker and reports observations.
    pub async fn run_reload_subscription_with_observer<F, Fut>(
        &self,
        shutdown: CancellationToken,
        reload: F,
        observer: Option<&dyn ConfigReloadObserver>,
    ) -> Result<()>
    where
        F: FnMut(ConfigReloadMessage) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let Some(notifier) = self.notifier().cloned() else {
            shutdown.cancelled().await;
            return Ok(());
        };
        run_config_reload_worker_with_observer(
            notifier,
            self.worker_config(),
            shutdown,
            reload,
            observer,
        )
        .await
    }

    /// Runs a reconnecting subscription with an authoritative reconcile callback.
    ///
    /// `reconcile` runs after each successful subscription. Product code should
    /// reload its full snapshot and invalidate all derived configuration caches.
    pub async fn run_reload_subscription_with_reconcile<R, RFut, F, Fut>(
        &self,
        shutdown: CancellationToken,
        reconcile: R,
        reload: F,
    ) -> Result<()>
    where
        R: FnMut() -> RFut,
        RFut: Future<Output = Result<()>>,
        F: FnMut(ConfigReloadMessage) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let Some(notifier) = self.notifier().cloned() else {
            shutdown.cancelled().await;
            return Ok(());
        };
        run_config_reload_supervisor(notifier, self.worker_config(), shutdown, reconcile, reload)
            .await
    }

    /// Runs a reconnecting subscription and reports reload and connection observations.
    pub async fn run_reload_subscription_with_reconcile_and_observers<R, RFut, F, Fut>(
        &self,
        shutdown: CancellationToken,
        reconcile: R,
        reload: F,
        reload_observer: Option<&dyn ConfigReloadObserver>,
        connection_observer: Option<&dyn ConfigSyncConnectionObserver>,
    ) -> Result<()>
    where
        R: FnMut() -> RFut,
        RFut: Future<Output = Result<()>>,
        F: FnMut(ConfigReloadMessage) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let Some(notifier) = self.notifier().cloned() else {
            shutdown.cancelled().await;
            return Ok(());
        };
        run_config_reload_supervisor_with_observers(
            notifier,
            self.worker_config(),
            shutdown,
            reconcile,
            reload,
            reload_observer,
            connection_observer,
        )
        .await
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
    use tokio::task::JoinHandle;

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
        receiver: Option<broadcast::Receiver<ConfigChangeEvent>>,
        task: Option<JoinHandle<()>>,
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
            let task = tokio::spawn(async move {
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
                    let decoded = match super::decode_config_reload_transport_payload(&payload) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "failed to parse Redis config reload message"
                            );
                            continue;
                        }
                    };
                    if sender.send(decoded).is_err() {
                        tracing::debug!(
                            channel = %channel,
                            "Redis config reload listener stopped after downstream receiver closed"
                        );
                        return;
                    }
                }
                tracing::warn!(
                    channel = %channel,
                    "Redis config reload listener stream ended unexpectedly"
                );
            });

            Ok(Self {
                receiver: Some(receiver),
                task: Some(task),
            })
        }

        /// Converts this listener into a notification receiver.
        pub fn into_notification(mut self) -> ConfigNotification {
            let receiver = match self.receiver.take() {
                Some(receiver) => receiver,
                None => {
                    let (sender, receiver) = broadcast::channel(1);
                    drop(sender);
                    receiver
                }
            };
            let Some(task) = self.task.take() else {
                return ConfigNotification::new(receiver);
            };
            ConfigNotification::with_task(receiver, task)
        }
    }

    impl Drop for RedisConfigReloadListener {
        fn drop(&mut self) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
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
        ConfigReloadReconnectPolicy, ConfigReloadWorkerConfig, ConfigSyncConfig,
        ConfigSyncConnectionObservation, ConfigSyncConnectionState, ConfigSyncRuntime,
        InMemoryConfigNotifier, SharedConfigChangeNotifier, build_config_sync_runtime,
        build_config_sync_runtime_with_runtime_id, config_reload_reconnect_delay,
        decode_config_reload_transport_payload, default_config_sync_topic,
        handle_config_reload_notification, redis_channel_from_topic,
        run_config_reload_supervisor_inner, run_config_reload_worker,
        run_config_reload_worker_with_observer,
    };
    use crate::ConfigCoreError;
    use std::collections::VecDeque;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::broadcast;
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

    enum SubscribeStep {
        Fail(&'static str),
        Channel(broadcast::Sender<ConfigChangeEvent>),
        Pending,
    }

    struct ScriptedConfigNotifier {
        steps: Mutex<VecDeque<SubscribeStep>>,
        subscribe_attempts: AtomicUsize,
    }

    impl ScriptedConfigNotifier {
        fn new(steps: impl IntoIterator<Item = SubscribeStep>) -> Self {
            Self {
                steps: Mutex::new(steps.into_iter().collect()),
                subscribe_attempts: AtomicUsize::new(0),
            }
        }

        fn subscribe_attempts(&self) -> usize {
            self.subscribe_attempts.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ConfigChangeNotifier for ScriptedConfigNotifier {
        async fn publish_reload(&self, _message: ConfigReloadMessage) -> super::Result<()> {
            Ok(())
        }

        async fn subscribe(&self) -> super::Result<super::ConfigNotification> {
            self.subscribe_attempts.fetch_add(1, Ordering::SeqCst);
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(SubscribeStep::Pending);
            match step {
                SubscribeStep::Fail(message) => Err(ConfigCoreError::notification(message)),
                SubscribeStep::Channel(sender) => {
                    Ok(super::ConfigNotification::new(sender.subscribe()))
                }
                SubscribeStep::Pending => std::future::pending().await,
            }
        }
    }

    #[derive(Default)]
    struct TestConnectionObserver {
        observations: Mutex<Vec<ConfigSyncConnectionObservation>>,
    }

    impl super::ConfigSyncConnectionObserver for TestConnectionObserver {
        fn observe_config_sync_connection(&self, observation: ConfigSyncConnectionObservation) {
            self.observations.lock().unwrap().push(observation);
        }
    }

    impl TestConnectionObserver {
        fn snapshot(&self) -> Vec<ConfigSyncConnectionObservation> {
            self.observations.lock().unwrap().clone()
        }
    }

    fn zero_reconnect_policy() -> ConfigReloadReconnectPolicy {
        ConfigReloadReconnectPolicy {
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            stable_reset_after: Duration::from_secs(30),
        }
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
    fn transport_payload_decode_surfaces_malformed_messages() {
        let message = ConfigReloadMessage::new(
            "aster_test",
            "runtime-a",
            ["feature_enabled"],
            ConfigNotificationSource::Cli,
        );
        let encoded = message.encode().unwrap();
        let event = decode_config_reload_transport_payload(&encoded).unwrap();
        assert_eq!(event.reload_message(), &message);

        assert!(decode_config_reload_transport_payload("{not-json").is_err());
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
    fn config_sync_runtime_can_use_explicit_runtime_id() {
        let runtime = build_config_sync_runtime_with_runtime_id(
            &ConfigSyncConfig::default(),
            "aster_test",
            "runtime-explicit",
        )
        .unwrap();

        assert!(!runtime.enabled());
        assert_eq!(runtime.namespace(), "aster_test");
        assert_eq!(runtime.runtime_id(), "runtime-explicit");
    }

    #[tokio::test]
    async fn disabled_config_sync_publish_is_noop() {
        let runtime = ConfigSyncRuntime::disabled_with_runtime_id("aster_test", "runtime-a");

        runtime
            .publish_reload(["feature"], ConfigNotificationSource::Api)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn disabled_config_sync_waits_for_shutdown_without_invoking_callbacks() {
        let runtime = ConfigSyncRuntime::disabled_with_runtime_id("aster_test", "runtime-a");
        let shutdown = CancellationToken::new();
        let worker_shutdown = shutdown.clone();
        let reconciles = Arc::new(AtomicUsize::new(0));
        let reloads = Arc::new(AtomicUsize::new(0));
        let worker_reconciles = reconciles.clone();
        let worker_reloads = reloads.clone();

        let worker = tokio::spawn(async move {
            runtime
                .run_reload_subscription_with_reconcile(
                    worker_shutdown,
                    move || {
                        let worker_reconciles = worker_reconciles.clone();
                        async move {
                            worker_reconciles.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    },
                    move |_| {
                        let worker_reloads = worker_reloads.clone();
                        async move {
                            worker_reloads.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    },
                )
                .await
        });

        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        assert_eq!(reconciles.load(Ordering::SeqCst), 0);
        assert_eq!(reloads.load(Ordering::SeqCst), 0);

        shutdown.cancel();
        timeout(Duration::from_millis(100), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
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
    async fn reload_supervisor_recovers_from_initial_subscribe_failures_and_reconciles() {
        let (sender, _) = broadcast::channel(8);
        let notifier = Arc::new(ScriptedConfigNotifier::new([
            SubscribeStep::Fail("redis unavailable"),
            SubscribeStep::Channel(sender.clone()),
        ]));
        let shutdown = CancellationToken::new();
        let reconciles = Arc::new(AtomicUsize::new(0));
        let observer = Arc::new(TestConnectionObserver::default());
        let worker_notifier = notifier.clone();
        let worker_shutdown = shutdown.clone();
        let worker_reconciles = reconciles.clone();
        let worker_observer = observer.clone();

        let worker = tokio::spawn(async move {
            let mut reconcile = move || {
                let worker_reconciles = worker_reconciles.clone();
                async move {
                    worker_reconciles.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            };
            let mut reload = |_| async { Ok(()) };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                zero_reconnect_policy(),
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                None,
                Some(worker_observer.as_ref()),
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while reconciles.load(Ordering::SeqCst) != 1 || notifier.subscribe_attempts() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let observations = observer.snapshot();
        let states = observations
            .iter()
            .map(|observation| observation.state)
            .collect::<Vec<_>>();
        assert_eq!(
            states,
            vec![
                ConfigSyncConnectionState::Disconnected,
                ConfigSyncConnectionState::Reconnecting,
                ConfigSyncConnectionState::Recovered,
            ]
        );
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation.reconnect_attempt)
                .collect::<Vec<_>>(),
            vec![1, 1, 1]
        );
        assert!(
            observations
                .iter()
                .all(|observation| observation.backoff_seconds == 0.0)
        );

        shutdown.cancel();
        worker.await.unwrap().unwrap();
        drop(sender);
    }

    #[tokio::test]
    async fn reload_supervisor_subscribes_before_reconcile_to_close_startup_race() {
        let notifier = Arc::new(InMemoryConfigNotifier::default());
        let shutdown = CancellationToken::new();
        let worker_shutdown = shutdown.clone();
        let worker_notifier = notifier.clone();
        let reloads = Arc::new(AtomicUsize::new(0));
        let worker_reloads = reloads.clone();

        let worker = tokio::spawn(async move {
            let reconcile_notifier = worker_notifier.clone();
            let mut reconcile = move || {
                let reconcile_notifier = reconcile_notifier.clone();
                async move {
                    reconcile_notifier
                        .publish_reload(ConfigReloadMessage::new(
                            "aster_test",
                            "node-b",
                            ["during_reconcile"],
                            ConfigNotificationSource::Api,
                        ))
                        .await
                }
            };
            let mut reload = move |_| {
                let worker_reloads = worker_reloads.clone();
                async move {
                    worker_reloads.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                zero_reconnect_policy(),
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                None,
                None,
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while reloads.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        shutdown.cancel();
        worker.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn lagged_subscription_reconnects_and_reconciles_without_reload_observation() {
        let (first_sender, _) = broadcast::channel(1);
        let (second_sender, _) = broadcast::channel(1);
        let notifier = Arc::new(ScriptedConfigNotifier::new([
            SubscribeStep::Channel(first_sender.clone()),
            SubscribeStep::Channel(second_sender.clone()),
        ]));
        let shutdown = CancellationToken::new();
        let worker_shutdown = shutdown.clone();
        let worker_notifier = notifier.clone();
        let reconciles = Arc::new(AtomicUsize::new(0));
        let worker_reconciles = reconciles.clone();
        let reload_observer = Arc::new(TestReloadObserver::default());
        let worker_reload_observer = reload_observer.clone();

        let worker = tokio::spawn(async move {
            let lag_sender = first_sender.clone();
            let mut reconcile = move || {
                let lag_sender = lag_sender.clone();
                let worker_reconciles = worker_reconciles.clone();
                async move {
                    let attempt = worker_reconciles.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        for key in ["first", "second"] {
                            lag_sender
                                .send(ConfigChangeEvent::Reload(ConfigReloadMessage::new(
                                    "aster_test",
                                    "node-b",
                                    [key],
                                    ConfigNotificationSource::Api,
                                )))
                                .expect("lag test subscription should still exist");
                        }
                    }
                    Ok(())
                }
            };
            let mut reload = |_| async { Ok(()) };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                zero_reconnect_policy(),
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                Some(worker_reload_observer.as_ref()),
                None,
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while reconciles.load(Ordering::SeqCst) != 2 || notifier.subscribe_attempts() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert!(reload_observer.snapshot().is_empty());
        shutdown.cancel();
        worker.await.unwrap().unwrap();
        drop(second_sender);
    }

    #[tokio::test]
    async fn stable_subscription_resets_reconnect_attempt_sequence() {
        let (first_sender, _) = broadcast::channel(8);
        let (second_sender, _) = broadcast::channel(8);
        let notifier = Arc::new(ScriptedConfigNotifier::new([
            SubscribeStep::Fail("initial outage"),
            SubscribeStep::Channel(first_sender.clone()),
            SubscribeStep::Channel(second_sender.clone()),
        ]));
        let shutdown = CancellationToken::new();
        let worker_shutdown = shutdown.clone();
        let worker_notifier = notifier.clone();
        let reconciles = Arc::new(AtomicUsize::new(0));
        let worker_reconciles = reconciles.clone();
        let observer = Arc::new(TestConnectionObserver::default());
        let worker_observer = observer.clone();
        let policy = ConfigReloadReconnectPolicy {
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            stable_reset_after: Duration::ZERO,
        };

        let worker = tokio::spawn(async move {
            let mut reconcile = move || {
                let worker_reconciles = worker_reconciles.clone();
                async move {
                    worker_reconciles.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            };
            let mut reload = |_| async { Ok(()) };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                policy,
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                None,
                Some(worker_observer.as_ref()),
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while reconciles.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(first_sender);
        timeout(Duration::from_secs(1), async {
            while reconciles.load(Ordering::SeqCst) != 2 || notifier.subscribe_attempts() != 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let recovered_attempts = observer
            .snapshot()
            .into_iter()
            .filter(|observation| observation.state == ConfigSyncConnectionState::Recovered)
            .map(|observation| observation.reconnect_attempt)
            .collect::<Vec<_>>();
        assert_eq!(recovered_attempts, vec![1, 1]);

        shutdown.cancel();
        worker.await.unwrap().unwrap();
        drop(second_sender);
    }

    #[tokio::test]
    async fn reload_supervisor_reconnects_after_subscription_closes_and_reconciles_again() {
        let (first_sender, _) = broadcast::channel(8);
        let (second_sender, _) = broadcast::channel(8);
        let notifier = Arc::new(ScriptedConfigNotifier::new([
            SubscribeStep::Channel(first_sender.clone()),
            SubscribeStep::Channel(second_sender.clone()),
        ]));
        let shutdown = CancellationToken::new();
        let reconciles = Arc::new(AtomicUsize::new(0));
        let observer = Arc::new(TestConnectionObserver::default());
        let worker_notifier = notifier.clone();
        let worker_shutdown = shutdown.clone();
        let worker_reconciles = reconciles.clone();
        let worker_observer = observer.clone();

        let worker = tokio::spawn(async move {
            let mut reconcile = move || {
                let worker_reconciles = worker_reconciles.clone();
                async move {
                    worker_reconciles.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            };
            let mut reload = |_| async { Ok(()) };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                zero_reconnect_policy(),
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                None,
                Some(worker_observer.as_ref()),
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while reconciles.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(first_sender);

        timeout(Duration::from_secs(1), async {
            while reconciles.load(Ordering::SeqCst) != 2 || notifier.subscribe_attempts() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let states = observer
            .snapshot()
            .into_iter()
            .map(|observation| observation.state)
            .collect::<Vec<_>>();
        assert_eq!(
            states,
            vec![
                ConfigSyncConnectionState::Connected,
                ConfigSyncConnectionState::Disconnected,
                ConfigSyncConnectionState::Reconnecting,
                ConfigSyncConnectionState::Recovered,
            ]
        );

        shutdown.cancel();
        worker.await.unwrap().unwrap();
        drop(second_sender);
    }

    #[tokio::test]
    async fn reload_supervisor_keeps_subscription_after_reconcile_error() {
        let (sender, _) = broadcast::channel(8);
        let notifier = Arc::new(ScriptedConfigNotifier::new([SubscribeStep::Channel(
            sender.clone(),
        )]));
        let shutdown = CancellationToken::new();
        let reconcile_attempts = Arc::new(AtomicUsize::new(0));
        let reloads = Arc::new(AtomicUsize::new(0));
        let worker_notifier = notifier.clone();
        let worker_shutdown = shutdown.clone();
        let worker_reconcile_attempts = reconcile_attempts.clone();
        let worker_reloads = reloads.clone();

        let worker = tokio::spawn(async move {
            let mut reconcile = move || {
                let worker_reconcile_attempts = worker_reconcile_attempts.clone();
                async move {
                    worker_reconcile_attempts.fetch_add(1, Ordering::SeqCst);
                    Err(ConfigCoreError::store("temporary reconcile failure"))
                }
            };
            let mut reload = move |_| {
                let worker_reloads = worker_reloads.clone();
                async move {
                    worker_reloads.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                zero_reconnect_policy(),
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                None,
                None,
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while reconcile_attempts.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        sender
            .send(ConfigChangeEvent::Reload(ConfigReloadMessage::new(
                "aster_test",
                "node-b",
                ["feature"],
                ConfigNotificationSource::Api,
            )))
            .unwrap();

        timeout(Duration::from_secs(1), async {
            while reloads.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(notifier.subscribe_attempts(), 1);

        shutdown.cancel();
        worker.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn reload_supervisor_shutdown_interrupts_reconnect_backoff() {
        let notifier = Arc::new(ScriptedConfigNotifier::new([SubscribeStep::Fail(
            "redis unavailable",
        )]));
        let shutdown = CancellationToken::new();
        let observer = Arc::new(TestConnectionObserver::default());
        let worker_shutdown = shutdown.clone();
        let worker_observer = observer.clone();
        let policy = ConfigReloadReconnectPolicy {
            initial_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(60),
            stable_reset_after: Duration::from_secs(30),
        };

        let worker = tokio::spawn(async move {
            let mut reconcile = || async { Ok(()) };
            let mut reload = |_| async { Ok(()) };
            run_config_reload_supervisor_inner(
                notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                policy,
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                None,
                Some(worker_observer.as_ref()),
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while !observer
                .snapshot()
                .iter()
                .any(|observation| observation.state == ConfigSyncConnectionState::Reconnecting)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        shutdown.cancel();
        timeout(Duration::from_millis(100), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn reload_supervisor_shutdown_interrupts_pending_subscribe() {
        let notifier = Arc::new(ScriptedConfigNotifier::new([SubscribeStep::Pending]));
        let shutdown = CancellationToken::new();
        let worker_shutdown = shutdown.clone();
        let worker_notifier = notifier.clone();

        let worker = tokio::spawn(async move {
            let mut reconcile = || async { Ok(()) };
            let mut reload = |_| async { Ok(()) };
            run_config_reload_supervisor_inner(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                zero_reconnect_policy(),
                worker_shutdown,
                &mut reconcile,
                &mut reload,
                None,
                None,
            )
            .await
        });

        timeout(Duration::from_secs(1), async {
            while notifier.subscribe_attempts() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        shutdown.cancel();
        timeout(Duration::from_millis(100), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[test]
    fn reconnect_delay_grows_with_equal_jitter_and_caps() {
        let policy = ConfigReloadReconnectPolicy {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(250),
            stable_reset_after: Duration::from_secs(30),
        };
        let expected_bounds = [(1, 50, 100), (2, 100, 200), (3, 125, 250), (64, 125, 250)];

        for (attempt, min_ms, max_ms) in expected_bounds {
            for _ in 0..64 {
                let delay_ms =
                    super::duration_millis_u64(config_reload_reconnect_delay(policy, attempt));
                assert!(
                    (min_ms..=max_ms).contains(&delay_ms),
                    "attempt {attempt} produced {delay_ms}ms outside [{min_ms}, {max_ms}]"
                );
            }
        }
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

    #[derive(Default)]
    struct TestReloadObserver {
        observations: Mutex<Vec<super::ConfigReloadObservation>>,
    }

    impl super::ConfigReloadObserver for TestReloadObserver {
        fn observe_config_reload(&self, observation: super::ConfigReloadObservation) {
            self.observations.lock().unwrap().push(observation);
        }
    }

    impl TestReloadObserver {
        fn snapshot(&self) -> Vec<super::ConfigReloadObservation> {
            self.observations.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn reload_worker_observes_decisions_and_reload_errors() {
        let notifier = Arc::new(InMemoryConfigNotifier::default());
        let shutdown = CancellationToken::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = attempts.clone();
        let observer = Arc::new(TestReloadObserver::default());
        let worker_observer = observer.clone();
        let worker_shutdown = shutdown.clone();
        let worker_notifier = notifier.clone();

        let worker = tokio::spawn(async move {
            run_config_reload_worker_with_observer(
                worker_notifier,
                ConfigReloadWorkerConfig::new("aster_test", "node-a"),
                worker_shutdown,
                move |_| {
                    let observed_attempts = observed_attempts.clone();
                    async move {
                        let attempt = observed_attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err(ConfigCoreError::store("temporary reload failure"))
                        } else {
                            Ok(())
                        }
                    }
                },
                Some(worker_observer.as_ref()),
            )
            .await
        });

        wait_for_subscriber(&notifier).await;

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
                "node-a",
                ["local"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();
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
                ["second", "third"],
                ConfigNotificationSource::Api,
            ))
            .await
            .unwrap();

        timeout(Duration::from_secs(1), async {
            while observer.snapshot().len() != 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        shutdown.cancel();
        worker.await.unwrap().unwrap();

        let observations = observer.snapshot();
        assert_eq!(observations.len(), 4);
        assert_eq!(
            observations[0].decision,
            ConfigReloadDecision::IgnoredNamespace
        );
        assert_eq!(observations[0].status, "ok");
        assert_eq!(observations[0].changed_keys, 1);
        assert_eq!(
            observations[1].decision,
            ConfigReloadDecision::IgnoredOrigin
        );
        assert_eq!(observations[1].status, "ok");
        assert_eq!(observations[2].decision, ConfigReloadDecision::Reloaded);
        assert_eq!(observations[2].status, "error");
        assert_eq!(observations[3].decision, ConfigReloadDecision::Reloaded);
        assert_eq!(observations[3].status, "ok");
        assert_eq!(observations[3].changed_keys, 2);
        assert!(observations.iter().all(|observation| {
            observation.source == "pubsub" && observation.duration_seconds >= 0.0
        }));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}

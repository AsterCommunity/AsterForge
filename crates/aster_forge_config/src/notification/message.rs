use serde::{Deserialize, Serialize};
use std::future::Future;

use crate::Result;

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

/// Decodes one transport payload into a config reload event.
///
/// Transport adapters should use this helper before forwarding data into the common notifier path.
/// Malformed payloads are returned as errors so listeners can log and continue instead of ending the
/// subscription loop.
pub fn decode_config_reload_transport_payload(payload: &str) -> Result<ConfigChangeEvent> {
    ConfigReloadMessage::decode(payload).map(ConfigChangeEvent::Reload)
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

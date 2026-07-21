use std::future::Future;
use tokio_util::sync::CancellationToken;

use crate::Result;

use super::message::{ConfigNotificationSource, ConfigReloadMessage, ConfigReloadWorkerConfig};
use super::notifier::SharedConfigChangeNotifier;
use super::supervisor::{
    ConfigReloadObserver, ConfigSyncConnectionObserver, run_config_reload_supervisor,
    run_config_reload_supervisor_with_observers, run_config_reload_worker,
    run_config_reload_worker_with_observer,
};

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

//! Product runtime state.
//!
//! Store product-owned resources here. Clone only cheap handles such as `Arc`, `web::Data`,
//! `DatabaseConnection`, `DbHandles`, and shared sender/metrics handles.

use std::sync::Arc;

use crate::config::AppConfig;

/// Shared runtime state passed to HTTP handlers and component factories.
#[derive(Clone)]
pub struct AppState {
    /// Static service configuration.
    pub config: Arc<AppConfig>,
    /// Database reader/writer handles.
    pub db_handles: aster_forge_db::DbHandles,
    /// Shared cache backend.
    pub cache: Arc<dyn aster_forge_cache::CacheBackend>,
    /// Cross-process runtime config reload handle.
    pub config_sync: aster_forge_config::ConfigSyncRuntime,
    /// Shared metrics recorder.
    pub metrics: aster_forge_metrics::SharedMetricsRecorder,
    /// Product mail sender.
    pub mail_sender: Arc<dyn aster_forge_mail::MailSender>,
}

impl AppState {
    /// Creates runtime state from prepared resources.
    pub fn new(
        config: AppConfig,
        db_handles: aster_forge_db::DbHandles,
        cache: Arc<dyn aster_forge_cache::CacheBackend>,
        config_sync: aster_forge_config::ConfigSyncRuntime,
        metrics: aster_forge_metrics::SharedMetricsRecorder,
        mail_sender: Arc<dyn aster_forge_mail::MailSender>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            db_handles,
            cache,
            config_sync,
            metrics,
            mail_sender,
        }
    }

    /// Returns the configured cache backend name.
    pub fn cache_backend_name(&self) -> &'static str {
        self.cache.backend_name()
    }

    /// Returns whether cross-process config reload notifications are enabled.
    pub fn config_sync_enabled(&self) -> bool {
        self.config_sync.enabled()
    }

    /// Returns the generated runtime instance ID.
    pub fn runtime_id(&self) -> &str {
        self.config_sync.runtime_id()
    }
}

//! Runtime state and lifecycle modules.

pub mod logging;
pub mod panic;
pub mod shutdown;
pub mod startup;
pub mod tasks;

use crate::cache::CacheBackend;
use crate::config::{Config, RuntimeConfig};
use crate::db::DbHandles;
use crate::metrics_core::SharedMetricsRecorder;
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AppState {
    pub db_handles: DbHandles,
    pub config: Arc<Config>,
    pub runtime_config: Arc<RuntimeConfig>,
    pub cache: Arc<dyn CacheBackend>,
    pub metrics: SharedMetricsRecorder,
    pub background_task_dispatch_wakeup: Arc<Notify>,
}

impl AppState {
    pub fn new_background_task_dispatch_wakeup() -> Arc<Notify> {
        Arc::new(Notify::new())
    }

    pub fn wake_background_task_dispatcher(&self) {
        self.background_task_dispatch_wakeup.notify_one();
    }
}

pub trait SharedRuntimeState {
    fn writer_db(&self) -> &DatabaseConnection;
    fn reader_db(&self) -> &DatabaseConnection;
    fn config(&self) -> &Arc<Config>;
    fn runtime_config(&self) -> &Arc<RuntimeConfig>;
    fn cache(&self) -> &Arc<dyn CacheBackend>;
    fn metrics(&self) -> &SharedMetricsRecorder;
}

impl SharedRuntimeState for AppState {
    fn writer_db(&self) -> &DatabaseConnection {
        self.db_handles.writer()
    }

    fn reader_db(&self) -> &DatabaseConnection {
        self.db_handles.reader()
    }

    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn runtime_config(&self) -> &Arc<RuntimeConfig> {
        &self.runtime_config
    }

    fn cache(&self) -> &Arc<dyn CacheBackend> {
        &self.cache
    }

    fn metrics(&self) -> &SharedMetricsRecorder {
        &self.metrics
    }
}

use std::sync::Arc;

use crate::cache;
use crate::config::{Config, RuntimeConfig};
use crate::db;
use crate::errors::{AsterError, MapAsterErr, Result};
use crate::metrics_core::SharedMetricsRecorder;
use crate::services::system_config_service;

pub(super) struct CommonRuntimeParts {
    pub config: Arc<Config>,
    pub db_handles: db::DbHandles,
    pub runtime_config: Arc<RuntimeConfig>,
    pub cache: Arc<dyn cache::CacheBackend>,
    pub metrics: SharedMetricsRecorder,
}

pub(super) async fn prepare_common(config: Arc<Config>) -> Result<CommonRuntimeParts> {
    crate::utils::paths::ensure_runtime_dirs(&config.server.temp_dir).await?;

    let metrics = create_metrics_recorder();
    let writer = db::connect_with_metrics(&config.database, metrics.clone()).await?;
    migration::Migrator::up(&writer, None)
        .await
        .map_aster_err(AsterError::database_operation)?;
    let db_handles =
        db::connect_reader_for_writer_with_metrics(&config.database, writer, metrics.clone())
            .await?;

    system_config_service::ensure_defaults(db_handles.writer()).await?;
    let runtime_config = Arc::new(RuntimeConfig::new());
    runtime_config.reload(db_handles.reader()).await?;
    let cache = cache::create_cache(&config.cache).await;

    crate::services::audit_service::init_global_audit_log_manager(db_handles.writer().clone());

    Ok(CommonRuntimeParts {
        config,
        db_handles,
        runtime_config,
        cache,
        metrics,
    })
}

fn create_metrics_recorder() -> SharedMetricsRecorder {
    #[cfg(feature = "metrics")]
    {
        match crate::metrics::init_metrics() {
            Ok(()) => {
                tracing::info!("prometheus metrics initialized");
                return std::sync::Arc::new(crate::metrics::PrometheusMetricsRecorder);
            }
            Err(error) => {
                tracing::warn!("failed to initialize prometheus metrics: {error}");
            }
        }
    }

    crate::metrics_core::NoopMetrics::arc()
}

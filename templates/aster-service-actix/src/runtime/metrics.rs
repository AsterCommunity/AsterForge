//! Product feature policy and explicit metrics resource construction.
use aster_forge_metrics::{InfrastructureMetricsRecorder, MetricsProvider, SharedMetricsRecorder};
use std::sync::Arc;

pub fn create_metrics_recorder() -> crate::errors::Result<SharedMetricsRecorder> {
    let builder = MetricsProvider::builder();
    #[cfg(feature = "metrics")]
    let builder = builder
        .with_prometheus()
        .map_err(|error| crate::errors::AppError::Config(error.to_string()))?;
    let provider = builder
        .build()
        .map_err(|error| crate::errors::AppError::Config(error.to_string()))?;
    if !provider.enabled() {
        return Ok(aster_forge_metrics::NoopMetrics::arc());
    }
    Ok(Arc::new(
        InfrastructureMetricsRecorder::new(provider)
            .map_err(|error| crate::errors::AppError::Config(error.to_string()))?,
    ))
}

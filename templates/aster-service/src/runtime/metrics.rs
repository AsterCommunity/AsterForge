//! Runtime metrics recorder initialization.

use aster_forge_metrics::SharedMetricsRecorder;

/// Creates the runtime metrics recorder for this build.
pub fn create_metrics_recorder() -> SharedMetricsRecorder {
    #[cfg(feature = "metrics")]
    {
        aster_forge_metrics::init_metrics_or_noop(crate::metrics::init_metrics, || {
            crate::metrics::PrometheusMetricsRecorder
        })
    }

    #[cfg(not(feature = "metrics"))]
    {
        aster_forge_metrics::NoopMetrics::arc()
    }
}

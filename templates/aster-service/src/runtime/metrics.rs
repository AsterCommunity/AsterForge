//! Runtime metrics recorder initialization.

use aster_forge_metrics::SharedMetricsRecorder;

/// Creates the runtime metrics recorder for this build.
pub fn create_metrics_recorder() -> SharedMetricsRecorder {
    #[cfg(feature = "metrics")]
    {
        aster_forge_metrics::init_configured_or_noop()
    }

    #[cfg(not(feature = "metrics"))]
    {
        aster_forge_metrics::NoopMetrics::arc()
    }
}

//! Metrics backend provided by AsterForge.

use actix_web::Scope;
use aster_forge_runtime::{HealthCheckScope, SystemHealthReport};

#[cfg(feature = "metrics")]
use aster_forge_metrics::prometheus::PrometheusMetricsRecorder;

/// Records a health report when metrics are enabled.
pub fn record_health_report(scope: HealthCheckScope, report: &SystemHealthReport) {
    #[cfg(feature = "metrics")]
    report.record_metrics(scope.as_str(), &PrometheusMetricsRecorder);

    #[cfg(not(feature = "metrics"))]
    let _ = (scope, report);
}

/// Adds the metrics HTTP route when the metrics feature is enabled.
pub fn configure_route(scope: Scope) -> Scope {
    aster_forge_actix_observability::configure_prometheus_route(scope)
}

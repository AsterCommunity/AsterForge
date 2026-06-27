//! Prometheus-backed `MetricsRecorder`.

/// Prometheus recorder used when the `metrics` feature is enabled.
pub struct PrometheusMetricsRecorder;

impl aster_forge_metrics::DbMetricsRecorder for PrometheusMetricsRecorder {
    fn enabled(&self) -> bool {
        true
    }

    fn record_db_query(&self, metric: &aster_forge_metrics::DbQueryMetric) {
        super::registry::record_db_query(metric);
    }
}

impl aster_forge_metrics::MetricsRecorder for PrometheusMetricsRecorder {
    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {
        super::registry::record_http_request(method, route, status, duration_seconds);
    }
}

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

    fn record_auth_event(&self, action: &'static str, status: &'static str, reason: &'static str) {
        super::registry::record_auth_event(action, status, reason);
    }

    fn record_application_event(
        &self,
        category: &'static str,
        event: &'static str,
        status: &'static str,
    ) {
        super::registry::record_application_event(category, event, status);
    }

    fn record_background_task_transition(&self, kind: &'static str, status: &'static str) {
        super::registry::record_background_task_transition(kind, status);
    }

    fn set_background_tasks_pending(&self, pending: u64) {
        super::registry::set_background_tasks_pending(pending);
    }

    fn record_external_operation(
        &self,
        system: &'static str,
        operation: &'static str,
        status: &'static str,
        duration_seconds: f64,
    ) {
        super::registry::record_external_operation(system, operation, status, duration_seconds);
    }

    fn system_metrics_updater_task(
        &self,
        shutdown_token: tokio_util::sync::CancellationToken,
    ) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>> {
        Some(Box::pin(super::system::system_metrics_updater_task(
            shutdown_token,
        )))
    }
}

impl aster_forge_runtime::HealthMetricsRecorder for PrometheusMetricsRecorder {
    fn record_health_report(
        &self,
        scope: &'static str,
        status: aster_forge_runtime::HealthStatus,
        duration_seconds: f64,
    ) {
        super::registry::record_health_report(scope, status, duration_seconds);
    }

    fn record_health_component(
        &self,
        scope: &'static str,
        component: &aster_forge_runtime::HealthComponentReport,
        duration_seconds: f64,
    ) {
        super::registry::record_health_component(
            scope,
            component.name,
            component.status,
            duration_seconds,
        );
    }
}

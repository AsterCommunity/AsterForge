//! Prometheus-backed `MetricsRecorder`.

pub struct PrometheusMetricsRecorder;

impl crate::metrics_core::MetricsRecorder for PrometheusMetricsRecorder {
    fn enabled(&self) -> bool {
        true
    }

    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {
        super::registry::record_http_request(method, route, status, duration_seconds);
    }

    fn record_db_query(&self, info: &sea_orm::metric::Info<'_>) {
        super::registry::record_db_query(info);
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

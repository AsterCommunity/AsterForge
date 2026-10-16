//! Shared infrastructure schema, instance recorder and one system sampler.
use crate::{
    DbMetricsRecorder, DbQueryMetric, MetricDescriptor, MetricError, MetricResult, MetricsProvider,
    MetricsRecorder, ProductCounter, ProductGauge, ProductHistogram,
};
use std::{future::Future, pin::Pin, sync::Arc, time::Instant};
use tokio_util::sync::CancellationToken;

struct InfrastructureMetricSet {
    http_requests_total: ProductCounter,
    http_request_duration_seconds: ProductHistogram,
    db_queries_total: ProductCounter,
    db_query_duration_seconds: ProductHistogram,
    auth_events_total: ProductCounter,
    application_events_total: ProductCounter,
    config_reloads_total: ProductCounter,
    config_reload_duration_seconds: ProductHistogram,
    config_reload_changed_keys: ProductHistogram,
    config_mutations_total: ProductCounter,
    config_mutation_changed_keys: ProductHistogram,
    background_tasks_total: ProductCounter,
    background_tasks_pending: ProductGauge,
    background_task_retries_total: ProductCounter,
    external_operations_total: ProductCounter,
    external_operation_duration_seconds: ProductHistogram,
    #[cfg(feature = "runtime-health")]
    health_report_status: ProductGauge,
    #[cfg(feature = "runtime-health")]
    health_report_duration_seconds: ProductHistogram,
    #[cfg(feature = "runtime-health")]
    health_component_status: ProductGauge,
    #[cfg(feature = "runtime-health")]
    health_component_duration_seconds: ProductHistogram,
    process_memory_rss_bytes: ProductGauge,
    process_cpu_milliseconds_total: ProductGauge,
    uptime_seconds: ProductGauge,
    #[cfg(feature = "allocator-metrics")]
    process_heap_memory_mib: ProductGauge,
    #[cfg(feature = "otlp")]
    exporter_successes: ProductGauge,
    #[cfg(feature = "otlp")]
    exporter_failures: ProductGauge,
    #[cfg(feature = "otlp")]
    exporter_last_success: ProductGauge,
}
impl InfrastructureMetricSet {
    #[expect(
        clippy::too_many_lines,
        reason = "the shared schema and atomic typed registration are reviewed together"
    )]
    fn new(provider: &MetricsProvider) -> MetricResult<Self> {
        let descriptors = [
            MetricDescriptor::counter("", "http_requests_total", "Total HTTP requests", &["method", "route", "status"]).with_unit("1"),
            MetricDescriptor::histogram_with_buckets("", "http_request_duration_seconds", "HTTP request duration in seconds", &["method", "route", "status"], &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0]).with_unit("s"),
            MetricDescriptor::counter("", "db_queries_total", "Total database queries observed through the shared database metrics adapter", &["backend", "kind", "status"]).with_unit("1"),
            MetricDescriptor::histogram_with_buckets("", "db_query_duration_seconds", "Database query duration in seconds", &["backend", "kind", "status"], &[
                0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]).with_unit("s"),
            MetricDescriptor::counter("", "auth_events_total", "Total authentication events", &["action", "status", "reason"]).with_unit("1"),
            MetricDescriptor::counter("", "application_events_total", "Total low-cardinality application events", &["category", "event", "status"]).with_unit("1"),
            MetricDescriptor::counter("", "config_reloads_total", "Total runtime config reload attempts", &["source", "decision", "status"]).with_unit("1"),
            MetricDescriptor::histogram_with_buckets("", "config_reload_duration_seconds", "Runtime config reload duration in seconds", &["source", "decision", "status"], &[
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]).with_unit("s"),
            MetricDescriptor::histogram_with_buckets("", "config_reload_changed_keys", "Number of changed keys observed by runtime config reload attempts", &["source", "decision", "status"], &[0.0, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 500.0]).with_unit("1"),
            MetricDescriptor::counter("", "config_mutations_total", "Total runtime config mutation attempts", &["source", "operation", "status"]).with_unit("1"),
            MetricDescriptor::histogram_with_buckets("", "config_mutation_changed_keys", "Number of changed keys in runtime config mutation attempts", &["source", "operation", "status"], &[0.0, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0]).with_unit("1"),
            MetricDescriptor::counter("", "background_tasks_total", "Total background task state transitions", &["kind", "status"]).with_unit("1"),
            MetricDescriptor::gauge("", "background_tasks_pending", "Pending or retryable background task backlog", &[]).with_unit("1"),
            MetricDescriptor::counter("", "background_task_retries_total", "Total background task retry transitions", &["kind"]).with_unit("1"),
            MetricDescriptor::counter("", "external_operations_total", "Total operations against external systems", &["system", "operation", "status"]).with_unit("1"),
            MetricDescriptor::histogram_with_buckets("", "external_operation_duration_seconds", "External system operation duration in seconds", &["system", "operation", "status"], &[
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 15.0, 60.0,
            ]).with_unit("s"),
            #[cfg(feature = "runtime-health")]
            MetricDescriptor::gauge("", "health_report_status", "Aggregate health status for a health check scope: healthy=0, degraded=1, unhealthy=2", &["scope"]).with_unit("1"),
            #[cfg(feature = "runtime-health")]
            MetricDescriptor::histogram_with_buckets("", "health_report_duration_seconds", "Aggregate health check duration in seconds", &["scope", "status"], &[
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]).with_unit("s"),
            #[cfg(feature = "runtime-health")]
            MetricDescriptor::gauge("", "health_component_status", "Health component status for a health check scope: healthy=0, degraded=1, unhealthy=2", &["scope", "component"]).with_unit("1"),
            #[cfg(feature = "runtime-health")]
            MetricDescriptor::histogram_with_buckets("", "health_component_duration_seconds", "Health component check duration in seconds", &["scope", "component", "status"], &[
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]).with_unit("s"),
            MetricDescriptor::gauge("", "process_memory_rss_bytes", "Process RSS memory in bytes", &[]).with_unit("By"),
            MetricDescriptor::gauge("", "process_cpu_milliseconds_total", "Process accumulated CPU time in milliseconds", &[]).with_unit("ms"),
            MetricDescriptor::gauge("", "process_uptime_seconds", "Process uptime in seconds", &[]).with_unit("s"),
            #[cfg(feature = "allocator-metrics")]
            MetricDescriptor::gauge("", "process_heap_memory_mib", "Allocator heap memory in MiB: allocated and peak_or_resident", &["kind"]).with_unit("MiBy"),
            #[cfg(feature = "otlp")]
            MetricDescriptor::gauge("telemetry", "export_successes", "Cumulative successful push exports observed locally.", &["exporter"]),
            #[cfg(feature = "otlp")]
            MetricDescriptor::gauge("telemetry", "export_failures", "Cumulative failed push exports observed locally.", &["exporter"]),
            #[cfg(feature = "otlp")]
            MetricDescriptor::gauge("telemetry", "export_last_success_timestamp_seconds", "Last acknowledged push export Unix timestamp.", &["exporter"]).with_unit("s"),
];
        let mut handles = provider.register_batch(&descriptors)?.into_iter();
        Ok(Self {
            http_requests_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            http_request_duration_seconds: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            db_queries_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            db_query_duration_seconds: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            auth_events_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            application_events_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            config_reloads_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            config_reload_duration_seconds: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            config_reload_changed_keys: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            config_mutations_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            config_mutation_changed_keys: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            background_tasks_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            background_tasks_pending: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            background_task_retries_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            external_operations_total: ProductCounter::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            external_operation_duration_seconds: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "runtime-health")]
            health_report_status: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "runtime-health")]
            health_report_duration_seconds: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "runtime-health")]
            health_component_status: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "runtime-health")]
            health_component_duration_seconds: ProductHistogram::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            process_memory_rss_bytes: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            process_cpu_milliseconds_total: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            uptime_seconds: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "allocator-metrics")]
            process_heap_memory_mib: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "otlp")]
            exporter_successes: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "otlp")]
            exporter_failures: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
            #[cfg(feature = "otlp")]
            exporter_last_success: ProductGauge::from_handle(
                handles.next().ok_or(MetricError::InvalidDescriptor)?,
            )?,
        })
    }
    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {
        let metrics = self;

        let status = status.to_string();
        metrics
            .http_requests_total
            .inc(&[method, route, &status], 1);
        metrics
            .http_request_duration_seconds
            .observe(&[method, route, &status], duration_seconds);
    }

    fn record_db_query(&self, metric: &DbQueryMetric) {
        let metrics = self;

        let backend = metric.backend.as_label();
        let kind = metric.kind.as_label();
        let status = metric.status_label();

        metrics.db_queries_total.inc(&[backend, kind, status], 1);
        metrics
            .db_query_duration_seconds
            .observe(&[backend, kind, status], metric.elapsed.as_secs_f64());
    }

    fn record_auth_event(&self, action: &'static str, status: &'static str, reason: &'static str) {
        let metrics = self;

        metrics.auth_events_total.inc(&[action, status, reason], 1);
    }

    fn record_application_event(
        &self,
        category: &'static str,
        event: &'static str,
        status: &'static str,
    ) {
        let metrics = self;

        metrics
            .application_events_total
            .inc(&[category, event, status], 1);
    }

    fn record_config_reload(
        &self,
        source: &'static str,
        decision: &'static str,
        status: &'static str,
        changed_keys: u64,
        duration_seconds: f64,
    ) {
        let metrics = self;

        metrics
            .config_reloads_total
            .inc(&[source, decision, status], 1);
        metrics
            .config_reload_duration_seconds
            .observe(&[source, decision, status], duration_seconds);
        metrics
            .config_reload_changed_keys
            .observe(&[source, decision, status], metric_u64(changed_keys));
    }

    fn record_config_mutation(
        &self,
        source: &'static str,
        operation: &'static str,
        status: &'static str,
        changed_keys: u64,
    ) {
        let metrics = self;

        metrics
            .config_mutations_total
            .inc(&[source, operation, status], 1);
        metrics
            .config_mutation_changed_keys
            .observe(&[source, operation, status], metric_u64(changed_keys));
    }

    fn record_background_task_transition(&self, kind: &'static str, status: &'static str) {
        let metrics = self;

        metrics.background_tasks_total.inc(&[kind, status], 1);
        if status == "retry" {
            metrics.background_task_retries_total.inc(&[kind], 1);
        }
    }

    fn set_background_tasks_pending(&self, pending: u64) {
        let metrics = self;

        metrics
            .background_tasks_pending
            .set(&[], metric_u64(pending));
    }

    fn record_external_operation(
        &self,
        system: &'static str,
        operation: &'static str,
        status: &'static str,
        duration_seconds: f64,
    ) {
        let metrics = self;

        metrics
            .external_operations_total
            .inc(&[system, operation, status], 1);
        metrics
            .external_operation_duration_seconds
            .observe(&[system, operation, status], duration_seconds);
    }

    /// Records an aggregate health report into the shared Prometheus registry.
    #[cfg(feature = "runtime-health")]
    fn record_health_report(
        &self,
        scope: &'static str,
        status_label: &'static str,
        status_value: f64,
        duration_seconds: f64,
    ) {
        let metrics = self;

        metrics.health_report_status.set(&[scope], status_value);
        metrics
            .health_report_duration_seconds
            .observe(&[scope, status_label], duration_seconds);
    }

    /// Records one health component into the shared Prometheus registry.
    #[cfg(feature = "runtime-health")]
    fn record_health_component(
        &self,
        scope: &'static str,
        component: &'static str,
        status_label: &'static str,
        status_value: f64,
        duration_seconds: f64,
    ) {
        let metrics = self;

        metrics
            .health_component_status
            .set(&[scope, component], status_value);
        metrics
            .health_component_duration_seconds
            .observe(&[scope, component, status_label], duration_seconds);
    }
}
#[expect(
    clippy::cast_precision_loss,
    reason = "Prometheus gauges and histograms use f64 samples; large integer observations are diagnostic approximations."
)]
fn metric_u64(value: u64) -> f64 {
    value as f64
}

#[derive(Clone)]
pub struct InfrastructureMetricsRecorder {
    provider: MetricsProvider,
    metrics: Arc<InfrastructureMetricSet>,
    started_at: Instant,
}
impl InfrastructureMetricsRecorder {
    /// # Errors
    /// Returns an error if the shared schema cannot be registered atomically on this provider.
    pub fn new(provider: MetricsProvider) -> MetricResult<Self> {
        let metrics = Arc::new(InfrastructureMetricSet::new(&provider)?);
        Ok(Self {
            provider,
            metrics,
            started_at: Instant::now(),
        })
    }
    #[must_use]
    pub fn provider(&self) -> &MetricsProvider {
        &self.provider
    }
}
impl DbMetricsRecorder for InfrastructureMetricsRecorder {
    fn enabled(&self) -> bool {
        self.provider.enabled()
    }

    fn record_db_query(&self, metric: &DbQueryMetric) {
        self.metrics.record_db_query(metric);
    }
}

impl MetricsRecorder for InfrastructureMetricsRecorder {
    fn provider(&self) -> Option<MetricsProvider> {
        Some(self.provider.clone())
    }
    #[cfg(feature = "runtime-health")]
    fn record_health(&self, scope: &'static str, report: &aster_forge_runtime::SystemHealthReport) {
        report.record_metrics(scope, self);
    }
    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {
        self.metrics
            .record_http_request(method, route, status, duration_seconds);
    }

    fn record_auth_event(&self, action: &'static str, status: &'static str, reason: &'static str) {
        self.metrics.record_auth_event(action, status, reason);
    }

    fn record_application_event(
        &self,
        category: &'static str,
        event: &'static str,
        status: &'static str,
    ) {
        self.metrics
            .record_application_event(category, event, status);
    }

    fn record_config_reload(
        &self,
        source: &'static str,
        decision: &'static str,
        status: &'static str,
        changed_keys: u64,
        duration_seconds: f64,
    ) {
        self.metrics
            .record_config_reload(source, decision, status, changed_keys, duration_seconds);
    }

    fn record_config_mutation(
        &self,
        source: &'static str,
        operation: &'static str,
        status: &'static str,
        changed_keys: u64,
    ) {
        self.metrics
            .record_config_mutation(source, operation, status, changed_keys);
    }

    fn record_background_task_transition(&self, kind: &'static str, status: &'static str) {
        self.metrics.record_background_task_transition(kind, status);
    }

    fn set_background_tasks_pending(&self, pending: u64) {
        self.metrics.set_background_tasks_pending(pending);
    }

    fn record_external_operation(
        &self,
        system: &'static str,
        operation: &'static str,
        status: &'static str,
        duration_seconds: f64,
    ) {
        self.metrics
            .record_external_operation(system, operation, status, duration_seconds);
    }

    fn system_metrics_updater_task(
        &self,
        shutdown_token: CancellationToken,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        if !self.provider.enabled() {
            return None;
        }
        Some(Box::pin(system_metrics_updater_task(
            self.clone(),
            shutdown_token,
        )))
    }
}

#[cfg(feature = "runtime-health")]
impl aster_forge_runtime::HealthMetricsRecorder for InfrastructureMetricsRecorder {
    fn record_health_report(
        &self,
        scope: &'static str,
        status: aster_forge_runtime::HealthStatus,
        duration_seconds: f64,
    ) {
        self.metrics.record_health_report(
            scope,
            status.as_str(),
            health_status_value(status.as_str()),
            duration_seconds,
        );
    }

    fn record_health_component(
        &self,
        scope: &'static str,
        component: &aster_forge_runtime::HealthComponentReport,
        duration_seconds: f64,
    ) {
        self.metrics.record_health_component(
            scope,
            component.name,
            component.status.as_str(),
            health_status_value(component.status.as_str()),
            duration_seconds,
        );
    }
}

#[cfg(feature = "runtime-health")]
fn health_status_value(status: &'static str) -> f64 {
    match status {
        "healthy" => 0.0,
        "degraded" => 1.0,
        _ => 2.0,
    }
}

async fn system_metrics_updater_task(
    recorder: InfrastructureMetricsRecorder,
    shutdown: CancellationToken,
) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let system = Arc::new(parking_lot::Mutex::new(System::new()));
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { biased; () = shutdown.cancelled() => break, _ = interval.tick() => {} }
        if !recorder.provider.enabled() {
            break;
        }
        let system = system.clone();
        let recorder = recorder.clone();
        let task = tokio::task::spawn_blocking(move || {
            let mut system = system.lock();
            let pid = Pid::from_u32(std::process::id());
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing().with_memory().with_cpu(),
            );
            if let Some(process) = system.process(pid) {
                recorder
                    .metrics
                    .process_memory_rss_bytes
                    .set(&[], metric_u64(process.memory()));
                recorder
                    .metrics
                    .process_cpu_milliseconds_total
                    .set(&[], metric_u64(process.accumulated_cpu_time()));
            }
            recorder
                .metrics
                .uptime_seconds
                .set(&[], recorder.started_at.elapsed().as_secs_f64());
            #[cfg(feature = "otlp")]
            if let Some(diagnostics) = recorder.provider.otlp_diagnostics() {
                let snapshot = diagnostics.snapshot();
                recorder
                    .metrics
                    .exporter_successes
                    .set(&["otlp"], metric_u64(snapshot.successes));
                recorder
                    .metrics
                    .exporter_failures
                    .set(&["otlp"], metric_u64(snapshot.failures));
                recorder
                    .metrics
                    .exporter_last_success
                    .set(&["otlp"], metric_u64(snapshot.last_success_unix_seconds));
            }
            #[cfg(feature = "allocator-metrics")]
            {
                let (allocated, resident) = aster_forge_alloc::stats();
                recorder
                    .metrics
                    .process_heap_memory_mib
                    .set(&["allocated"], allocated);
                recorder
                    .metrics
                    .process_heap_memory_mib
                    .set(&["peak_or_resident"], resident);
            }
        });
        if task.await.is_err() {
            tracing::warn!("system metrics sampler failed");
        }
    }
}

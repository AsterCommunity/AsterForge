//! Shared infrastructure schema, instance recorder and one system sampler.
use crate::{DbMetricsRecorder, DbQueryMetric, MetricResult, MetricsProvider, MetricsRecorder};
use std::{future::Future, pin::Pin, sync::Arc, time::Instant};
use tokio_util::sync::CancellationToken;

crate::product_metrics! {
    struct InfrastructureMetricSet {
        http_requests_total: counter("", "aster.http.server.requests", "aster.http.server.requests under aster-metrics/v1.", &["aster.request.outcome"]).with_unit("{request}"),
        http_request_duration_seconds: histogram_with_buckets("", "http.server.request.duration", "http.server.request.duration under aster-metrics/v1.", &["http.request.method", "url.scheme", "http.route", "http.response.status_code", "error.type"], &[0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0]).with_unit("s"),
        http_active: up_down_counter("", "http.server.active_requests", "http.server.active_requests under aster-metrics/v1.", &["http.request.method", "url.scheme"]).with_unit("{request}"),
        role_ready: gauge("", "aster.runtime.ready", "Current role ready endpoint result.", &[]).with_unit("{state}"),
        pool_connections: observable_up_down_counter("", "db.client.connection.count", "Native idle/used connection count.", &["db.client.connection.pool.name","db.client.connection.state"]).with_unit("{connection}"),
        pool_max: observable_up_down_counter("", "db.client.connection.max", "Actual finite connection pool limit.", &["db.client.connection.pool.name"]).with_unit("{connection}"),
        task_handler_active: up_down_counter("", "aster.task.handler.active", "Actually active local task handlers.", &["kind"]).with_unit("{execution}"),
        task_handler_duration: histogram_with_buckets("", "aster.task.handler.duration", "Local handler lifetime; return does not independently prove persistence.", &["kind","result"], &[0.01,0.1,1.0,5.0,15.0,60.0,300.0,900.0,3600.0]).with_unit("s"),
        db_query_duration_seconds: histogram_with_buckets("", "db.client.operation.duration", "db.client.operation.duration under aster-metrics/v1.", &["db.system.name", "db.operation.name", "error.type"], &[0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0]).with_unit("s"),
        auth_events_total: counter("", "aster.auth.events", "aster.auth.events under aster-metrics/v1.", &["action", "status", "reason"]).with_unit("{event}"),
        application_events_total: counter("", "aster.application.events", "aster.application.events under aster-metrics/v1.", &["category", "event", "status"]).with_unit("{event}"),
        config_reloads_total: counter("", "aster.config.reloads", "aster.config.reloads under aster-metrics/v1.", &["source", "decision", "status"]).with_unit("{operation}"),
        config_reload_duration_seconds: histogram_with_buckets("", "aster.config.reload.duration", "aster.config.reload.duration under aster-metrics/v1.", &["source", "decision", "status"], &[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0]).with_unit("s"),
        config_reload_changed_keys: histogram_with_buckets("", "aster.config.reload.changed_keys", "aster.config.reload.changed_keys under aster-metrics/v1.", &["source", "decision", "status"], &[0.0, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 500.0]).with_unit("{key}"),
        config_mutations_total: counter("", "aster.config.mutations", "aster.config.mutations under aster-metrics/v1.", &["source", "operation", "status"]).with_unit("{operation}"),
        config_mutation_changed_keys: histogram_with_buckets("", "aster.config.mutation.changed_keys", "aster.config.mutation.changed_keys under aster-metrics/v1.", &["source", "operation", "status"], &[0.0, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0]).with_unit("{key}"),
        background_tasks_total: counter("", "aster.task.transitions", "aster.task.transitions under aster-metrics/v1.", &["kind", "status"]).with_unit("{transition}"),
        background_tasks_pending: gauge("", "aster.task.pending", "aster.task.pending under aster-metrics/v1.", &[]).with_unit("{task}"),
        background_task_retries_total: counter("", "aster.task.retries", "aster.task.retries under aster-metrics/v1.", &["kind"]).with_unit("{retry}"),
        external_operations_total: counter("", "aster.external.operations", "aster.external.operations under aster-metrics/v1.", &["system", "operation", "status"]).with_unit("{operation}"),
        external_operation_duration_seconds: histogram_with_buckets("", "aster.external.operation.duration", "aster.external.operation.duration under aster-metrics/v1.", &["system", "operation", "status"], &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 15.0, 60.0]).with_unit("s"),
        #[cfg(feature = "runtime-health")]
        health_report_status: gauge("", "aster.runtime.health.status", "Aggregate health report: healthy=0, degraded=1, unhealthy=2.", &["scope"]).with_unit("{state}"),
        #[cfg(feature = "runtime-health")]
        health_report_duration_seconds: histogram_with_buckets("", "aster.runtime.health.duration", "aster.runtime.health.duration under aster-metrics/v1.", &["scope", "status"], &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]).with_unit("s"),
        #[cfg(feature = "runtime-health")]
        health_component_status: gauge("", "aster.runtime.component.health.status", "Component health: healthy=0, degraded=1, unhealthy=2.", &["scope", "component"]).with_unit("{state}"),
        #[cfg(feature = "runtime-health")]
        health_component_duration_seconds: histogram_with_buckets("", "aster.runtime.component.health.duration", "aster.runtime.component.health.duration under aster-metrics/v1.", &["scope", "component", "status"], &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]).with_unit("s"),
        process_memory_rss_bytes: observable_up_down_counter("", "process.memory.usage", "process.memory.usage under aster-metrics/v1.", &[]).with_unit("By"),
        process_cpu_time: observable_counter("", "process.cpu.time", "process.cpu.time under aster-metrics/v1.", &[]).with_unit("s"),
        uptime_seconds: gauge("", "process.uptime", "process.uptime under aster-metrics/v1.", &[]).with_unit("s"),
        observation_failures: observable_counter("", "aster.observability.failures", "aster.observability.failures under aster-metrics/v1.", &["component", "operation"]).with_unit("{failure}"),
        #[cfg(feature = "allocator-metrics")]
        process_heap_memory_mib: gauge("", "aster.process.heap", "aster.process.heap under aster-metrics/v1.", &["kind"]).with_unit("By"),
        #[cfg(feature = "otlp")]
        exporter_successes: observable_counter("", "aster.telemetry.exports", "aster.telemetry.exports under aster-metrics/v1.", &["exporter"]).with_unit("{export}"),
        #[cfg(feature = "otlp")]
        exporter_failures: observable_counter("", "aster.telemetry.export.failures", "aster.telemetry.export.failures under aster-metrics/v1.", &["exporter"]).with_unit("{failure}"),
        #[cfg(feature = "otlp")]
        exporter_last_success: gauge("", "aster.telemetry.export.last_success", "aster.telemetry.export.last_success under aster-metrics/v1.", &["exporter"]).with_unit("s"),
    }
}
impl InfrastructureMetricSet {
    fn new(provider: &MetricsProvider) -> MetricResult<Self> {
        let metrics = Self::register(provider)?;
        for method in [
            "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "CONNECT", "OPTIONS", "TRACE",
            "_OTHER",
        ] {
            for scheme in ["http", "https"] {
                metrics.http_active.add(&[method, scheme], 0);
            }
        }
        for outcome in [
            "completed",
            "rejected",
            "auth_rejected",
            "server_error",
            "canceled",
            "transport_error",
        ] {
            metrics.http_requests_total.inc(&[outcome], 0);
        }
        Ok(metrics)
    }
    fn record_http_request(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        duration_seconds: f64,
    ) {
        let metrics = self;

        let method = normalized_http_method(method);
        let outcome = match status {
            401 | 403 => "auth_rejected",
            400..=499 => "rejected",
            500..=599 => "server_error",
            _ => "completed",
        };
        metrics.http_requests_total.inc(&[outcome], 1);
        let status = if status == 0 {
            String::new()
        } else {
            status.to_string()
        };
        metrics.http_request_duration_seconds.observe(
            &[
                method,
                scheme,
                route,
                &status,
                if status.starts_with('5') {
                    status.as_str()
                } else {
                    ""
                },
            ],
            duration_seconds,
        );
    }
    fn record_db_query(&self, metric: &DbQueryMetric) {
        let backend = match metric.backend {
            crate::DbMetricBackend::Postgres => "postgresql",
            other => other.as_label(),
        };
        let operation = match metric.kind {
            crate::DbQueryKind::Select => "SELECT",
            crate::DbQueryKind::Insert => "INSERT",
            crate::DbQueryKind::Update => "UPDATE",
            crate::DbQueryKind::Delete => "DELETE",
            crate::DbQueryKind::Pragma => "PRAGMA",
            _ => "_OTHER",
        };
        self.db_query_duration_seconds.observe(
            &[backend, operation, if metric.failed { "other" } else { "" }],
            metric.elapsed.as_secs_f64(),
        );
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
    fn record_http_request(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        duration_seconds: f64,
    ) {
        self.metrics
            .record_http_request(method, scheme, route, status, duration_seconds);
    }

    fn record_http_stream_failure(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        duration: f64,
        cancelled: bool,
    ) {
        self.metrics.http_requests_total.inc(
            &[if cancelled {
                "canceled"
            } else {
                "transport_error"
            }],
            1,
        );
        let status = if status == 0 {
            String::new()
        } else {
            status.to_string()
        };
        self.metrics.http_request_duration_seconds.observe(
            &[
                normalized_http_method(method),
                scheme,
                route,
                &status,
                if cancelled { "canceled" } else { "other" },
            ],
            duration,
        );
    }
    fn role_readiness(&self, ready: bool) {
        self.metrics
            .role_ready
            .set(&[], if ready { 1.0 } else { 0.0 });
    }
    fn db_pool(&self, pool: &'static str, idle: u32, used: u32, max: Option<u32>) {
        self.metrics
            .pool_connections
            .set(&[pool, "idle"], f64::from(idle));
        self.metrics
            .pool_connections
            .set(&[pool, "used"], f64::from(used));
        if let Some(max) = max {
            self.metrics.pool_max.set(&[pool], f64::from(max));
        }
    }
    fn task_handler_active(&self, kind: &'static str, delta: i64) {
        self.metrics.task_handler_active.add(&[kind], delta);
    }
    fn task_handler_finished(&self, kind: &'static str, result: &'static str, seconds: f64) {
        self.metrics
            .task_handler_duration
            .observe(&[kind, result], seconds);
    }
    fn http_active(&self, method: &str, scheme: &str, delta: i64) {
        self.metrics
            .http_active
            .add(&[normalized_http_method(method), scheme], delta);
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
    let system = Arc::new(parking_lot::Mutex::new(None));
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { biased; () = shutdown.cancelled() => break, _ = interval.tick() => {} }
        if !recorder.provider.enabled() {
            break;
        }
        let system = system.clone();
        let sample_recorder = recorder.clone();
        let task = tokio::task::spawn_blocking(move || {
            sample_system_metrics(&sample_recorder, &mut system.lock(), sysinfo::System::new);
        });
        if task.await.is_err() {
            recorder.provider.record_collection_failure();
            tracing::warn!("system metrics sampler failed");
        }
    }
}

fn sample_system_metrics(
    recorder: &InfrastructureMetricsRecorder,
    system: &mut Option<sysinfo::System>,
    initialize: impl FnOnce() -> Result<sysinfo::System, sysinfo::Error>,
) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate};

    if system.is_none() {
        match initialize() {
            Ok(initialized) => *system = Some(initialized),
            Err(error) => {
                recorder.provider.record_collection_failure();
                tracing::warn!(%error, "system metrics sampler initialization failed; retrying next interval");
            }
        }
    }
    if let Some(system) = system {
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
                .process_cpu_time
                .set(&[], metric_u64(process.accumulated_cpu_time()) / 1000.0);
        }
    }
    recorder.metrics.observation_failures.set(
        &["metrics", "record"],
        metric_u64(recorder.provider.observation_failures()),
    );
    recorder.metrics.observation_failures.set(
        &["metrics", "collect"],
        metric_u64(recorder.provider.collection_failures()),
    );
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
            .set(&["allocated"], allocated * 1024.0 * 1024.0);
        recorder
            .metrics
            .process_heap_memory_mib
            .set(&["peak_or_resident"], resident * 1024.0 * 1024.0);
    }
}

#[cfg(all(test, feature = "backend-prometheus"))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn system_initialization_retries_without_blocking_other_samples() {
        let provider = MetricsProvider::builder()
            .with_prometheus()
            .unwrap()
            .build()
            .unwrap();
        let recorder = InfrastructureMetricsRecorder::new(provider.clone()).unwrap();
        let mut system = None;
        let mut initialization_attempts = 0;

        for error in [
            sysinfo::Error::Unsupported,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied).into(),
        ] {
            sample_system_metrics(&recorder, &mut system, || {
                initialization_attempts += 1;
                Err(error)
            });
            assert!(system.is_none());
            let output = provider
                .export_prometheus(Duration::from_secs(1))
                .await
                .unwrap();
            assert!(sample_value(&output, "process_uptime_seconds") > 0.0);
            assert!(
                !output
                    .lines()
                    .any(|line| line.starts_with("process_memory_usage_bytes "))
            );
            assert!(
                !output
                    .lines()
                    .any(|line| line.starts_with("process_cpu_time_seconds_total "))
            );
            #[cfg(feature = "allocator-metrics")]
            assert!(output.contains("aster_process_heap_bytes{kind=\"allocated\"}"));
        }

        sample_system_metrics(&recorder, &mut system, || {
            initialization_attempts += 1;
            sysinfo::System::new()
        });
        assert!(system.is_some());
        sample_system_metrics(&recorder, &mut system, || {
            initialization_attempts += 1;
            Err(sysinfo::Error::Unsupported)
        });
        assert_eq!(initialization_attempts, 3, "reuse the initialized system");

        let output = provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap();
        assert!(sample_value(&output, "process_memory_usage_bytes") > 0.0);
        assert!(sample_value(&output, "process_cpu_time_seconds_total") >= 0.0);
        provider.shutdown(Duration::from_secs(1)).await.unwrap();
    }

    fn sample_value(output: &str, name: &str) -> f64 {
        output
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
            .unwrap_or_else(|| panic!("missing sample {name}: {output}"))
            .parse()
            .unwrap()
    }
}

/// Finite HTTP method vocabulary shared by adapters and the recorder.
#[must_use]
pub fn normalized_http_method(method: &str) -> &'static str {
    match method {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "CONNECT" => "CONNECT",
        "OPTIONS" => "OPTIONS",
        "TRACE" => "TRACE",
        _ => "_OTHER",
    }
}

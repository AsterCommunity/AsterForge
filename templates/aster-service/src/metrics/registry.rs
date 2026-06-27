//! Prometheus registry and low-cardinality infrastructure metric families.

use std::sync::OnceLock;
use std::time::Instant;

use aster_forge_runtime::HealthStatus;
use prometheus::{
    Encoder, Gauge, GaugeVec, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};

static METRICS: OnceLock<Metrics> = OnceLock::new();
pub(crate) static PROCESS_STARTED_AT: OnceLock<Instant> = OnceLock::new();

/// Prometheus metric families owned by the generated service.
pub struct Metrics {
    registry: Registry,
    http_requests_total: IntCounterVec,
    http_request_duration_seconds: HistogramVec,
    db_queries_total: IntCounterVec,
    db_query_duration_seconds: HistogramVec,
    auth_events_total: IntCounterVec,
    application_events_total: IntCounterVec,
    background_tasks_total: IntCounterVec,
    background_tasks_pending: IntGauge,
    background_task_retries_total: IntCounterVec,
    external_operations_total: IntCounterVec,
    external_operation_duration_seconds: HistogramVec,
    health_report_status: GaugeVec,
    health_report_duration_seconds: HistogramVec,
    health_component_status: GaugeVec,
    health_component_duration_seconds: HistogramVec,
    pub(crate) process_memory_rss_bytes: Gauge,
    pub(crate) process_cpu_milliseconds_total: IntGauge,
    pub(crate) uptime_seconds: Gauge,
    process_heap_allocated_mib: Gauge,
    process_heap_peak_or_resident_mib: Gauge,
}

impl Metrics {
    fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let http_requests_total = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["method", "route", "status"],
        )?;
        let http_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0]),
            &["method", "route", "status"],
        )?;
        let db_queries_total = IntCounterVec::new(
            Opts::new(
                "db_queries_total",
                "Total database queries observed through the shared database metrics adapter",
            ),
            &["backend", "kind", "status"],
        )?;
        let db_query_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "db_query_duration_seconds",
                "Database query duration in seconds",
            )
            .buckets(vec![
                0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]),
            &["backend", "kind", "status"],
        )?;
        let auth_events_total = IntCounterVec::new(
            Opts::new("auth_events_total", "Total authentication events"),
            &["action", "status", "reason"],
        )?;
        let application_events_total = IntCounterVec::new(
            Opts::new(
                "application_events_total",
                "Total low-cardinality application events",
            ),
            &["category", "event", "status"],
        )?;
        let background_tasks_total = IntCounterVec::new(
            Opts::new(
                "background_tasks_total",
                "Total background task state transitions",
            ),
            &["kind", "status"],
        )?;
        let background_tasks_pending = IntGauge::new(
            "background_tasks_pending",
            "Pending or retryable background task backlog",
        )?;
        let background_task_retries_total = IntCounterVec::new(
            Opts::new(
                "background_task_retries_total",
                "Total background task retry transitions",
            ),
            &["kind"],
        )?;
        let external_operations_total = IntCounterVec::new(
            Opts::new(
                "external_operations_total",
                "Total operations against external systems",
            ),
            &["system", "operation", "status"],
        )?;
        let external_operation_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "external_operation_duration_seconds",
                "External system operation duration in seconds",
            )
            .buckets(vec![
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 15.0, 60.0,
            ]),
            &["system", "operation", "status"],
        )?;
        let health_report_status = GaugeVec::new(
            Opts::new(
                "health_report_status",
                "Aggregate health status for a health check scope: healthy=0, degraded=1, unhealthy=2",
            ),
            &["scope"],
        )?;
        let health_report_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "health_report_duration_seconds",
                "Aggregate health check duration in seconds",
            )
            .buckets(vec![
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]),
            &["scope", "status"],
        )?;
        let health_component_status = GaugeVec::new(
            Opts::new(
                "health_component_status",
                "Health component status for a health check scope: healthy=0, degraded=1, unhealthy=2",
            ),
            &["scope", "component"],
        )?;
        let health_component_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "health_component_duration_seconds",
                "Health component check duration in seconds",
            )
            .buckets(vec![
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]),
            &["scope", "component", "status"],
        )?;
        let process_memory_rss_bytes =
            Gauge::new("process_memory_rss_bytes", "Process RSS memory in bytes")?;
        let process_cpu_milliseconds_total = IntGauge::new(
            "process_cpu_milliseconds_total",
            "Process accumulated CPU time in milliseconds",
        )?;
        let uptime_seconds = Gauge::new("process_uptime_seconds", "Process uptime in seconds")?;
        let process_heap_allocated_mib = Gauge::new(
            "process_heap_allocated_mib",
            "Current heap allocation reported by the configured allocator in MiB",
        )?;
        let process_heap_peak_or_resident_mib = Gauge::new(
            "process_heap_peak_or_resident_mib",
            "Peak tracked heap allocation or jemalloc resident memory in MiB",
        )?;

        for collector in [
            Box::new(http_requests_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(http_request_duration_seconds.clone()),
            Box::new(db_queries_total.clone()),
            Box::new(db_query_duration_seconds.clone()),
            Box::new(auth_events_total.clone()),
            Box::new(application_events_total.clone()),
            Box::new(background_tasks_total.clone()),
            Box::new(background_tasks_pending.clone()),
            Box::new(background_task_retries_total.clone()),
            Box::new(external_operations_total.clone()),
            Box::new(external_operation_duration_seconds.clone()),
            Box::new(health_report_status.clone()),
            Box::new(health_report_duration_seconds.clone()),
            Box::new(health_component_status.clone()),
            Box::new(health_component_duration_seconds.clone()),
            Box::new(process_memory_rss_bytes.clone()),
            Box::new(process_cpu_milliseconds_total.clone()),
            Box::new(uptime_seconds.clone()),
            Box::new(process_heap_allocated_mib.clone()),
            Box::new(process_heap_peak_or_resident_mib.clone()),
        ] {
            registry.register(collector)?;
        }

        Ok(Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            db_queries_total,
            db_query_duration_seconds,
            auth_events_total,
            application_events_total,
            background_tasks_total,
            background_tasks_pending,
            background_task_retries_total,
            external_operations_total,
            external_operation_duration_seconds,
            health_report_status,
            health_report_duration_seconds,
            health_component_status,
            health_component_duration_seconds,
            process_memory_rss_bytes,
            process_cpu_milliseconds_total,
            uptime_seconds,
            process_heap_allocated_mib,
            process_heap_peak_or_resident_mib,
        })
    }

    fn export(&self) -> Result<String, String> {
        self.refresh_allocator_stats();
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buf = Vec::new();
        encoder
            .encode(&metric_families, &mut buf)
            .map_err(|error| error.to_string())?;
        String::from_utf8(buf).map_err(|error| error.to_string())
    }

    fn refresh_allocator_stats(&self) {
        let (allocated, peak_or_resident) = aster_forge_alloc::stats();
        self.process_heap_allocated_mib.set(allocated);
        self.process_heap_peak_or_resident_mib.set(peak_or_resident);
    }
}

/// Initializes the Prometheus registry.
pub fn init_metrics() -> Result<(), prometheus::Error> {
    if METRICS.get().is_some() {
        return Ok(());
    }

    let _ = PROCESS_STARTED_AT.get_or_init(Instant::now);
    let metrics = Metrics::new()?;
    let _ = METRICS.set(metrics);
    Ok(())
}

pub fn get_metrics() -> Option<&'static Metrics> {
    METRICS.get()
}

/// Exports the current Prometheus text exposition body.
pub fn export_metrics() -> Result<String, String> {
    let metrics = get_metrics().ok_or_else(|| "metrics registry is not initialized".to_string())?;
    metrics.export()
}

pub fn record_http_request(method: &str, route: &str, status: u16, duration_seconds: f64) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    let status = status.to_string();
    metrics
        .http_requests_total
        .with_label_values(&[method, route, &status])
        .inc();
    metrics
        .http_request_duration_seconds
        .with_label_values(&[method, route, &status])
        .observe(duration_seconds);
}

pub fn record_db_query(metric: &aster_forge_metrics::DbQueryMetric) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    let backend = metric.backend.as_label();
    let kind = metric.kind.as_label();
    let status = metric.status_label();
    metrics
        .db_queries_total
        .with_label_values(&[backend, kind, status])
        .inc();
    metrics
        .db_query_duration_seconds
        .with_label_values(&[backend, kind, status])
        .observe(metric.elapsed.as_secs_f64());
}

pub fn record_auth_event(action: &'static str, status: &'static str, reason: &'static str) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    metrics
        .auth_events_total
        .with_label_values(&[action, status, reason])
        .inc();
}

pub fn record_application_event(category: &'static str, event: &'static str, status: &'static str) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    metrics
        .application_events_total
        .with_label_values(&[category, event, status])
        .inc();
}

pub fn record_background_task_transition(kind: &'static str, status: &'static str) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    metrics
        .background_tasks_total
        .with_label_values(&[kind, status])
        .inc();
    if status == "retry" {
        metrics
            .background_task_retries_total
            .with_label_values(&[kind])
            .inc();
    }
}

pub fn set_background_tasks_pending(pending: u64) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    metrics
        .background_tasks_pending
        .set(i64::try_from(pending).unwrap_or(i64::MAX));
}

pub fn record_external_operation(
    system: &'static str,
    operation: &'static str,
    status: &'static str,
    duration_seconds: f64,
) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    metrics
        .external_operations_total
        .with_label_values(&[system, operation, status])
        .inc();
    metrics
        .external_operation_duration_seconds
        .with_label_values(&[system, operation, status])
        .observe(duration_seconds);
}

pub fn record_health_report(scope: &'static str, status: HealthStatus, duration_seconds: f64) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    metrics
        .health_report_status
        .with_label_values(&[scope])
        .set(health_status_value(status));
    metrics
        .health_report_duration_seconds
        .with_label_values(&[scope, status.as_str()])
        .observe(duration_seconds);
}

pub fn record_health_component(
    scope: &'static str,
    component: &'static str,
    status: HealthStatus,
    duration_seconds: f64,
) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    metrics
        .health_component_status
        .with_label_values(&[scope, component])
        .set(health_status_value(status));
    metrics
        .health_component_duration_seconds
        .with_label_values(&[scope, component, status.as_str()])
        .observe(duration_seconds);
}

const fn health_status_value(status: HealthStatus) -> f64 {
    match status {
        HealthStatus::Healthy => 0.0,
        HealthStatus::Degraded => 1.0,
        HealthStatus::Unhealthy => 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HealthStatus, get_metrics, init_metrics, record_health_component, record_health_report,
    };

    #[test]
    fn health_metrics_are_exported_with_low_cardinality_labels() {
        init_metrics().expect("metrics registry should initialize");

        record_health_report("diagnostics", HealthStatus::Degraded, 0.25);
        record_health_component("diagnostics", "cache", HealthStatus::Degraded, 0.05);

        let body = get_metrics()
            .expect("metrics registry should be initialized")
            .export()
            .expect("metrics should export");

        assert!(body.contains("health_report_status"));
        assert!(body.contains("health_report_duration_seconds_count"));
        assert!(body.contains("health_component_status"));
        assert!(body.contains("health_component_duration_seconds_count"));
        assert!(body.contains("scope=\"diagnostics\""));
        assert!(body.contains("component=\"cache\""));
        assert!(body.contains("status=\"degraded\""));
    }
}

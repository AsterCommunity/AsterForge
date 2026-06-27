//! Prometheus registry and low-cardinality infrastructure metric families.

use std::sync::OnceLock;

use prometheus::{
    Encoder, Gauge, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder,
};

static METRICS: OnceLock<Metrics> = OnceLock::new();

/// Prometheus metric families owned by the generated service.
pub struct Metrics {
    registry: Registry,
    http_requests_total: IntCounterVec,
    http_request_duration_seconds: HistogramVec,
    db_queries_total: IntCounterVec,
    db_query_duration_seconds: HistogramVec,
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

    let metrics = Metrics::new()?;
    let _ = METRICS.set(metrics);
    Ok(())
}

/// Exports the current Prometheus text exposition body.
pub fn export_metrics() -> Result<String, String> {
    let metrics = METRICS
        .get()
        .ok_or_else(|| "metrics registry is not initialized".to_string())?;
    metrics.export()
}

pub fn record_http_request(method: &str, route: &str, status: u16, duration_seconds: f64) {
    let Some(metrics) = METRICS.get() else {
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
    let Some(metrics) = METRICS.get() else {
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

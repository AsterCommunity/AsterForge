//! Prometheus backend for shared Aster infrastructure metrics.
//!
//! This module is enabled by the `prometheus` feature. Product crates can use
//! [`init_or_noop`] to obtain a [`SharedMetricsRecorder`](crate::SharedMetricsRecorder)
//! and [`export_metrics`] for their HTTP metrics endpoint without depending on
//! the `prometheus` crate directly. Product-specific metric families can be
//! registered with Forge descriptors and recorded through opaque handles, so
//! products keep ownership of domain labels without importing Prometheus types.

use crate::{
    DbMetricsRecorder, DbQueryMetric, MetricDescriptor, MetricKind, MetricsRecorder,
    SharedMetricsRecorder,
};
use prometheus::{
    Encoder, Gauge, GaugeVec, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

static METRICS: OnceLock<PrometheusMetrics> = OnceLock::new();
static PROCESS_STARTED_AT: OnceLock<Instant> = OnceLock::new();

fn boxed_collector<C>(collector: C) -> Box<dyn prometheus::core::Collector>
where
    C: prometheus::core::Collector + 'static,
{
    Box::new(collector)
}

/// Prometheus metric families for product-neutral Aster infrastructure.
pub struct PrometheusMetrics {
    registry: Registry,
    http_requests_total: IntCounterVec,
    http_request_duration_seconds: HistogramVec,
    db_queries_total: IntCounterVec,
    db_query_duration_seconds: HistogramVec,
    auth_events_total: IntCounterVec,
    application_events_total: IntCounterVec,
    config_reloads_total: IntCounterVec,
    config_reload_duration_seconds: HistogramVec,
    config_reload_changed_keys: HistogramVec,
    config_mutations_total: IntCounterVec,
    config_mutation_changed_keys: HistogramVec,
    background_tasks_total: IntCounterVec,
    background_tasks_pending: IntGauge,
    background_task_retries_total: IntCounterVec,
    external_operations_total: IntCounterVec,
    external_operation_duration_seconds: HistogramVec,
    health_report_status: GaugeVec,
    health_report_duration_seconds: HistogramVec,
    health_component_status: GaugeVec,
    health_component_duration_seconds: HistogramVec,
    process_memory_rss_bytes: Gauge,
    process_cpu_milliseconds_total: IntGauge,
    uptime_seconds: Gauge,
    #[cfg(feature = "allocator-metrics")]
    process_heap_memory_mib: GaugeVec,
    product_metrics: Mutex<ProductMetricRegistry>,
}

impl PrometheusMetrics {
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
        let config_reloads_total = IntCounterVec::new(
            Opts::new(
                "config_reloads_total",
                "Total runtime config reload attempts",
            ),
            &["source", "decision", "status"],
        )?;
        let config_reload_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "config_reload_duration_seconds",
                "Runtime config reload duration in seconds",
            )
            .buckets(vec![
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0,
            ]),
            &["source", "decision", "status"],
        )?;
        let config_reload_changed_keys = HistogramVec::new(
            HistogramOpts::new(
                "config_reload_changed_keys",
                "Number of changed keys observed by runtime config reload attempts",
            )
            .buckets(vec![0.0, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 500.0]),
            &["source", "decision", "status"],
        )?;
        let config_mutations_total = IntCounterVec::new(
            Opts::new(
                "config_mutations_total",
                "Total runtime config mutation attempts",
            ),
            &["source", "operation", "status"],
        )?;
        let config_mutation_changed_keys = HistogramVec::new(
            HistogramOpts::new(
                "config_mutation_changed_keys",
                "Number of changed keys in runtime config mutation attempts",
            )
            .buckets(vec![0.0, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0]),
            &["source", "operation", "status"],
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
        #[cfg(feature = "allocator-metrics")]
        let process_heap_memory_mib = GaugeVec::new(
            Opts::new(
                "process_heap_memory_mib",
                "Allocator heap memory in MiB: allocated and peak_or_resident",
            ),
            &["kind"],
        )?;

        let collectors: Vec<Box<dyn prometheus::core::Collector>> = vec![
            boxed_collector(http_requests_total.clone()),
            boxed_collector(http_request_duration_seconds.clone()),
            boxed_collector(db_queries_total.clone()),
            boxed_collector(db_query_duration_seconds.clone()),
            boxed_collector(auth_events_total.clone()),
            boxed_collector(application_events_total.clone()),
            boxed_collector(config_reloads_total.clone()),
            boxed_collector(config_reload_duration_seconds.clone()),
            boxed_collector(config_reload_changed_keys.clone()),
            boxed_collector(config_mutations_total.clone()),
            boxed_collector(config_mutation_changed_keys.clone()),
            boxed_collector(background_tasks_total.clone()),
            boxed_collector(background_tasks_pending.clone()),
            boxed_collector(background_task_retries_total.clone()),
            boxed_collector(external_operations_total.clone()),
            boxed_collector(external_operation_duration_seconds.clone()),
            boxed_collector(health_report_status.clone()),
            boxed_collector(health_report_duration_seconds.clone()),
            boxed_collector(health_component_status.clone()),
            boxed_collector(health_component_duration_seconds.clone()),
            boxed_collector(process_memory_rss_bytes.clone()),
            boxed_collector(process_cpu_milliseconds_total.clone()),
            boxed_collector(uptime_seconds.clone()),
            #[cfg(feature = "allocator-metrics")]
            boxed_collector(process_heap_memory_mib.clone()),
        ];

        for collector in collectors {
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
            config_reloads_total,
            config_reload_duration_seconds,
            config_reload_changed_keys,
            config_mutations_total,
            config_mutation_changed_keys,
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
            #[cfg(feature = "allocator-metrics")]
            process_heap_memory_mib,
            product_metrics: Mutex::new(ProductMetricRegistry::default()),
        })
    }

    fn export(&self) -> Result<String, String> {
        self.refresh_allocator_metrics();
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buf = Vec::new();
        encoder
            .encode(&metric_families, &mut buf)
            .map_err(|error| error.to_string())?;
        String::from_utf8(buf).map_err(|error| error.to_string())
    }

    #[cfg(feature = "allocator-metrics")]
    fn refresh_allocator_metrics(&self) {
        let (allocated_mib, peak_or_resident_mib) = aster_forge_alloc::stats();
        self.process_heap_memory_mib
            .with_label_values(&["allocated"])
            .set(allocated_mib);
        self.process_heap_memory_mib
            .with_label_values(&["peak_or_resident"])
            .set(peak_or_resident_mib);
    }

    #[cfg(not(feature = "allocator-metrics"))]
    fn refresh_allocator_metrics(&self) {}
}

#[derive(Default)]
struct ProductMetricRegistry {
    collectors: BTreeMap<ProductMetricKey, ProductMetricCollector>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProductMetricKey {
    subsystem: &'static str,
    name: &'static str,
}

#[derive(Clone)]
enum ProductMetricCollector {
    Counter {
        labels: &'static [&'static str],
        collector: IntCounterVec,
    },
    Gauge {
        labels: &'static [&'static str],
        collector: GaugeVec,
    },
    Histogram {
        labels: &'static [&'static str],
        collector: HistogramVec,
    },
}

impl ProductMetricCollector {
    fn kind(&self) -> MetricKind {
        match self {
            Self::Counter { .. } => MetricKind::Counter,
            Self::Gauge { .. } => MetricKind::Gauge,
            Self::Histogram { .. } => MetricKind::Histogram,
        }
    }

    fn label_count(&self) -> usize {
        match self {
            Self::Counter { labels, .. }
            | Self::Gauge { labels, .. }
            | Self::Histogram { labels, .. } => labels.len(),
        }
    }
}

/// Opaque handle returned after registering a product metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductMetricHandle {
    subsystem: &'static str,
    name: &'static str,
    kind: MetricKind,
    label_count: usize,
}

impl ProductMetricHandle {
    /// Creates a handle for a metric descriptor.
    const fn new(descriptor: &MetricDescriptor) -> Self {
        Self {
            subsystem: descriptor.subsystem,
            name: descriptor.name,
            kind: descriptor.kind,
            label_count: descriptor.labels.len(),
        }
    }

    /// Returns the subsystem that owns this metric.
    pub const fn subsystem(&self) -> &'static str {
        self.subsystem
    }

    /// Returns the descriptor-local metric name.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the registered metric kind.
    pub const fn kind(&self) -> MetricKind {
        self.kind
    }

    /// Returns the number of label values required by this metric.
    pub const fn label_count(&self) -> usize {
        self.label_count
    }
}

/// Typed handle for a registered product counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductCounter {
    handle: ProductMetricHandle,
}

impl ProductCounter {
    /// Creates a typed counter from an opaque product metric handle.
    pub fn from_handle(handle: ProductMetricHandle) -> ProductMetricResult<Self> {
        typed_product_metric_handle(handle, MetricKind::Counter).map(|handle| Self { handle })
    }

    /// Returns the underlying opaque handle.
    pub const fn handle(&self) -> ProductMetricHandle {
        self.handle
    }

    /// Increments this counter and logs recording failures.
    pub fn inc(&self, label_values: &[&str], value: u64) {
        if let Err(error) = self.try_inc(label_values, value) {
            log_product_metric_error(error);
        }
    }

    /// Increments this counter and returns recording failures.
    pub fn try_inc(&self, label_values: &[&str], value: u64) -> ProductMetricResult<()> {
        inc_product_counter(self.handle, label_values, value)
    }
}

/// Typed handle for a registered product gauge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductGauge {
    handle: ProductMetricHandle,
}

impl ProductGauge {
    /// Creates a typed gauge from an opaque product metric handle.
    pub fn from_handle(handle: ProductMetricHandle) -> ProductMetricResult<Self> {
        typed_product_metric_handle(handle, MetricKind::Gauge).map(|handle| Self { handle })
    }

    /// Returns the underlying opaque handle.
    pub const fn handle(&self) -> ProductMetricHandle {
        self.handle
    }

    /// Sets this gauge and logs recording failures.
    pub fn set(&self, label_values: &[&str], value: f64) {
        if let Err(error) = self.try_set(label_values, value) {
            log_product_metric_error(error);
        }
    }

    /// Sets this gauge and returns recording failures.
    pub fn try_set(&self, label_values: &[&str], value: f64) -> ProductMetricResult<()> {
        set_product_gauge(self.handle, label_values, value)
    }

    /// Adds to this gauge and logs recording failures.
    pub fn add(&self, label_values: &[&str], value: f64) {
        if let Err(error) = self.try_add(label_values, value) {
            log_product_metric_error(error);
        }
    }

    /// Adds to this gauge and returns recording failures.
    pub fn try_add(&self, label_values: &[&str], value: f64) -> ProductMetricResult<()> {
        add_product_gauge(self.handle, label_values, value)
    }
}

/// Typed handle for a registered product histogram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductHistogram {
    handle: ProductMetricHandle,
}

impl ProductHistogram {
    /// Creates a typed histogram from an opaque product metric handle.
    pub fn from_handle(handle: ProductMetricHandle) -> ProductMetricResult<Self> {
        typed_product_metric_handle(handle, MetricKind::Histogram).map(|handle| Self { handle })
    }

    /// Returns the underlying opaque handle.
    pub const fn handle(&self) -> ProductMetricHandle {
        self.handle
    }

    /// Observes a value in this histogram and logs recording failures.
    pub fn observe(&self, label_values: &[&str], value: f64) {
        if let Err(error) = self.try_observe(label_values, value) {
            log_product_metric_error(error);
        }
    }

    /// Observes a value in this histogram and returns recording failures.
    pub fn try_observe(&self, label_values: &[&str], value: f64) -> ProductMetricResult<()> {
        observe_product_histogram(self.handle, label_values, value)
    }
}

fn log_product_metric_error(error: ProductMetricError) {
    tracing::warn!(error = %error, "failed to record product metric");
}

fn typed_product_metric_handle(
    handle: ProductMetricHandle,
    expected_kind: MetricKind,
) -> ProductMetricResult<ProductMetricHandle> {
    if handle.kind != expected_kind {
        return Err(ProductMetricError::WrongKind {
            subsystem: handle.subsystem,
            name: handle.name,
            actual: handle.kind,
            expected: expected_kind,
        });
    }

    Ok(handle)
}

/// Errors returned by product metric registration or recording.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProductMetricError {
    /// The Prometheus registry has not been initialized.
    #[error("metrics registry is not initialized")]
    NotInitialized,
    /// A product metric with the same subsystem/name was already registered.
    #[error("duplicate product metric registration: {subsystem}.{name}")]
    DuplicateRegistration {
        /// Owning subsystem.
        subsystem: &'static str,
        /// Metric name.
        name: &'static str,
    },
    /// The handle does not point to a registered metric.
    #[error("unknown product metric: {subsystem}.{name}")]
    UnknownMetric {
        /// Owning subsystem.
        subsystem: &'static str,
        /// Metric name.
        name: &'static str,
    },
    /// The recording operation does not match the registered metric kind.
    #[error("product metric {subsystem}.{name} has kind {actual:?}, expected {expected:?}")]
    WrongKind {
        /// Owning subsystem.
        subsystem: &'static str,
        /// Metric name.
        name: &'static str,
        /// Registered kind.
        actual: MetricKind,
        /// Expected kind.
        expected: MetricKind,
    },
    /// The number of label values did not match the descriptor.
    #[error(
        "product metric {subsystem}.{name} expected {expected} label values, received {actual}"
    )]
    LabelCountMismatch {
        /// Owning subsystem.
        subsystem: &'static str,
        /// Metric name.
        name: &'static str,
        /// Expected label value count.
        expected: usize,
        /// Actual label value count.
        actual: usize,
    },
    /// Prometheus rejected the metric collector.
    #[error("prometheus metric error: {0}")]
    Prometheus(String),
    /// The product metric registry lock was poisoned.
    #[error("product metric registry lock is poisoned")]
    LockPoisoned,
}

/// Result type for product metric operations.
pub type ProductMetricResult<T> = std::result::Result<T, ProductMetricError>;

/// Initializes the shared Prometheus registry.
pub fn init_metrics() -> Result<(), prometheus::Error> {
    if METRICS.get().is_some() {
        return Ok(());
    }

    let _ = PROCESS_STARTED_AT.get_or_init(Instant::now);
    let metrics = PrometheusMetrics::new()?;
    let _ = METRICS.set(metrics);
    Ok(())
}

/// Initializes Prometheus metrics and falls back to a no-op recorder on failure.
pub fn init_or_noop() -> SharedMetricsRecorder {
    crate::init_metrics_or_noop(init_metrics, || PrometheusMetricsRecorder)
}

/// Returns whether the Prometheus registry has been initialized.
pub fn is_initialized() -> bool {
    METRICS.get().is_some()
}

/// Exports the current Prometheus text exposition body.
pub fn export_metrics() -> Result<String, String> {
    let metrics = METRICS
        .get()
        .ok_or_else(|| "metrics registry is not initialized".to_string())?;
    metrics.export()
}

/// Registers a product-owned metric family in the shared Prometheus registry.
///
/// Product crates pass a Forge [`MetricDescriptor`] and receive an opaque handle
/// for future record calls. This keeps product code independent from the
/// `prometheus` crate while still allowing product-specific metric families.
pub fn register_product_metric(
    descriptor: MetricDescriptor,
) -> ProductMetricResult<ProductMetricHandle> {
    let metrics = METRICS.get().ok_or(ProductMetricError::NotInitialized)?;
    let key = ProductMetricKey {
        subsystem: descriptor.subsystem,
        name: descriptor.name,
    };
    let handle = ProductMetricHandle::new(&descriptor);
    let collector = build_product_metric_collector(&descriptor)?;
    let mut product_metrics = metrics
        .product_metrics
        .lock()
        .map_err(|_| ProductMetricError::LockPoisoned)?;
    if product_metrics.collectors.contains_key(&key) {
        return Err(ProductMetricError::DuplicateRegistration {
            subsystem: descriptor.subsystem,
            name: descriptor.name,
        });
    }
    metrics
        .registry
        .register(product_collector_box(&collector))
        .map_err(|error| ProductMetricError::Prometheus(error.to_string()))?;
    product_metrics.collectors.insert(key, collector);
    Ok(handle)
}

/// Registers multiple product-owned metric families in order.
pub fn register_product_metrics<I>(descriptors: I) -> ProductMetricResult<Vec<ProductMetricHandle>>
where
    I: IntoIterator<Item = MetricDescriptor>,
{
    descriptors
        .into_iter()
        .map(register_product_metric)
        .collect()
}

/// Registers a product counter and returns a typed handle.
pub fn register_product_counter(
    descriptor: MetricDescriptor,
) -> ProductMetricResult<ProductCounter> {
    ensure_descriptor_kind(&descriptor, MetricKind::Counter)?;
    ProductCounter::from_handle(register_product_metric(descriptor)?)
}

/// Registers a product gauge and returns a typed handle.
pub fn register_product_gauge(descriptor: MetricDescriptor) -> ProductMetricResult<ProductGauge> {
    ensure_descriptor_kind(&descriptor, MetricKind::Gauge)?;
    ProductGauge::from_handle(register_product_metric(descriptor)?)
}

/// Registers a product histogram and returns a typed handle.
pub fn register_product_histogram(
    descriptor: MetricDescriptor,
) -> ProductMetricResult<ProductHistogram> {
    ensure_descriptor_kind(&descriptor, MetricKind::Histogram)?;
    ProductHistogram::from_handle(register_product_metric(descriptor)?)
}

fn ensure_descriptor_kind(
    descriptor: &MetricDescriptor,
    expected_kind: MetricKind,
) -> ProductMetricResult<()> {
    if descriptor.kind != expected_kind {
        return Err(ProductMetricError::WrongKind {
            subsystem: descriptor.subsystem,
            name: descriptor.name,
            actual: descriptor.kind,
            expected: expected_kind,
        });
    }

    Ok(())
}

fn build_product_metric_collector(
    descriptor: &MetricDescriptor,
) -> ProductMetricResult<ProductMetricCollector> {
    match descriptor.kind {
        MetricKind::Counter => {
            let metric_name = product_metric_name(descriptor);
            let collector =
                IntCounterVec::new(Opts::new(metric_name, descriptor.help), descriptor.labels)
                    .map_err(|error| ProductMetricError::Prometheus(error.to_string()))?;
            Ok(ProductMetricCollector::Counter {
                labels: descriptor.labels,
                collector,
            })
        }
        MetricKind::Gauge => {
            let metric_name = product_metric_name(descriptor);
            let collector =
                GaugeVec::new(Opts::new(metric_name, descriptor.help), descriptor.labels)
                    .map_err(|error| ProductMetricError::Prometheus(error.to_string()))?;
            Ok(ProductMetricCollector::Gauge {
                labels: descriptor.labels,
                collector,
            })
        }
        MetricKind::Histogram => {
            let metric_name = product_metric_name(descriptor);
            let mut opts = HistogramOpts::new(metric_name, descriptor.help);
            if !descriptor.buckets.is_empty() {
                opts = opts.buckets(descriptor.buckets.to_vec());
            }
            let collector = HistogramVec::new(opts, descriptor.labels)
                .map_err(|error| ProductMetricError::Prometheus(error.to_string()))?;
            Ok(ProductMetricCollector::Histogram {
                labels: descriptor.labels,
                collector,
            })
        }
    }
}

fn product_metric_name(descriptor: &MetricDescriptor) -> String {
    format!("{}_{}", descriptor.subsystem, descriptor.name)
}

fn product_collector_box(
    collector: &ProductMetricCollector,
) -> Box<dyn prometheus::core::Collector> {
    match collector {
        ProductMetricCollector::Counter { collector, .. } => Box::new(collector.clone()),
        ProductMetricCollector::Gauge { collector, .. } => Box::new(collector.clone()),
        ProductMetricCollector::Histogram { collector, .. } => Box::new(collector.clone()),
    }
}

/// Increments a registered product counter.
pub fn inc_product_counter(
    handle: ProductMetricHandle,
    label_values: &[&str],
    value: u64,
) -> ProductMetricResult<()> {
    with_product_metric(handle, MetricKind::Counter, label_values, |collector| {
        let ProductMetricCollector::Counter { collector, .. } = collector else {
            return;
        };
        collector.with_label_values(label_values).inc_by(value);
    })
}

/// Sets a registered product gauge.
pub fn set_product_gauge(
    handle: ProductMetricHandle,
    label_values: &[&str],
    value: f64,
) -> ProductMetricResult<()> {
    with_product_metric(handle, MetricKind::Gauge, label_values, |collector| {
        let ProductMetricCollector::Gauge { collector, .. } = collector else {
            return;
        };
        collector.with_label_values(label_values).set(value);
    })
}

/// Adds to a registered product gauge.
pub fn add_product_gauge(
    handle: ProductMetricHandle,
    label_values: &[&str],
    value: f64,
) -> ProductMetricResult<()> {
    with_product_metric(handle, MetricKind::Gauge, label_values, |collector| {
        let ProductMetricCollector::Gauge { collector, .. } = collector else {
            return;
        };
        collector.with_label_values(label_values).add(value);
    })
}

/// Observes a registered product histogram.
pub fn observe_product_histogram(
    handle: ProductMetricHandle,
    label_values: &[&str],
    value: f64,
) -> ProductMetricResult<()> {
    with_product_metric(handle, MetricKind::Histogram, label_values, |collector| {
        let ProductMetricCollector::Histogram { collector, .. } = collector else {
            return;
        };
        collector.with_label_values(label_values).observe(value);
    })
}

fn with_product_metric<F>(
    handle: ProductMetricHandle,
    expected_kind: MetricKind,
    label_values: &[&str],
    record: F,
) -> ProductMetricResult<()>
where
    F: FnOnce(&ProductMetricCollector),
{
    let metrics = METRICS.get().ok_or(ProductMetricError::NotInitialized)?;
    let product_metrics = metrics
        .product_metrics
        .lock()
        .map_err(|_| ProductMetricError::LockPoisoned)?;
    let key = ProductMetricKey {
        subsystem: handle.subsystem,
        name: handle.name,
    };
    let collector =
        product_metrics
            .collectors
            .get(&key)
            .ok_or(ProductMetricError::UnknownMetric {
                subsystem: handle.subsystem,
                name: handle.name,
            })?;
    if collector.kind() != expected_kind || handle.kind != expected_kind {
        return Err(ProductMetricError::WrongKind {
            subsystem: handle.subsystem,
            name: handle.name,
            actual: collector.kind(),
            expected: expected_kind,
        });
    }
    let expected_labels = collector.label_count();
    if expected_labels != label_values.len() || handle.label_count != label_values.len() {
        return Err(ProductMetricError::LabelCountMismatch {
            subsystem: handle.subsystem,
            name: handle.name,
            expected: expected_labels,
            actual: label_values.len(),
        });
    }

    record(collector);
    Ok(())
}

/// Declares a typed product metric set backed by the shared Prometheus registry.
///
/// The generated struct owns typed metric handles and exposes a `register()` function that
/// registers every declared metric in order.
///
/// ```
/// # #[cfg(feature = "backend-prometheus")]
/// # {
/// aster_forge_metrics::product_metrics! {
///     pub struct ProductMetrics {
///         requests: counter(
///             "example",
///             "requests_total",
///             "Total example requests.",
///             &["status"],
///         ),
///         latency: histogram_with_buckets(
///             "example",
///             "request_duration_seconds",
///             "Example request duration.",
///             &["status"],
///             &[0.1, 0.5, 1.0],
///         ),
///     }
/// }
/// # }
/// ```
#[macro_export]
macro_rules! product_metrics {
    (
        $(#[$struct_meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field:ident : $kind:ident (
                    $subsystem:expr,
                    $metric_name:expr,
                    $help:expr,
                    $labels:expr
                    $(, $buckets:expr)?
                    $(,)?
                )
            ),* $(,)?
        }
    ) => {
        $(#[$struct_meta])*
        $vis struct $name {
            $(
                $(#[$field_meta])*
                pub $field: $crate::product_metrics!(@field_type $kind),
            )*
        }

        impl $name {
            /// Registers every metric in this set and returns typed handles.
            pub fn register() -> $crate::prometheus::ProductMetricResult<Self> {
                Ok(Self {
                    $(
                        $field: $crate::product_metrics!(
                            @register
                            $kind,
                            $subsystem,
                            $metric_name,
                            $help,
                            $labels
                            $(, $buckets)?
                        )?,
                    )*
                })
            }
        }
    };
    (@field_type counter) => {
        $crate::prometheus::ProductCounter
    };
    (@field_type gauge) => {
        $crate::prometheus::ProductGauge
    };
    (@field_type histogram) => {
        $crate::prometheus::ProductHistogram
    };
    (@field_type histogram_with_buckets) => {
        $crate::prometheus::ProductHistogram
    };
    (@register counter, $subsystem:expr, $metric_name:expr, $help:expr, $labels:expr) => {
        $crate::prometheus::register_product_counter($crate::MetricDescriptor::counter(
            $subsystem,
            $metric_name,
            $help,
            $labels,
        ))
    };
    (@register gauge, $subsystem:expr, $metric_name:expr, $help:expr, $labels:expr) => {
        $crate::prometheus::register_product_gauge($crate::MetricDescriptor::gauge(
            $subsystem,
            $metric_name,
            $help,
            $labels,
        ))
    };
    (@register histogram, $subsystem:expr, $metric_name:expr, $help:expr, $labels:expr) => {
        $crate::prometheus::register_product_histogram($crate::MetricDescriptor::histogram(
            $subsystem,
            $metric_name,
            $help,
            $labels,
        ))
    };
    (@register histogram_with_buckets, $subsystem:expr, $metric_name:expr, $help:expr, $labels:expr, $buckets:expr) => {
        $crate::prometheus::register_product_histogram(
            $crate::MetricDescriptor::histogram_with_buckets(
                $subsystem,
                $metric_name,
                $help,
                $labels,
                $buckets,
            ),
        )
    };
}

/// Prometheus recorder for shared infrastructure metrics.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrometheusMetricsRecorder;

impl DbMetricsRecorder for PrometheusMetricsRecorder {
    fn enabled(&self) -> bool {
        true
    }

    fn record_db_query(&self, metric: &DbQueryMetric) {
        record_db_query(metric);
    }
}

impl MetricsRecorder for PrometheusMetricsRecorder {
    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {
        record_http_request(method, route, status, duration_seconds);
    }

    fn record_auth_event(&self, action: &'static str, status: &'static str, reason: &'static str) {
        record_auth_event(action, status, reason);
    }

    fn record_application_event(
        &self,
        category: &'static str,
        event: &'static str,
        status: &'static str,
    ) {
        record_application_event(category, event, status);
    }

    fn record_config_reload(
        &self,
        source: &'static str,
        decision: &'static str,
        status: &'static str,
        changed_keys: u64,
        duration_seconds: f64,
    ) {
        record_config_reload(source, decision, status, changed_keys, duration_seconds);
    }

    fn record_config_mutation(
        &self,
        source: &'static str,
        operation: &'static str,
        status: &'static str,
        changed_keys: u64,
    ) {
        record_config_mutation(source, operation, status, changed_keys);
    }

    fn record_background_task_transition(&self, kind: &'static str, status: &'static str) {
        record_background_task_transition(kind, status);
    }

    fn set_background_tasks_pending(&self, pending: u64) {
        set_background_tasks_pending(pending);
    }

    fn record_external_operation(
        &self,
        system: &'static str,
        operation: &'static str,
        status: &'static str,
        duration_seconds: f64,
    ) {
        record_external_operation(system, operation, status, duration_seconds);
    }

    fn system_metrics_updater_task(
        &self,
        shutdown_token: CancellationToken,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        Some(Box::pin(system_metrics_updater_task(shutdown_token)))
    }
}

#[cfg(feature = "runtime-health")]
impl aster_forge_runtime::HealthMetricsRecorder for PrometheusMetricsRecorder {
    fn record_health_report(
        &self,
        scope: &'static str,
        status: aster_forge_runtime::HealthStatus,
        duration_seconds: f64,
    ) {
        record_health_report(
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
        record_health_component(
            scope,
            component.name,
            component.status.as_str(),
            health_status_value(component.status.as_str()),
            duration_seconds,
        );
    }
}

fn record_http_request(method: &str, route: &str, status: u16, duration_seconds: f64) {
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

fn record_db_query(metric: &DbQueryMetric) {
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

fn record_auth_event(action: &'static str, status: &'static str, reason: &'static str) {
    let Some(metrics) = METRICS.get() else {
        return;
    };

    metrics
        .auth_events_total
        .with_label_values(&[action, status, reason])
        .inc();
}

fn record_application_event(category: &'static str, event: &'static str, status: &'static str) {
    let Some(metrics) = METRICS.get() else {
        return;
    };

    metrics
        .application_events_total
        .with_label_values(&[category, event, status])
        .inc();
}

fn record_config_reload(
    source: &'static str,
    decision: &'static str,
    status: &'static str,
    changed_keys: u64,
    duration_seconds: f64,
) {
    let Some(metrics) = METRICS.get() else {
        return;
    };

    metrics
        .config_reloads_total
        .with_label_values(&[source, decision, status])
        .inc();
    metrics
        .config_reload_duration_seconds
        .with_label_values(&[source, decision, status])
        .observe(duration_seconds);
    metrics
        .config_reload_changed_keys
        .with_label_values(&[source, decision, status])
        .observe(changed_keys as f64);
}

fn record_config_mutation(
    source: &'static str,
    operation: &'static str,
    status: &'static str,
    changed_keys: u64,
) {
    let Some(metrics) = METRICS.get() else {
        return;
    };

    metrics
        .config_mutations_total
        .with_label_values(&[source, operation, status])
        .inc();
    metrics
        .config_mutation_changed_keys
        .with_label_values(&[source, operation, status])
        .observe(changed_keys as f64);
}

fn record_background_task_transition(kind: &'static str, status: &'static str) {
    let Some(metrics) = METRICS.get() else {
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

fn set_background_tasks_pending(pending: u64) {
    let Some(metrics) = METRICS.get() else {
        return;
    };

    metrics
        .background_tasks_pending
        .set(i64::try_from(pending).unwrap_or(i64::MAX));
}

fn record_external_operation(
    system: &'static str,
    operation: &'static str,
    status: &'static str,
    duration_seconds: f64,
) {
    let Some(metrics) = METRICS.get() else {
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

/// Records an aggregate health report into the shared Prometheus registry.
pub fn record_health_report(
    scope: &'static str,
    status_label: &'static str,
    status_value: f64,
    duration_seconds: f64,
) {
    let Some(metrics) = METRICS.get() else {
        return;
    };

    metrics
        .health_report_status
        .with_label_values(&[scope])
        .set(status_value);
    metrics
        .health_report_duration_seconds
        .with_label_values(&[scope, status_label])
        .observe(duration_seconds);
}

/// Records one health component into the shared Prometheus registry.
pub fn record_health_component(
    scope: &'static str,
    component: &'static str,
    status_label: &'static str,
    status_value: f64,
    duration_seconds: f64,
) {
    let Some(metrics) = METRICS.get() else {
        return;
    };

    metrics
        .health_component_status
        .with_label_values(&[scope, component])
        .set(status_value);
    metrics
        .health_component_duration_seconds
        .with_label_values(&[scope, component, status_label])
        .observe(duration_seconds);
}

#[cfg(feature = "runtime-health")]
fn health_status_value(status: &'static str) -> f64 {
    match status {
        "healthy" => 0.0,
        "degraded" => 1.0,
        "unhealthy" => 2.0,
        _ => 2.0,
    }
}

async fn system_metrics_updater_task(shutdown_token: CancellationToken) {
    use std::sync::Mutex;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        tokio::select! {
            biased;
            _ = shutdown_token.cancelled() => break,
            _ = interval.tick() => {}
        }

        if shutdown_token.is_cancelled() {
            break;
        }

        let Some(metrics) = METRICS.get() else {
            continue;
        };

        let update = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let pid = Pid::from_u32(std::process::id());
            let sys_mutex = SYSTEM.get_or_init(|| Mutex::new(System::new()));
            let Ok(mut sys) = sys_mutex.lock() else {
                tracing::warn!("system metrics updater lock is poisoned");
                return;
            };
            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing().with_memory().with_cpu(),
            );
            if let Some(process) = sys.process(pid) {
                metrics
                    .process_memory_rss_bytes
                    .set(process.memory() as f64);
                let cpu_millis = i64::try_from(process.accumulated_cpu_time()).unwrap_or(i64::MAX);
                metrics.process_cpu_milliseconds_total.set(cpu_millis);
            }
            let uptime = PROCESS_STARTED_AT
                .get()
                .map(Instant::elapsed)
                .unwrap_or_default()
                .as_secs_f64();
            metrics.uptime_seconds.set(uptime);
            metrics.refresh_allocator_metrics();
        }));

        if let Err(panic) = update {
            tracing::error!(panic = %panic_message(panic), "system metrics updater panicked");
        }
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProductCounter, ProductGauge, ProductHistogram, ProductMetricError, ProductMetricHandle,
        PrometheusMetricsRecorder, export_metrics, inc_product_counter, init_metrics,
        observe_product_histogram, record_health_component, record_health_report,
        register_product_counter, register_product_gauge, register_product_histogram,
        register_product_metric, register_product_metrics, set_product_gauge,
    };
    use crate::{DbMetricsRecorder, MetricDescriptor, MetricKind, MetricsRecorder};

    #[test]
    fn prometheus_recorder_exports_low_cardinality_metrics() {
        init_metrics().expect("metrics registry should initialize");
        let recorder = PrometheusMetricsRecorder;

        recorder.record_http_request("GET", "/health", 200, 0.01);
        recorder.record_config_reload("pubsub", "reloaded", "ok", 3, 0.02);
        recorder.record_config_mutation("api", "upsert", "error", 1);
        record_health_report("diagnostics", "degraded", 1.0, 0.25);
        record_health_component("diagnostics", "cache", "degraded", 1.0, 0.05);

        let body = export_metrics().expect("metrics should export");
        assert!(recorder.enabled());
        assert!(body.contains("http_requests_total"));
        assert!(body.contains("config_reloads_total"));
        assert!(body.contains("config_reload_duration_seconds_count"));
        assert!(body.contains("config_mutations_total"));
        assert!(body.contains("health_report_status"));
        assert!(body.contains("health_component_status"));
        assert!(body.contains("source=\"pubsub\""));
        assert!(body.contains("decision=\"reloaded\""));
        assert!(body.contains("operation=\"upsert\""));
    }

    #[cfg(feature = "allocator-metrics")]
    #[test]
    fn allocator_metrics_export_heap_memory_kinds() {
        init_metrics().expect("metrics registry should initialize");

        let body = export_metrics().expect("metrics should export");

        assert!(body.contains("process_heap_memory_mib"));
        assert!(body.contains("kind=\"allocated\""));
        assert!(body.contains("kind=\"peak_or_resident\""));
    }

    #[test]
    fn product_metric_operations_fail_before_registration() {
        init_metrics().expect("metrics registry should initialize");
        let handle = ProductMetricHandle {
            subsystem: "missing_product_metric_test",
            name: "events_total",
            kind: MetricKind::Counter,
            label_count: 0,
        };

        let error = inc_product_counter(handle, &[], 1)
            .expect_err("unknown metric handle should be rejected");

        assert_eq!(
            error,
            ProductMetricError::UnknownMetric {
                subsystem: "missing_product_metric_test",
                name: "events_total"
            }
        );
    }

    #[test]
    fn typed_product_metric_handles_record_and_validate_kind() {
        init_metrics().expect("metrics registry should initialize");

        let counter = register_product_counter(MetricDescriptor::counter(
            "typed_product_metric_test",
            "events_total",
            "Typed product metric events.",
            &["status"],
        ))
        .expect("counter should register");
        let gauge = register_product_gauge(MetricDescriptor::gauge(
            "typed_product_metric_test",
            "queue_depth",
            "Typed product metric queue depth.",
            &["queue"],
        ))
        .expect("gauge should register");
        let histogram = register_product_histogram(MetricDescriptor::histogram_with_buckets(
            "typed_product_metric_test",
            "duration_seconds",
            "Typed product metric duration.",
            &["kind"],
            &[0.1, 1.0],
        ))
        .expect("histogram should register");

        counter.try_inc(&["ok"], 2).expect("counter should record");
        counter.inc(&["ok"], 1);
        gauge.try_set(&["mail"], 4.0).expect("gauge should set");
        gauge.add(&["mail"], 1.0);
        histogram
            .try_observe(&["dispatch"], 0.2)
            .expect("histogram should observe");
        histogram.observe(&["dispatch"], 0.3);

        let wrong_kind = ProductGauge::from_handle(counter.handle())
            .expect_err("counter handle should not become a gauge");
        assert_eq!(
            wrong_kind,
            ProductMetricError::WrongKind {
                subsystem: "typed_product_metric_test",
                name: "events_total",
                actual: MetricKind::Counter,
                expected: MetricKind::Gauge
            }
        );
        assert!(ProductCounter::from_handle(counter.handle()).is_ok());
        assert!(ProductHistogram::from_handle(histogram.handle()).is_ok());

        let body = export_metrics().expect("metrics should export");
        assert!(body.contains("typed_product_metric_test_events_total"));
        assert!(body.contains("typed_product_metric_test_queue_depth"));
        assert!(body.contains("typed_product_metric_test_duration_seconds_bucket"));
    }

    #[test]
    fn product_metrics_macro_registers_typed_metric_set() {
        init_metrics().expect("metrics registry should initialize");

        crate::product_metrics! {
            #[derive(Clone, Copy)]
            pub struct MacroProductMetrics {
                /// Macro counter.
                requests: counter(
                    "macro_product_metric_test",
                    "requests_total",
                    "Macro product metric requests.",
                    &["status"],
                ),
                queue_depth: gauge(
                    "macro_product_metric_test",
                    "queue_depth",
                    "Macro product metric queue depth.",
                    &["queue"],
                ),
                latency: histogram_with_buckets(
                    "macro_product_metric_test",
                    "latency_seconds",
                    "Macro product metric latency.",
                    &["route"],
                    &[0.01, 0.1],
                ),
            }
        }

        let metrics = MacroProductMetrics::register().expect("metric set should register");
        metrics.requests.inc(&["ok"], 1);
        metrics.queue_depth.set(&["mail"], 3.0);
        metrics.latency.observe(&["/healthz"], 0.02);

        let body = export_metrics().expect("metrics should export");
        assert!(body.contains("macro_product_metric_test_requests_total"));
        assert!(body.contains("macro_product_metric_test_queue_depth"));
        assert!(body.contains("macro_product_metric_test_latency_seconds_bucket"));
    }

    #[test]
    fn product_metrics_register_record_and_validate_boundaries() {
        init_metrics().expect("metrics registry should initialize");

        let counter = register_product_metric(MetricDescriptor::counter(
            "product_registration_test",
            "events_total",
            "Product registration test events.",
            &["kind", "status"],
        ))
        .expect("counter should register");
        assert_eq!(counter.subsystem(), "product_registration_test");
        assert_eq!(counter.name(), "events_total");
        assert_eq!(counter.kind(), MetricKind::Counter);
        assert_eq!(counter.label_count(), 2);

        let duplicate = register_product_metric(MetricDescriptor::counter(
            "product_registration_test",
            "events_total",
            "Duplicate product registration test events.",
            &["kind", "status"],
        ))
        .expect_err("duplicate should be rejected before touching prometheus registry");
        assert_eq!(
            duplicate,
            ProductMetricError::DuplicateRegistration {
                subsystem: "product_registration_test",
                name: "events_total"
            }
        );

        let handles = register_product_metrics([
            MetricDescriptor::gauge(
                "product_registration_test",
                "queue_depth",
                "Product registration test queue depth.",
                &["queue"],
            ),
            MetricDescriptor::histogram_with_buckets(
                "product_registration_test",
                "job_duration_seconds",
                "Product registration test job duration.",
                &["kind"],
                &[0.1, 0.5, 1.0],
            ),
        ])
        .expect("gauge and histogram should register");
        assert_eq!(handles.len(), 2);

        inc_product_counter(counter, &["dispatch", "ok"], 3)
            .expect("counter should record with matching labels");
        set_product_gauge(handles[0], &["mail"], 7.0).expect("gauge should record");
        observe_product_histogram(handles[1], &["dispatch"], 0.25)
            .expect("histogram should record");

        let wrong_label_count = inc_product_counter(counter, &["dispatch"], 1)
            .expect_err("wrong label count should be rejected");
        assert_eq!(
            wrong_label_count,
            ProductMetricError::LabelCountMismatch {
                subsystem: "product_registration_test",
                name: "events_total",
                expected: 2,
                actual: 1
            }
        );

        let wrong_kind = set_product_gauge(counter, &["dispatch", "ok"], 1.0)
            .expect_err("wrong recording kind should be rejected");
        assert_eq!(
            wrong_kind,
            ProductMetricError::WrongKind {
                subsystem: "product_registration_test",
                name: "events_total",
                actual: MetricKind::Counter,
                expected: MetricKind::Gauge
            }
        );

        let body = export_metrics().expect("metrics should export");
        assert!(body.contains("product_registration_test_events_total"));
        assert!(body.contains("kind=\"dispatch\""));
        assert!(body.contains("status=\"ok\""));
        assert!(body.contains("product_registration_test_queue_depth"));
        assert!(body.contains("queue=\"mail\""));
        assert!(body.contains("product_registration_test_job_duration_seconds_bucket"));
        assert!(body.contains("le=\"0.5\""));
    }
}

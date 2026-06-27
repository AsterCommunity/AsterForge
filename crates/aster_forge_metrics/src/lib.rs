//! Shared metrics recorder traits and subsystem registration primitives.
//!
//! Applications often share the same infrastructure metrics while exposing different
//! product-domain metrics. This crate keeps the common recorder surface small and provides a
//! registration catalog so each subsystem can describe the metrics it owns without forcing every
//! product-specific method into a single shared trait. Concrete backends, such as Prometheus
//! collectors, remain in application crates where label choices and feature flags are known.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Normalized database backend label used by infrastructure metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbMetricBackend {
    /// SQLite backend.
    Sqlite,
    /// MySQL backend.
    MySql,
    /// PostgreSQL backend.
    Postgres,
    /// A backend not recognized by this shared metrics surface.
    Other,
}

impl DbMetricBackend {
    /// Returns the stable label used for metrics exporters.
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::MySql => "mysql",
            Self::Postgres => "postgres",
            Self::Other => "other",
        }
    }
}

/// Normalized database query kind used by infrastructure metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbQueryKind {
    /// SELECT or read-like query.
    Select,
    /// INSERT query.
    Insert,
    /// UPDATE query.
    Update,
    /// DELETE query.
    Delete,
    /// Common table expression query.
    With,
    /// Transaction control statement.
    Transaction,
    /// Data definition statement.
    Ddl,
    /// SQLite PRAGMA statement.
    Pragma,
    /// Query kind that could not be classified cheaply.
    Other,
}

impl DbQueryKind {
    /// Returns the stable label used for metrics exporters.
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::With => "with",
            Self::Transaction => "transaction",
            Self::Ddl => "ddl",
            Self::Pragma => "pragma",
            Self::Other => "other",
        }
    }
}

/// Product-neutral database query metric emitted by database adapters.
///
/// This shape intentionally avoids exposing raw SQL to metrics recorders. Products should keep DB
/// metrics low-cardinality and free of query parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbQueryMetric {
    /// Database backend that executed the query.
    pub backend: DbMetricBackend,
    /// Low-cardinality query kind.
    pub kind: DbQueryKind,
    /// Whether the query failed.
    pub failed: bool,
    /// Query duration observed by the database adapter.
    pub elapsed: Duration,
}

impl DbQueryMetric {
    /// Creates a database query metric.
    pub const fn new(
        backend: DbMetricBackend,
        kind: DbQueryKind,
        failed: bool,
        elapsed: Duration,
    ) -> Self {
        Self {
            backend,
            kind,
            failed,
            elapsed,
        }
    }

    /// Returns the stable status label.
    pub const fn status_label(&self) -> &'static str {
        if self.failed { "error" } else { "ok" }
    }
}

/// Minimal metrics hook used by database connection helpers.
pub trait DbMetricsRecorder: Send + Sync {
    /// Returns whether metrics are actively recorded.
    fn enabled(&self) -> bool;

    /// Records one database query metric.
    fn record_db_query(&self, metric: &DbQueryMetric);
}

/// Metrics recorder that ignores every database query.
#[derive(Debug, Default)]
pub struct NoopDbMetrics;

impl DbMetricsRecorder for NoopDbMetrics {
    fn enabled(&self) -> bool {
        false
    }

    fn record_db_query(&self, _metric: &DbQueryMetric) {}
}

/// Shared trait object for database metrics recorders.
pub type SharedDbMetricsRecorder = Arc<dyn DbMetricsRecorder>;

/// Application-wide metrics recorder interface.
///
/// Product crates should keep domain-specific metrics in extension traits or subsystem recorders.
/// The methods here cover infrastructure signals that are shared by Aster services.
#[allow(unused_variables)]
pub trait MetricsRecorder: DbMetricsRecorder + Send + Sync {
    /// Records an HTTP request.
    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {}

    /// Records an authentication event.
    fn record_auth_event(&self, action: &'static str, status: &'static str, reason: &'static str) {}

    /// Records a generic application event.
    fn record_application_event(
        &self,
        category: &'static str,
        event: &'static str,
        status: &'static str,
    ) {
    }

    /// Records a background task state transition.
    fn record_background_task_transition(&self, kind: &'static str, status: &'static str) {}

    /// Sets the number of pending background tasks.
    fn set_background_tasks_pending(&self, pending: u64) {}

    /// Records an operation against an external system.
    fn record_external_operation(
        &self,
        system: &'static str,
        operation: &'static str,
        status: &'static str,
        duration_seconds: f64,
    ) {
    }

    /// Creates an optional background task that updates system-level metrics.
    fn system_metrics_updater_task(
        &self,
        shutdown_token: CancellationToken,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        None
    }
}

/// Shared trait object for application metrics recorders.
pub type SharedMetricsRecorder = Arc<dyn MetricsRecorder>;

/// Metrics recorder that ignores every event.
#[derive(Debug, Default)]
pub struct NoopMetrics;

impl MetricsRecorder for NoopMetrics {}

impl DbMetricsRecorder for NoopMetrics {
    fn enabled(&self) -> bool {
        false
    }

    fn record_db_query(&self, _metric: &DbQueryMetric) {}
}

impl NoopMetrics {
    /// Creates a noop recorder.
    pub fn new() -> Self {
        Self
    }

    /// Creates a shared noop recorder.
    pub fn arc() -> SharedMetricsRecorder {
        Arc::new(Self::new())
    }
}

/// Initializes a concrete metrics backend or falls back to [`NoopMetrics`].
///
/// Product crates own concrete exporters and feature flags. This helper only centralizes the
/// common startup mechanics used by Aster services: try to initialize the product metrics backend,
/// return the concrete recorder on success, and keep startup working with a no-op recorder on
/// initialization failure.
pub fn init_metrics_or_noop<I, B, E, R>(init_metrics: I, build_recorder: B) -> SharedMetricsRecorder
where
    I: FnOnce() -> std::result::Result<(), E>,
    B: FnOnce() -> R,
    E: std::fmt::Display,
    R: MetricsRecorder + 'static,
{
    match init_metrics() {
        Ok(()) => {
            tracing::info!("metrics backend initialized");
            Arc::new(build_recorder())
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "failed to initialize metrics backend; using noop metrics"
            );
            NoopMetrics::arc()
        }
    }
}

/// Kind of metric described by a subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Monotonically increasing counter.
    Counter,
    /// Point-in-time value.
    Gauge,
    /// Duration or distribution bucket metric.
    Histogram,
}

/// Static metric descriptor registered by a subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricDescriptor {
    /// Owning subsystem name.
    pub subsystem: &'static str,
    /// Metric name without backend-specific namespace decoration.
    pub name: &'static str,
    /// Human-readable help text.
    pub help: &'static str,
    /// Metric kind.
    pub kind: MetricKind,
    /// Ordered label names used by the metric.
    pub labels: &'static [&'static str],
}

impl MetricDescriptor {
    /// Creates a descriptor for a counter metric.
    pub const fn counter(
        subsystem: &'static str,
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            subsystem,
            name,
            help,
            kind: MetricKind::Counter,
            labels,
        }
    }

    /// Creates a descriptor for a gauge metric.
    pub const fn gauge(
        subsystem: &'static str,
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            subsystem,
            name,
            help,
            kind: MetricKind::Gauge,
            labels,
        }
    }

    /// Creates a descriptor for a histogram metric.
    pub const fn histogram(
        subsystem: &'static str,
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            subsystem,
            name,
            help,
            kind: MetricKind::Histogram,
            labels,
        }
    }
}

/// Errors returned by metrics registration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MetricRegistrationError {
    /// A subsystem tried to register the same metric name twice.
    #[error("duplicate metric registration: {subsystem}.{name}")]
    DuplicateMetric {
        /// Owning subsystem name.
        subsystem: &'static str,
        /// Duplicate metric name.
        name: &'static str,
    },
}

/// Result type returned by metrics registration helpers.
pub type Result<T> = std::result::Result<T, MetricRegistrationError>;

/// Catalog of metric descriptors registered by application subsystems.
#[derive(Debug, Default)]
pub struct MetricCatalog {
    descriptors: Vec<MetricDescriptor>,
}

impl MetricCatalog {
    /// Creates an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one metric descriptor.
    pub fn register(&mut self, descriptor: MetricDescriptor) -> Result<()> {
        if self.descriptors.iter().any(|existing| {
            existing.subsystem == descriptor.subsystem && existing.name == descriptor.name
        }) {
            return Err(MetricRegistrationError::DuplicateMetric {
                subsystem: descriptor.subsystem,
                name: descriptor.name,
            });
        }

        self.descriptors.push(descriptor);
        Ok(())
    }

    /// Returns all descriptors in registration order.
    pub fn descriptors(&self) -> &[MetricDescriptor] {
        &self.descriptors
    }

    /// Returns descriptors owned by `subsystem`.
    pub fn subsystem<'a>(
        &'a self,
        subsystem: &'a str,
    ) -> impl Iterator<Item = &'a MetricDescriptor> + 'a {
        self.descriptors
            .iter()
            .filter(move |descriptor| descriptor.subsystem == subsystem)
    }
}

/// A subsystem that owns and registers a set of metric descriptors.
pub trait MetricsSubsystem {
    /// Stable subsystem name.
    fn name(&self) -> &'static str;

    /// Registers metric descriptors owned by this subsystem.
    fn register_metrics(&self, catalog: &mut MetricCatalog) -> Result<()>;
}

/// Registers every subsystem into one catalog.
pub fn register_subsystems(
    catalog: &mut MetricCatalog,
    subsystems: &[&dyn MetricsSubsystem],
) -> Result<()> {
    for subsystem in subsystems {
        subsystem.register_metrics(catalog)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct EnabledMetrics;

    impl DbMetricsRecorder for EnabledMetrics {
        fn enabled(&self) -> bool {
            true
        }

        fn record_db_query(&self, _metric: &DbQueryMetric) {}
    }

    impl MetricsRecorder for EnabledMetrics {}

    struct HttpSubsystem;

    impl MetricsSubsystem for HttpSubsystem {
        fn name(&self) -> &'static str {
            "http"
        }

        fn register_metrics(&self, catalog: &mut MetricCatalog) -> Result<()> {
            catalog.register(MetricDescriptor::histogram(
                self.name(),
                "request_duration_seconds",
                "HTTP request duration.",
                &["method", "route", "status"],
            ))
        }
    }

    #[test]
    fn noop_metrics_reports_disabled() {
        let recorder = NoopMetrics::new();

        assert!(!recorder.enabled());
        recorder.record_http_request("GET", "/health", 200, 0.01);
        recorder.record_auth_event("login", "ok", "password");
        recorder.record_application_event("config", "updated", "ok");
        recorder.record_background_task_transition("cleanup", "completed");
        recorder.set_background_tasks_pending(2);
        recorder.record_external_operation("oidc", "token", "ok", 0.02);
        assert!(
            recorder
                .system_metrics_updater_task(CancellationToken::new())
                .is_none()
        );
    }

    #[test]
    fn database_metric_labels_are_stable() {
        assert_eq!(DbMetricBackend::Sqlite.as_label(), "sqlite");
        assert_eq!(DbMetricBackend::MySql.as_label(), "mysql");
        assert_eq!(DbMetricBackend::Postgres.as_label(), "postgres");
        assert_eq!(DbMetricBackend::Other.as_label(), "other");

        assert_eq!(DbQueryKind::Select.as_label(), "select");
        assert_eq!(DbQueryKind::Insert.as_label(), "insert");
        assert_eq!(DbQueryKind::Update.as_label(), "update");
        assert_eq!(DbQueryKind::Delete.as_label(), "delete");
        assert_eq!(DbQueryKind::With.as_label(), "with");
        assert_eq!(DbQueryKind::Transaction.as_label(), "transaction");
        assert_eq!(DbQueryKind::Ddl.as_label(), "ddl");
        assert_eq!(DbQueryKind::Pragma.as_label(), "pragma");
        assert_eq!(DbQueryKind::Other.as_label(), "other");

        let ok = DbQueryMetric::new(
            DbMetricBackend::Sqlite,
            DbQueryKind::Select,
            false,
            std::time::Duration::from_millis(3),
        );
        assert_eq!(ok.status_label(), "ok");

        let failed = DbQueryMetric::new(
            DbMetricBackend::Sqlite,
            DbQueryKind::Select,
            true,
            std::time::Duration::from_millis(3),
        );
        assert_eq!(failed.status_label(), "error");
    }

    #[test]
    fn init_metrics_or_noop_returns_concrete_recorder_after_successful_init() {
        let recorder = init_metrics_or_noop(|| Ok::<(), &'static str>(()), || EnabledMetrics);

        assert!(recorder.enabled());
    }

    #[test]
    fn init_metrics_or_noop_returns_noop_recorder_after_failed_init() {
        let recorder = init_metrics_or_noop(|| Err::<(), _>("registry failed"), || EnabledMetrics);

        assert!(!recorder.enabled());
    }

    #[test]
    fn catalog_registers_descriptors_in_order() {
        let mut catalog = MetricCatalog::new();

        catalog
            .register(MetricDescriptor::counter(
                "auth",
                "events_total",
                "Authentication events.",
                &["action", "status"],
            ))
            .expect("auth metric should register");
        catalog
            .register(MetricDescriptor::gauge(
                "tasks",
                "pending",
                "Pending background tasks.",
                &[],
            ))
            .expect("task metric should register");

        assert_eq!(catalog.descriptors().len(), 2);
        assert_eq!(catalog.descriptors()[0].name, "events_total");
        assert_eq!(catalog.subsystem("tasks").count(), 1);
    }

    #[test]
    fn catalog_rejects_duplicate_subsystem_metric_names() {
        let mut catalog = MetricCatalog::new();
        let first = MetricDescriptor::counter("auth", "events_total", "first", &[]);
        let duplicate = MetricDescriptor::counter("auth", "events_total", "second", &["status"]);

        catalog
            .register(first)
            .expect("first metric should register");
        let error = catalog
            .register(duplicate)
            .expect_err("duplicate metric should be rejected");

        assert_eq!(
            error,
            MetricRegistrationError::DuplicateMetric {
                subsystem: "auth",
                name: "events_total"
            }
        );
    }

    #[test]
    fn register_subsystems_delegates_to_each_subsystem() {
        let mut catalog = MetricCatalog::new();

        register_subsystems(&mut catalog, &[&HttpSubsystem])
            .expect("subsystem metrics should register");

        assert_eq!(catalog.descriptors().len(), 1);
        assert_eq!(catalog.descriptors()[0].subsystem, "http");
        assert_eq!(catalog.descriptors()[0].kind, MetricKind::Histogram);
    }
}

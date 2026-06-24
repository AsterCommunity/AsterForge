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

use tokio_util::sync::CancellationToken;

/// Application-wide metrics recorder interface.
///
/// Product crates should keep domain-specific metrics in extension traits or subsystem recorders.
/// The methods here cover infrastructure signals that are shared by Aster services.
#[allow(unused_variables)]
pub trait MetricsRecorder: aster_forge_db::DbMetricsRecorder + Send + Sync {
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

impl aster_forge_db::DbMetricsRecorder for NoopMetrics {
    fn enabled(&self) -> bool {
        false
    }

    fn record_db_query(&self, _info: &sea_orm::metric::Info<'_>) {}
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
    use aster_forge_db::DbMetricsRecorder;

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

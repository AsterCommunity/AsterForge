//! Shared metrics recorder traits and subsystem registration primitives.
//!
//! Applications often share the same infrastructure metrics while exposing different
//! product-domain metrics. This crate keeps the common recorder surface small and provides a
//! instance-owned provider so each subsystem can register the metrics it owns without forcing every
//! product-specific method into a single shared trait. Concrete backends are selected once at the
//! product entrypoint through Forge feature flags, so business modules record metric semantics
//! without depending on exporter crates.
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

mod instruments;
mod provider;
mod shared;
pub use instruments::*;
pub use provider::*;
pub use shared::{InfrastructureMetricsRecorder, normalized_http_method};
#[cfg(test)]
mod instance_tests {
    include!("tests.rs");
}
#[cfg(feature = "otlp")]
mod otlp;
#[cfg(feature = "otlp")]
pub use otlp::*;

/// Normalized database backend label used by infrastructure metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbMetricBackend {
    /// `SQLite` backend.
    Sqlite,
    /// `MySQL` backend.
    MySql,
    /// `PostgreSQL` backend.
    Postgres,
    /// A backend not recognized by this shared metrics surface.
    Other,
}

impl DbMetricBackend {
    /// Returns the stable label used for metrics exporters.
    #[must_use]
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
    /// `SQLite` PRAGMA statement.
    Pragma,
    /// Query kind that could not be classified cheaply.
    Other,
}

impl DbQueryKind {
    /// Returns the stable label used for metrics exporters.
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
#[expect(
    unused_variables,
    reason = "Default trait methods intentionally ignore metric inputs so products can implement only the signals they collect."
)]
pub trait MetricsRecorder: DbMetricsRecorder + Send + Sync {
    /// Instance resources for endpoint injection and lifecycle management.
    fn provider(&self) -> Option<MetricsProvider> {
        None
    }

    #[cfg(feature = "runtime-health")]
    fn record_health(
        &self,
        _scope: &'static str,
        _report: &aster_forge_runtime::SystemHealthReport,
    ) {
    }
    /// Records an HTTP request using Aster Metrics v1 transport semantics.
    /// `method` and `route` become metric label values and MUST be low
    /// cardinality: pass the route template (e.g. `/api/v1/files/{id}`),
    /// never the raw request path. Raw paths containing IDs or UUIDs allocate
    /// a new, never-freed time series per distinct value — a user-input-driven
    /// memory bomb.
    fn record_http_request(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        duration_seconds: f64,
    ) {
    }

    /// Records a cancelled or failed response stream without inventing an HTTP status.
    fn record_http_stream_failure(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        duration_seconds: f64,
        cancelled: bool,
    ) {
        self.record_http_request(method, scheme, route, status, duration_seconds);
    }

    /// Adjusts active HTTP handlers before their route or final status is known.
    fn http_active(&self, method: &str, scheme: &str, delta: i64) {}

    /// Publishes actual role readiness from the same result as the ready endpoint.
    fn role_readiness(&self, ready: bool) {}

    /// Publishes native idle/used counts and an optional real pool maximum.
    fn db_pool(&self, pool: &str, idle: u32, used: u32, max: Option<u32>) {}

    /// Adjusts actual in-process task handler executions, independent of persistence.
    fn task_handler_active(&self, kind: &'static str, delta: i64) {}

    /// Records a local handler return/drop; this does not assert a committed task result.
    fn task_handler_finished(&self, kind: &'static str, result: &'static str, seconds: f64) {}

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

    /// Records a runtime configuration reload attempt.
    fn record_config_reload(
        &self,
        source: &'static str,
        decision: &'static str,
        status: &'static str,
        changed_keys: u64,
        duration_seconds: f64,
    ) {
    }

    /// Records a runtime configuration mutation.
    fn record_config_mutation(
        &self,
        source: &'static str,
        operation: &'static str,
        status: &'static str,
        changed_keys: u64,
    ) {
    }

    /// Records a background task state transition.
    fn record_background_task_transition(&self, kind: &'static str, status: &'static str) {}

    /// Sets the number of pending background tasks.
    fn set_background_tasks_pending(&self, pending: u64) {}

    /// Sets the authoritative age of the oldest queued task; an empty queue has age zero.
    fn set_background_tasks_oldest_age_seconds(&self, seconds: f64) {}

    /// Records a successful claim that took over an expired processing lease.
    fn record_task_lease_takeover(&self, kind: &'static str) {}

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

/// Owns exactly one handler active increment, including cancellation and unwind.
pub struct HttpRequestGuard {
    recorder: SharedMetricsRecorder,
    method: &'static str,
    scheme: &'static str,
    route: String,
    status: u16,
    started: std::time::Instant,
    finished: bool,
}
impl HttpRequestGuard {
    /// Starts one handler; route and status are deliberately unknown here.
    #[must_use]
    pub fn new(recorder: SharedMetricsRecorder, method: &str, scheme: &str) -> Self {
        let method = normalized_http_method(method);
        let scheme = if scheme == "https" { "https" } else { "http" };
        recorder.http_active(method, scheme, 1);
        Self {
            recorder,
            method,
            scheme,
            route: String::new(),
            status: 0,
            started: std::time::Instant::now(),
            finished: false,
        }
    }
    /// Supplies matched transport metadata without changing the lifetime owner.
    pub fn response(&mut self, route: String, status: u16) {
        self.route = route;
        self.status = status;
    }
    /// Records one terminal response-body observation.
    pub fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            self.recorder.http_active(self.method, self.scheme, -1);
            self.recorder.record_http_request(
                self.method,
                self.scheme,
                &self.route,
                self.status,
                self.started.elapsed().as_secs_f64(),
            );
        }
    }
    /// Records a body transport failure while preserving any committed status.
    pub fn fail(&mut self) {
        self.finish_failure(false);
    }
    fn finish_failure(&mut self, cancelled: bool) {
        if !self.finished {
            self.finished = true;
            self.recorder.http_active(self.method, self.scheme, -1);
            self.recorder.record_http_stream_failure(
                self.method,
                self.scheme,
                &self.route,
                self.status,
                self.started.elapsed().as_secs_f64(),
                cancelled,
            );
        }
    }
}
impl Drop for HttpRequestGuard {
    fn drop(&mut self) {
        self.finish_failure(true);
    }
}

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
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Creates a shared noop recorder.
    #[must_use]
    pub fn arc() -> SharedMetricsRecorder {
        Arc::new(Self::new())
    }
}

/// Kind of metric described by a subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Monotonically increasing counter.
    Counter,
    /// Signed additive state.
    UpDownCounter,
    /// Cumulative snapshot read at collection without repeated addition.
    ObservableCounter,
    /// Signed snapshot read at collection.
    ObservableUpDownCounter,
    /// Point-in-time value.
    Gauge,
    /// Duration or distribution bucket metric.
    Histogram,
}

/// Static metric descriptor registered by a subsystem.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// Subset of labels encoded as signed integers in OpenTelemetry attributes.
    pub integer_labels: &'static [&'static str],
    /// Optional histogram buckets.
    pub buckets: &'static [f64],
    /// UCUM unit, independent of the exporter name.
    pub unit: &'static str,
}

impl MetricDescriptor {
    /// Prometheus family name under the pinned exporter translation contract.
    #[must_use]
    pub fn prometheus_name(&self) -> String {
        let mut name = crate::provider::metric_name(self).replace('.', "_");
        let direct = |unit: &str| match unit {
            "d" => Some("days"),
            "h" => Some("hours"),
            "min" => Some("minutes"),
            "s" => Some("seconds"),
            "ms" => Some("milliseconds"),
            "us" => Some("microseconds"),
            "ns" => Some("nanoseconds"),
            "By" | "B" => Some("bytes"),
            "KiBy" => Some("kibibytes"),
            "MiBy" => Some("mebibytes"),
            "GiBy" => Some("gibibytes"),
            "TiBy" => Some("tibibytes"),
            "KBy" | "KB" => Some("kilobytes"),
            "MBy" | "MB" => Some("megabytes"),
            "GBy" | "GB" => Some("gigabytes"),
            "TBy" | "TB" => Some("terabytes"),
            "m" => Some("meters"),
            "V" => Some("volts"),
            "A" => Some("amperes"),
            "J" => Some("joules"),
            "W" => Some("watts"),
            "g" => Some("grams"),
            "Cel" => Some("celsius"),
            "Hz" => Some("hertz"),
            "1" => Some("ratio"),
            "%" => Some("percent"),
            _ => None,
        };
        let suffix = direct(self.unit).map(str::to_owned).or_else(|| {
            let (numerator, denominator) = self.unit.split_once('/')?;
            let per = match denominator {
                "s" => "second",
                "m" => "minute",
                "h" => "hour",
                "d" => "day",
                "w" => "week",
                "mo" => "month",
                "y" => "year",
                _ => return None,
            };
            let prefix = if ["1", "d", "h", "min", "s", "ms", "us", "ns"].contains(&numerator) {
                None
            } else {
                direct(numerator)
            };
            Some(prefix.map_or_else(
                || format!("per_{per}"),
                |prefix| format!("{prefix}_per_{per}"),
            ))
        });
        if let Some(unit) = suffix {
            name.push('_');
            name.push_str(&unit);
        }
        if matches!(
            self.kind,
            MetricKind::Counter | MetricKind::ObservableCounter
        ) {
            name.push_str("_total");
        }
        name
    }

    /// Creates a signed additive instrument.
    #[must_use]
    pub const fn up_down_counter(
        subsystem: &'static str,
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            kind: MetricKind::UpDownCounter,
            ..Self::gauge(subsystem, name, help, labels)
        }
    }
    /// Creates a cumulative snapshot instrument; callers set absolute values.
    #[must_use]
    pub const fn observable_counter(
        subsystem: &'static str,
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            kind: MetricKind::ObservableCounter,
            ..Self::gauge(subsystem, name, help, labels)
        }
    }
    /// Creates a signed snapshot instrument; callers set absolute values.
    #[must_use]
    pub const fn observable_up_down_counter(
        subsystem: &'static str,
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            kind: MetricKind::ObservableUpDownCounter,
            ..Self::gauge(subsystem, name, help, labels)
        }
    }
    /// Sets the stable instrument unit without changing the exported name.
    #[must_use]
    pub const fn with_unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }

    /// Encodes these declared label keys as integer attributes, rejecting non-integer values.
    #[must_use]
    pub const fn with_integer_labels(mut self, labels: &'static [&'static str]) -> Self {
        self.integer_labels = labels;
        self
    }

    /// Creates a descriptor for a counter metric.
    #[must_use]
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
            integer_labels: &[],
            buckets: &[],
            unit: "1",
        }
    }

    /// Creates a descriptor for a gauge metric.
    #[must_use]
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
            integer_labels: &[],
            buckets: &[],
            unit: "1",
        }
    }

    /// Creates a descriptor for a histogram metric.
    #[must_use]
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
            integer_labels: &[],
            buckets: &[],
            unit: "1",
        }
    }

    /// Creates a descriptor for a histogram metric with explicit buckets.
    #[must_use]
    pub const fn histogram_with_buckets(
        subsystem: &'static str,
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
        buckets: &'static [f64],
    ) -> Self {
        Self {
            subsystem,
            name,
            help,
            kind: MetricKind::Histogram,
            labels,
            integer_labels: &[],
            buckets,
            unit: "1",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_metrics_reports_disabled() {
        let recorder = NoopMetrics::new();

        assert!(!recorder.enabled());
        recorder.record_http_request("GET", "http", "/health", 200, 0.01);
        recorder.record_auth_event("login", "ok", "password");
        recorder.record_application_event("config", "updated", "ok");
        recorder.record_config_reload("api", "reloaded", "ok", 2, 0.01);
        recorder.record_config_mutation("api", "upsert", "ok", 1);
        recorder.record_background_task_transition("cleanup", "completed");
        recorder.set_background_tasks_pending(2);
        recorder.set_background_tasks_oldest_age_seconds(1.0);
        recorder.record_task_lease_takeover("cleanup");
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
    fn histogram_descriptor_preserves_explicit_buckets() {
        let descriptor = MetricDescriptor::histogram_with_buckets(
            "worker",
            "duration_seconds",
            "Worker duration.",
            &["kind"],
            &[0.1, 1.0],
        );

        assert_eq!(descriptor.kind, MetricKind::Histogram);
        assert_eq!(descriptor.labels, &["kind"]);
        assert_eq!(descriptor.buckets, &[0.1, 1.0]);
    }
}

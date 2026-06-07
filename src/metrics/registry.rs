//! Prometheus registry, metric families, and recording functions.

use crate::errors::display_error;
use prometheus::{
    Encoder, Gauge, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use sea_orm::DbBackend;
use std::sync::OnceLock;
use std::time::Instant;

static METRICS: OnceLock<Metrics> = OnceLock::new();
pub(crate) static PROCESS_STARTED_AT: OnceLock<Instant> = OnceLock::new();

pub struct Metrics {
    pub registry: Registry,
    pub http_requests_total: IntCounterVec,
    pub http_request_duration_seconds: HistogramVec,
    pub db_queries_total: IntCounterVec,
    pub db_query_duration_seconds: HistogramVec,
    pub auth_events_total: IntCounterVec,
    pub application_events_total: IntCounterVec,
    pub background_tasks_total: IntCounterVec,
    pub background_tasks_pending: IntGauge,
    pub background_task_retries_total: IntCounterVec,
    pub external_operations_total: IntCounterVec,
    pub external_operation_duration_seconds: HistogramVec,
    pub process_memory_rss_bytes: Gauge,
    pub process_cpu_milliseconds_total: IntGauge,
    pub uptime_seconds: Gauge,
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
                "Total database queries observed through SeaORM",
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
                "Total template-level application events",
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
        let process_memory_rss_bytes =
            Gauge::new("process_memory_rss_bytes", "Process RSS memory in bytes")?;
        let process_cpu_milliseconds_total = IntGauge::new(
            "process_cpu_milliseconds_total",
            "Process accumulated CPU time in milliseconds",
        )?;
        let uptime_seconds = Gauge::new("process_uptime_seconds", "Process uptime in seconds")?;

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
            Box::new(process_memory_rss_bytes.clone()),
            Box::new(process_cpu_milliseconds_total.clone()),
            Box::new(uptime_seconds.clone()),
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
            process_memory_rss_bytes,
            process_cpu_milliseconds_total,
            uptime_seconds,
        })
    }

    pub fn export(&self) -> Result<String, String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buf = Vec::new();
        encoder
            .encode(&metric_families, &mut buf)
            .map_err(display_error)?;
        String::from_utf8(buf).map_err(display_error)
    }
}

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

pub fn record_db_query(info: &sea_orm::metric::Info<'_>) {
    let Some(metrics) = get_metrics() else {
        return;
    };

    let backend = backend_label(info.statement.db_backend);
    let kind = query_kind_from_sql(&info.statement.sql);
    let status = if info.failed { "error" } else { "ok" };

    metrics
        .db_queries_total
        .with_label_values(&[backend, kind, status])
        .inc();
    metrics
        .db_query_duration_seconds
        .with_label_values(&[backend, kind, status])
        .observe(info.elapsed.as_secs_f64());
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

fn backend_label(backend: DbBackend) -> &'static str {
    match backend {
        DbBackend::MySql => "mysql",
        DbBackend::Postgres => "postgres",
        DbBackend::Sqlite => "sqlite",
        _ => "other",
    }
}

fn query_kind_from_sql(sql: &str) -> &'static str {
    let token = sql.split_whitespace().next().unwrap_or_default();
    if token.eq_ignore_ascii_case("SELECT") {
        "select"
    } else if token.eq_ignore_ascii_case("INSERT") {
        "insert"
    } else if token.eq_ignore_ascii_case("UPDATE") {
        "update"
    } else if token.eq_ignore_ascii_case("DELETE") {
        "delete"
    } else if token.eq_ignore_ascii_case("WITH") {
        "with"
    } else if token.eq_ignore_ascii_case("BEGIN")
        || token.eq_ignore_ascii_case("COMMIT")
        || token.eq_ignore_ascii_case("ROLLBACK")
        || token.eq_ignore_ascii_case("SAVEPOINT")
        || token.eq_ignore_ascii_case("RELEASE")
    {
        "transaction"
    } else if token.eq_ignore_ascii_case("CREATE")
        || token.eq_ignore_ascii_case("ALTER")
        || token.eq_ignore_ascii_case("DROP")
        || token.eq_ignore_ascii_case("TRUNCATE")
    {
        "ddl"
    } else if token.eq_ignore_ascii_case("PRAGMA") {
        "pragma"
    } else {
        "other"
    }
}

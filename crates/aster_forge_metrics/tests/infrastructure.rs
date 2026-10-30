#![cfg(feature = "backend-prometheus")]
use aster_forge_metrics::*;
use std::time::Duration;

fn setup() -> (MetricsProvider, InfrastructureMetricsRecorder) {
    let provider = MetricsProvider::builder()
        .with_prometheus()
        .unwrap()
        .build()
        .unwrap();
    let recorder = InfrastructureMetricsRecorder::new(provider.clone()).unwrap();
    (provider, recorder)
}

fn assert_sample(output: &str, name: &str, attributes: &[(&str, &str)], expected: f64) {
    let matching: Vec<_> = output
        .lines()
        .filter(|line| {
            line.strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('{') || rest.starts_with(' '))
                && attributes
                    .iter()
                    .all(|(key, value)| line.contains(&format!("{key}=\"{value}\"")))
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected one {name} {attributes:?}: {matching:?}"
    );
    let actual: f64 = matching[0].rsplit_once(' ').unwrap().1.parse().unwrap();
    assert!(
        (actual - expected).abs() < f64::EPSILON,
        "{name} {attributes:?}: expected {expected}, got {actual}"
    );
}

#[tokio::test]
async fn http_outcomes_and_duration_attributes_follow_transport_results() {
    let (provider, recorder) = setup();
    for status in [200, 302, 401, 403, 404, 429, 500, 503] {
        recorder.record_http_request("GET", "https", "/files/{id}", status, 0.25);
    }
    recorder.record_http_request("HEAD", "http", "", 0, 0.0);
    recorder.record_http_stream_failure("CUSTOM", "https", "", 0, 0.5, true);
    recorder.record_http_stream_failure("GET", "http", "/download/{id}", 200, 0.75, false);
    let raw = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    for (outcome, count) in [
        ("completed", 3.0),
        ("auth_rejected", 2.0),
        ("rejected", 2.0),
        ("server_error", 2.0),
        ("canceled", 1.0),
        ("transport_error", 1.0),
    ] {
        assert_sample(
            &raw,
            "aster_http_server_requests_total",
            &[("aster_request_outcome", outcome)],
            count,
        );
    }
    for status in ["200", "302", "401", "403", "404", "429", "500", "503"] {
        let labels = [
            ("http_request_method", "GET"),
            ("url_scheme", "https"),
            ("http_route", "/files/{id}"),
            ("http_response_status_code", status),
        ];
        assert_sample(
            &raw,
            "http_server_request_duration_seconds_count",
            &labels,
            1.0,
        );
        assert_sample(
            &raw,
            "http_server_request_duration_seconds_sum",
            &labels,
            0.25,
        );
        let line = raw
            .lines()
            .find(|line| {
                line.starts_with("http_server_request_duration_seconds_count{")
                    && labels
                        .iter()
                        .all(|(k, v)| line.contains(&format!("{k}=\"{v}\"")))
            })
            .unwrap();
        if status.starts_with('5') {
            assert!(line.contains(&format!("error_type=\"{status}\"")));
        } else {
            assert!(!line.contains("error_type"));
        }
    }
    let canceled = raw
        .lines()
        .find(|line| {
            line.starts_with("http_server_request_duration_seconds_count{")
                && line.contains("error_type=\"canceled\"")
        })
        .unwrap();
    assert!(canceled.contains("http_request_method=\"_OTHER\""));
    assert!(!canceled.contains("http_route="));
    assert!(!canceled.contains("http_response_status_code="));
    let unknown = raw
        .lines()
        .find(|line| {
            line.starts_with("http_server_request_duration_seconds_count{")
                && line.contains("http_request_method=\"HEAD\"")
        })
        .unwrap();
    assert!(!unknown.contains("http_route="));
    assert!(!unknown.contains("http_response_status_code="));
    assert!(!unknown.contains("error_type="));
    assert_sample(
        &raw,
        "http_server_request_duration_seconds_sum",
        &[
            ("error_type", "other"),
            ("http_response_status_code", "200"),
        ],
        0.75,
    );
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn database_attributes_normalize_backends_operations_and_omit_success_error() {
    let (provider, recorder) = setup();
    for (backend, _) in [
        (DbMetricBackend::Sqlite, "sqlite"),
        (DbMetricBackend::MySql, "mysql"),
        (DbMetricBackend::Postgres, "postgresql"),
        (DbMetricBackend::Other, "other"),
    ] {
        for kind in [
            DbQueryKind::Select,
            DbQueryKind::Insert,
            DbQueryKind::Update,
            DbQueryKind::Delete,
            DbQueryKind::Pragma,
            DbQueryKind::With,
            DbQueryKind::Transaction,
            DbQueryKind::Ddl,
            DbQueryKind::Other,
        ] {
            for failed in [false, true] {
                recorder.record_db_query(&DbQueryMetric::new(
                    backend,
                    kind,
                    failed,
                    Duration::from_millis(125),
                ));
            }
        }
    }
    let raw = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    for backend in ["sqlite", "mysql", "postgresql", "other"] {
        for (operation, count) in [
            ("SELECT", 1.0),
            ("INSERT", 1.0),
            ("UPDATE", 1.0),
            ("DELETE", 1.0),
            ("PRAGMA", 1.0),
            ("_OTHER", 4.0),
        ] {
            for failed in [false, true] {
                let lines: Vec<_> = raw
                    .lines()
                    .filter(|line| {
                        line.starts_with("db_client_operation_duration_seconds_count{")
                            && line.contains(&format!("db_system_name=\"{backend}\""))
                            && line.contains(&format!("db_operation_name=\"{operation}\""))
                            && line.contains("error_type=") == failed
                    })
                    .collect();
                assert_eq!(lines.len(), 1, "{backend} {operation} failed={failed}");
                let actual = lines[0].rsplit_once(' ').unwrap().1.parse::<f64>().unwrap();
                assert!(
                    (actual - count).abs() < f64::EPSILON,
                    "{backend} {operation} failed={failed}: expected {count}, got {actual}"
                );
                if failed {
                    assert!(lines[0].contains("error_type=\"other\""));
                }
            }
        }
    }
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn shared_events_preserve_counts_units_and_pending_is_an_absolute_gauge() {
    let (provider, recorder) = setup();
    recorder.record_auth_event("login", "ok", "password");
    recorder.record_application_event("config", "updated", "ok");
    recorder.record_config_reload("api", "reloaded", "ok", 3, 0.125);
    recorder.record_config_reload("api", "reloaded", "ok", 2, 0.25);
    recorder.record_config_mutation("api", "upsert", "ok", 4);
    recorder.record_background_task_transition("cleanup", "retry");
    recorder.record_background_task_transition("cleanup", "completed");
    recorder.set_background_tasks_pending(9);
    recorder.set_background_tasks_pending(2);
    recorder.record_external_operation("s3", "get", "ok", 0.5);
    let raw = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    for (name, attrs, value) in [
        (
            "aster_auth_events_total",
            vec![
                ("action", "login"),
                ("status", "ok"),
                ("reason", "password"),
            ],
            1.0,
        ),
        (
            "aster_application_events_total",
            vec![
                ("category", "config"),
                ("event", "updated"),
                ("status", "ok"),
            ],
            1.0,
        ),
        (
            "aster_config_reloads_total",
            vec![
                ("source", "api"),
                ("decision", "reloaded"),
                ("status", "ok"),
            ],
            2.0,
        ),
        ("aster_config_reload_duration_seconds_sum", vec![], 0.375),
        ("aster_config_reload_changed_keys_sum", vec![], 5.0),
        ("aster_config_reload_changed_keys_count", vec![], 2.0),
        (
            "aster_config_mutations_total",
            vec![("source", "api"), ("operation", "upsert"), ("status", "ok")],
            1.0,
        ),
        ("aster_config_mutation_changed_keys_sum", vec![], 4.0),
        (
            "aster_task_transitions_total",
            vec![("kind", "cleanup"), ("status", "retry")],
            1.0,
        ),
        (
            "aster_task_transitions_total",
            vec![("kind", "cleanup"), ("status", "completed")],
            1.0,
        ),
        ("aster_task_retries_total", vec![("kind", "cleanup")], 1.0),
        ("aster_task_pending", vec![], 2.0),
        (
            "aster_external_operations_total",
            vec![("system", "s3"), ("operation", "get"), ("status", "ok")],
            1.0,
        ),
        ("aster_external_operation_duration_seconds_sum", vec![], 0.5),
    ] {
        assert_sample(&raw, name, &attrs, value);
    }
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn readiness_pool_snapshots_and_local_task_handlers_have_independent_lifetimes() {
    let (provider, recorder) = setup();
    recorder.role_readiness(false);
    recorder.db_pool("writer", 2, 1, Some(8));
    recorder.db_pool("reader", 0, 0, None);
    recorder.task_handler_active("cleanup", 2);
    let active = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    assert_sample(&active, "aster_runtime_ready", &[], 0.0);
    assert_sample(
        &active,
        "aster_task_handler_active",
        &[("kind", "cleanup")],
        2.0,
    );
    assert!(
        !active.lines().any(
            |line| line.starts_with("db_client_connection_max{") && line.contains("\"reader\"")
        )
    );
    recorder.role_readiness(true);
    recorder.db_pool("writer", 1, 2, Some(8));
    recorder.task_handler_active("cleanup", -2);
    for result in ["returned", "error", "canceled"] {
        recorder.task_handler_finished("cleanup", result, 0.5);
    }
    let raw = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    assert_sample(&raw, "aster_runtime_ready", &[], 1.0);
    assert_sample(
        &raw,
        "aster_task_handler_active",
        &[("kind", "cleanup")],
        0.0,
    );
    assert_sample(
        &raw,
        "db_client_connection_count",
        &[
            ("db_client_connection_pool_name", "writer"),
            ("db_client_connection_state", "idle"),
        ],
        1.0,
    );
    assert_sample(
        &raw,
        "db_client_connection_count",
        &[
            ("db_client_connection_pool_name", "writer"),
            ("db_client_connection_state", "used"),
        ],
        2.0,
    );
    assert_sample(
        &raw,
        "db_client_connection_max",
        &[("db_client_connection_pool_name", "writer")],
        8.0,
    );
    for result in ["returned", "error", "canceled"] {
        assert_sample(
            &raw,
            "aster_task_handler_duration_seconds_count",
            &[("kind", "cleanup"), ("result", result)],
            1.0,
        );
        assert_sample(
            &raw,
            "aster_task_handler_duration_seconds_sum",
            &[("kind", "cleanup"), ("result", result)],
            0.5,
        );
    }
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[cfg(feature = "runtime-health")]
#[tokio::test]
async fn diagnostic_health_states_do_not_override_endpoint_readiness() {
    use aster_forge_runtime::{HealthComponentReport, SystemHealthReport};
    let (provider, recorder) = setup();
    recorder.role_readiness(true);
    for (component, label, value) in [
        (HealthComponentReport::healthy("db", "ok"), "healthy", 0.0),
        (
            HealthComponentReport::degraded("db", "fallback"),
            "degraded",
            1.0,
        ),
        (
            HealthComponentReport::unhealthy("db", "offline"),
            "unhealthy",
            2.0,
        ),
    ] {
        let report = SystemHealthReport::with_duration(
            vec![component.with_duration(Duration::from_millis(125))],
            Duration::from_millis(250),
        );
        recorder.record_health("diagnostics", &report);
        let raw = provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap();
        assert_sample(&raw, "aster_runtime_ready", &[], 1.0);
        assert_sample(
            &raw,
            "aster_runtime_health_status",
            &[("scope", "diagnostics")],
            value,
        );
        assert_sample(
            &raw,
            "aster_runtime_component_health_status",
            &[("scope", "diagnostics"), ("component", "db")],
            value,
        );
        assert_sample(
            &raw,
            "aster_runtime_health_duration_seconds_sum",
            &[("status", label)],
            0.25,
        );
        assert_sample(
            &raw,
            "aster_runtime_component_health_duration_seconds_sum",
            &[("status", label)],
            0.125,
        );
    }
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#![cfg(feature = "backend-prometheus")]
use aster_forge_metrics::*;
use std::fmt::Write;
use std::time::Duration;

#[tokio::test]
async fn actual_export_matches_reviewed_wire_contract_and_resource() {
    let provider = MetricsProvider::builder()
        .with_resource(MetricsResource {
            service_name: "contract".into(),
            service_version: "test".into(),
            environment: "test".into(),
            node_mode: "primary".into(),
            role: "api".into(),
            instance_id: "boot-1".into(),
        })
        .unwrap()
        .with_profile("example", "aster-metrics/v1")
        .unwrap()
        .with_prometheus()
        .unwrap()
        .build()
        .unwrap();
    let recorder = InfrastructureMetricsRecorder::new(provider.clone()).unwrap();
    recorder.record_http_request("GET", "http", "/files/{id}", 200, 0.125);
    recorder.record_db_query(&DbQueryMetric::new(
        DbMetricBackend::Postgres,
        DbQueryKind::Select,
        false,
        Duration::from_millis(2),
    ));
    recorder.role_readiness(true);
    recorder.db_pool("writer", 2, 1, Some(8));
    recorder.set_background_tasks_pending(3);
    recorder.set_background_tasks_oldest_age_seconds(2.5);
    recorder.record_task_lease_takeover("archive");
    let raw = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    for expected in [
        "# TYPE http_server_request_duration_seconds histogram",
        "# TYPE db_client_operation_duration_seconds histogram",
        "# TYPE db_client_connection_count gauge",
        "# TYPE aster_runtime_ready gauge",
        "aster_task_lease_takeovers_total{kind=\"archive\"",
        "# TYPE target_info gauge",
        "aster_metrics_schema_version=\"aster-metrics/v1\"",
        "aster_node_mode=\"primary\"",
        "aster_role=\"api\"",
        "db_system_name=\"postgresql\"",
        "http_route=\"/files/{id}\"",
    ] {
        assert!(raw.contains(expected), "missing {expected}: {raw}");
    }
    assert!(
        raw.lines()
            .any(|line| { line.starts_with("aster_task_pending{") && line.ends_with("} 3") })
    );
    assert!(raw.lines().any(|line| {
        line.starts_with("aster_task_pending_oldest_age_seconds{") && line.ends_with("} 2.5")
    }));
    assert!(!raw.contains("db_queries_total"));
    assert!(!raw.contains("process_cpu_milliseconds_total"));
    let mut output = String::new();
    for descriptor in provider.descriptors() {
        let name = descriptor.prometheus_name();
        writeln!(
            output,
            "{} {} {} {:?}",
            descriptor.name, name, descriptor.unit, descriptor.kind
        )
        .unwrap();
    }
    if std::env::var_os("ASTER_UPDATE_METRICS_CONTRACT").is_some() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/profile-v1.txt"),
            &output,
        )
        .unwrap();
    }
    let expected = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profile-v1.txt"
    ))
    .unwrap();
    // Keep one reviewed all-capabilities contract while exercising individual
    // exporter builds as well. Only omit entries whose capability is absent.
    let expected: String = expected
        .lines()
        .filter(|line| {
            (cfg!(feature = "allocator-metrics") || !line.starts_with("aster.process.heap "))
                && (cfg!(feature = "runtime-health")
                    || !(line.starts_with("aster.runtime.health.")
                        || line.starts_with("aster.runtime.component.health.")))
                && (cfg!(feature = "otlp") || !line.starts_with("aster.telemetry."))
        })
        .fold(String::new(), |mut output, line| {
            writeln!(output, "{line}").unwrap();
            output
        });
    assert_eq!(output, expected);
}

#![cfg(feature = "backend-prometheus")]
use aster_forge_metrics::*;
use std::time::Duration;

fn provider() -> MetricsProvider {
    MetricsProvider::builder()
        .with_prometheus()
        .unwrap()
        .build()
        .unwrap()
}

#[tokio::test]
async fn cumulative_snapshots_are_not_added_again_and_unknown_is_absent() {
    let provider = provider();
    let handles = provider
        .register_batch(&[
            MetricDescriptor::observable_counter("", "test.cpu.time", "CPU time.", &[])
                .with_unit("s"),
            MetricDescriptor::observable_up_down_counter(
                "",
                "test.memory.usage",
                "Resident bytes.",
                &[],
            )
            .with_unit("By"),
        ])
        .unwrap();
    let cpu = ProductSnapshotCounter::from_handle(handles[0].clone()).unwrap();
    let memory = ProductSnapshotUpDownCounter::from_handle(handles[1].clone()).unwrap();
    let before = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!before.contains("test_cpu_time"));
    cpu.set(&[], 12.5);
    memory.set(&[], 1024.0);
    for _ in 0..2 {
        let output = provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap();
        assert!(output.contains("# TYPE test_cpu_time_seconds_total counter"));
        assert!(output.contains("test_cpu_time_seconds_total 12.5"));
        assert!(output.contains("test_memory_usage_bytes 1024"));
    }
    assert_eq!(cpu.try_set(&[], 12.0), Err(MetricError::InvalidValue));
    memory.set(&[], 512.0);
    assert!(
        provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap()
            .contains("test_memory_usage_bytes 512")
    );
}

#[tokio::test]
async fn translated_name_and_attribute_collisions_are_rejected_atomically() {
    let provider = provider();
    let a = MetricDescriptor::counter("", "test.events", "Events.", &[]).with_unit("{event}");
    let b =
        MetricDescriptor::gauge("", "test_events_total", "Collision.", &[]).with_unit("{count}");
    assert_eq!(
        provider.register_batch(&[a, b]).err(),
        Some(MetricError::DuplicateRegistration)
    );
    assert!(provider.descriptors().is_empty());
    let invalid =
        MetricDescriptor::gauge("", "test.labels", "Labels.", &["http.route", "http_route"]);
    assert_eq!(
        provider.register_batch(&[invalid]).err(),
        Some(MetricError::InvalidDescriptor)
    );
}

#[tokio::test]
async fn descriptor_translation_matches_actual_exporter_for_supported_units() {
    let provider = provider();
    for (name, unit) in [
        ("days", "d"),
        ("hours", "h"),
        ("minutes", "min"),
        ("seconds", "s"),
        ("milliseconds", "ms"),
        ("microseconds", "us"),
        ("nanoseconds", "ns"),
        ("bytes", "By"),
        ("bytes_alias", "B"),
        ("kibibytes", "KiBy"),
        ("mebibytes", "MiBy"),
        ("gibibytes", "GiBy"),
        ("tebibytes", "TiBy"),
        ("kilobytes", "KBy"),
        ("kilobytes_alias", "KB"),
        ("megabytes", "MBy"),
        ("megabytes_alias", "MB"),
        ("gigabytes", "GBy"),
        ("gigabytes_alias", "GB"),
        ("terabytes", "TBy"),
        ("terabytes_alias", "TB"),
        ("meters", "m"),
        ("volts", "V"),
        ("amperes", "A"),
        ("joules", "J"),
        ("watts", "W"),
        ("grams", "g"),
        ("celsius", "Cel"),
        ("hertz", "Hz"),
        ("ratio", "1"),
        ("percent", "%"),
        ("events", "{event}"),
        ("second_rate", "By/s"),
        ("minute_rate", "By/m"),
        ("hour_rate", "By/h"),
        ("day_rate", "By/d"),
        ("week_rate", "By/w"),
        ("month_rate", "By/mo"),
        ("year_rate", "By/y"),
        ("ratio_rate", "1/s"),
        ("duration_rate", "ms/s"),
        ("event_rate", "{event}/s"),
        ("unknown_denominator", "By/unknown"),
    ] {
        let descriptor =
            MetricDescriptor::counter("units", name, "Unit translation.", &[]).with_unit(unit);
        let handle = provider.register_batch(&[descriptor]).unwrap().remove(0);
        ProductCounter::from_handle(handle).unwrap().inc(&[], 1);
        let raw = provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap();
        assert!(
            raw.contains(&format!("# TYPE {} counter", descriptor.prometheus_name())),
            "{unit}: {raw}"
        );
    }
}

#[tokio::test]
async fn unknown_snapshot_never_creates_a_false_zero_after_invalid_input() {
    let provider = provider();
    let descriptor =
        MetricDescriptor::observable_counter("", "test.snapshot", "Snapshot.", &["kind"])
            .with_unit("{event}");
    let handle = provider.register_batch(&[descriptor]).unwrap().remove(0);
    let counter = ProductSnapshotCounter::from_handle(handle).unwrap();
    assert_eq!(
        counter.try_set(&["invalid"], f64::NAN),
        Err(MetricError::InvalidValue)
    );
    let raw = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!raw.contains("test_snapshot_total"));
    counter.set(&["empty"], 0.0);
    assert!(
        provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap()
            .contains("test_snapshot_total{kind=\"empty\"} 0")
    );
}

#[tokio::test]
async fn active_guard_cancellation_and_unknown_route_keep_standard_boundaries() {
    let provider = provider();
    let recorder =
        std::sync::Arc::new(InfrastructureMetricsRecorder::new(provider.clone()).unwrap());
    let guard = HttpRequestGuard::new(recorder.clone(), "PRIVATE_METHOD", "http");
    let active = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    assert!(active.contains(
        "http_server_active_requests{http_request_method=\"_OTHER\",url_scheme=\"http\"} 1"
    ));
    drop(guard);
    recorder.record_http_request("PRIVATE_METHOD", "http", "", 404, 0.25);
    let output = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    assert!(output.contains(
        "http_server_active_requests{http_request_method=\"_OTHER\",url_scheme=\"http\"} 0"
    ));
    let sample = output
        .lines()
        .find(|line| line.starts_with("http_server_request_duration_seconds_count{"))
        .unwrap();
    assert!(!sample.contains("http_route"));
    assert!(!output.contains("http_requests_total"));
}

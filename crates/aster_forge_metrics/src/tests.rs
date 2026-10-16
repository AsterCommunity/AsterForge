use super::*;
#[cfg(feature = "backend-prometheus")]
use std::time::Duration;

product_metrics! {
    struct TestMetrics {
        bytes: counter("file", "bytes_total", "Observed bytes.", &["mode"]).with_unit("By"),
        active: gauge("file", "active", "Active streams.", &["mode"]),
        duration: histogram_with_buckets("file", "duration_seconds", "Stream duration.", &["mode"], &[0.1, 1.0]).with_unit("s"),
    }
}

#[test]
fn batch_validation_and_macro_registration_are_atomic() {
    let provider = MetricsProvider::builder().build().unwrap();
    let good = MetricDescriptor::counter("test", "events_total", "Events.", &[]);
    for invalid in [
        MetricDescriptor::counter("test", "1invalid", "Invalid.", &[]).with_unit(" "),
        MetricDescriptor::counter("test", "invalid", "", &[]),
        MetricDescriptor::counter("test", "invalid", "Invalid.", &["state", "state"]),
        MetricDescriptor::histogram_with_buckets("test", "duration", "Duration.", &[], &[1.0, 0.5]),
        MetricDescriptor::histogram_with_buckets("test", "duration", "Duration.", &[], &[f64::NAN]),
        MetricDescriptor::histogram_with_buckets("test", "duration", "Duration.", &[], &[-1.0]),
    ] {
        assert!(provider.register_batch(&[good, invalid]).is_err());
        assert!(provider.descriptors().is_empty());
    }
    let handles = provider.register_batch(&[good]).unwrap();
    assert!(ProductGauge::from_handle(handles[0].clone()).is_err());
    assert_eq!(provider.register_batch(&[good]).err(), Some(MetricError::DuplicateRegistration));
    let histogram = MetricDescriptor::histogram("other", "duration", "Duration.", &[]);
    let collision = MetricDescriptor::gauge("other", "duration_count", "Count.", &[]);
    assert_eq!(provider.register_batch(&[histogram, collision]).err(), Some(MetricError::DuplicateRegistration));
    assert_eq!(provider.descriptors().len(), 1);
    TestMetrics::register(&provider).unwrap();
    assert!(TestMetrics::register(&provider).is_err());
    assert_eq!(provider.descriptors().len(), 4);
}

#[test]
fn noop_accepts_the_same_schema_without_allocating_series() {
    let provider = MetricsProvider::builder().build().unwrap();
    assert!(!provider.enabled());
    let metrics = TestMetrics::register(&provider).unwrap();
    metrics.bytes.inc(&["direct"], 42);
    metrics.active.add(&["direct"], 1.0);
    metrics.duration.observe(&["direct"], 0.05);
    assert_eq!(provider.descriptors()[0].unit, "1"); // sorted by name
}

#[cfg(feature = "backend-prometheus")]
fn prometheus_provider() -> MetricsProvider {
    MetricsProvider::builder().with_prometheus().unwrap().build().unwrap()
}

#[cfg(feature = "backend-prometheus")]
#[tokio::test]
async fn instances_are_isolated_and_compatible_exposition_is_stable_across_scrapes() {
    let first = prometheus_provider();
    let second = prometheus_provider();
    let metrics = TestMetrics::register(&first).unwrap();
    TestMetrics::register(&second).unwrap();
    metrics.bytes.inc(&["direct"], 123);
    metrics.active.set(&["direct"], 3.0);
    metrics.active.add(&["direct"], -1.0);
    metrics.duration.observe(&["direct"], 0.25);
    for _ in 0..2 {
        let output = first.export_prometheus(Duration::from_secs(1)).await.unwrap();
        assert!(output.contains("# TYPE file_bytes_total counter"), "{output}");
        assert!(output.contains("file_bytes_total{mode=\"direct\"} 123"), "{output}");
        assert!(output.contains("file_active{mode=\"direct\"} 2"), "{output}");
        assert!(output.contains("file_duration_seconds_count{mode=\"direct\"} 1"), "{output}");
        assert!(output.contains("le=\"0.1\""), "{output}");
        assert!(!output.contains("target_info") && !output.contains("otel_scope"));
    }
    let output = second.export_prometheus(Duration::from_secs(1)).await.unwrap();
    assert!(!output.contains("123"));
    first.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(metrics.bytes.try_inc(&["direct"], 1), Err(MetricError::Closed));
    first.shutdown(Duration::from_secs(1)).await.unwrap();
    assert!(second.enabled());
}

#[cfg(feature = "backend-prometheus")]
#[tokio::test]
async fn value_labels_cardinality_and_size_boundaries_are_enforced() {
    let provider = MetricsProvider::builder().with_limits(MetricsLimits {
        max_series_per_instrument: 1, max_label_bytes: 6, max_exposition_bytes: 32,
        ..MetricsLimits::default()
    }).with_prometheus().unwrap().build().unwrap();
    let metrics = TestMetrics::register(&provider).unwrap();
    assert_eq!(metrics.bytes.try_inc(&[], 1), Err(MetricError::InvalidLabels));
    assert_eq!(metrics.bytes.try_inc(&["too-long"], 1), Err(MetricError::InvalidLabels));
    metrics.bytes.try_inc(&["direct"], u64::MAX).unwrap();
    assert_eq!(metrics.bytes.try_inc(&["direct"], 1), Err(MetricError::InvalidValue));
    assert_eq!(metrics.bytes.try_inc(&["remote"], 1), Err(MetricError::SeriesLimit));
    assert_eq!(metrics.active.try_set(&["direct"], f64::INFINITY), Err(MetricError::InvalidValue));
    assert_eq!(metrics.duration.try_observe(&["direct"], -1.0), Err(MetricError::InvalidValue));
    assert_eq!(provider.export_prometheus(Duration::from_secs(1)).await, Err(MetricError::Export));
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[cfg(feature = "backend-prometheus")]
#[tokio::test]
async fn concurrent_gauge_updates_and_counters_do_not_lose_observations() {
    let provider = prometheus_provider();
    let metrics = std::sync::Arc::new(TestMetrics::register(&provider).unwrap());
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let metrics = metrics.clone();
        tasks.push(std::thread::spawn(move || for _ in 0..1000 {
            metrics.bytes.inc(&["direct"], 1);
            metrics.active.add(&["direct"], 1.0);
        }));
    }
    for task in tasks { task.join().unwrap(); }
    let output = provider.export_prometheus(Duration::from_secs(1)).await.unwrap();
    assert!(output.contains("file_bytes_total{mode=\"direct\"} 8000"));
    assert!(output.contains("file_active{mode=\"direct\"} 8000"));
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[cfg(all(feature = "backend-prometheus", feature = "runtime-health"))]
#[tokio::test]
async fn shared_schema_and_sampler_remain_instance_owned() {
    let provider = prometheus_provider();
    let recorder = InfrastructureMetricsRecorder::new(provider.clone()).unwrap();
    recorder.record_http_request("GET", "/files/{id}", 200, 0.2);
    recorder.record_auth_event("login", "ok", "password");
    recorder.record_config_reload("api", "reloaded", "ok", 2, 0.01);
    recorder.record_config_mutation("api", "upsert", "ok", 1);
    recorder.record_background_task_transition("cleanup", "retry");
    recorder.set_background_tasks_pending(2);
    recorder.record_external_operation("s3", "get", "ok", 0.1);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(recorder.system_metrics_updater_task(shutdown.clone()).unwrap());
    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown.cancel(); task.await.unwrap();
    let output = provider.export_prometheus(Duration::from_secs(1)).await.unwrap();
    for name in ["http_requests_total", "auth_events_total", "config_reloads_total",
        "config_mutations_total", "background_tasks_total", "external_operations_total",
        "process_memory_rss_bytes", "process_cpu_milliseconds_total", "process_uptime_seconds"] {
        assert!(output.contains(name), "missing {name}: {output}");
    }
    assert!(output.contains("# TYPE process_cpu_milliseconds_total gauge"));
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

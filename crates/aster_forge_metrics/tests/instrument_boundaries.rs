use aster_forge_metrics::*;
use std::time::Duration;

product_metrics! {
    struct StateMetrics {
        active: up_down_counter("test", "active", "Active operations.", &["kind"]).with_unit("{operation}"),
        cumulative: observable_counter("test", "cumulative", "Cumulative snapshot.", &["kind"]).with_unit("{operation}"),
        current: observable_up_down_counter("test", "current", "Signed snapshot.", &["kind"]).with_unit("{operation}"),
    }
}

#[tokio::test]
async fn noop_state_instruments_skip_values_and_series_but_close_rejects_observations() {
    let provider = MetricsProvider::builder().build().unwrap();
    let metrics = StateMetrics::register(&provider).unwrap();
    // Disabled recording must not validate/allocate labels or inspect values.
    assert_eq!(metrics.active.try_add(&[], i64::MIN), Ok(()));
    assert_eq!(metrics.cumulative.try_set(&[], f64::NAN), Ok(()));
    assert_eq!(metrics.current.try_set(&[], f64::INFINITY), Ok(()));
    assert_eq!(provider.observation_failures(), 0);
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        metrics.active.try_add(&["task"], 1),
        Err(MetricError::Closed)
    );
    assert_eq!(
        metrics.cumulative.try_set(&["task"], 1.0),
        Err(MetricError::Closed)
    );
    assert_eq!(
        metrics.current.try_set(&["task"], 1.0),
        Err(MetricError::Closed)
    );
}

#[test]
fn state_handle_conversions_reject_other_instrument_kinds() {
    let provider = MetricsProvider::builder().build().unwrap();
    let handle = provider
        .register_batch(&[MetricDescriptor::counter("test", "events", "Events.", &[])])
        .unwrap()
        .remove(0);
    assert_eq!(
        ProductUpDownCounter::from_handle(handle.clone()).err(),
        Some(MetricError::WrongKind)
    );
    assert_eq!(
        ProductSnapshotCounter::from_handle(handle.clone()).err(),
        Some(MetricError::WrongKind)
    );
    assert_eq!(
        ProductSnapshotUpDownCounter::from_handle(handle).err(),
        Some(MetricError::WrongKind)
    );
}

#[cfg(feature = "backend-prometheus")]
#[tokio::test]
async fn invalid_integer_labels_do_not_consume_budget_or_create_snapshot_samples() {
    let provider = MetricsProvider::builder()
        .with_limits(MetricsLimits {
            max_total_series: 2,
            ..MetricsLimits::default()
        })
        .with_prometheus()
        .unwrap()
        .build()
        .unwrap();
    let descriptors = [
        MetricDescriptor::gauge("test", "typed", "Typed value.", &["status"])
            .with_unit("{state}")
            .with_integer_labels(&["status"]),
        MetricDescriptor::observable_up_down_counter(
            "test",
            "snapshot",
            "Typed snapshot.",
            &["status"],
        )
        .with_unit("{state}")
        .with_integer_labels(&["status"]),
    ];
    let handles = provider.register_batch(&descriptors).unwrap();
    let gauge = ProductGauge::from_handle(handles[0].clone()).unwrap();
    let snapshot = ProductSnapshotUpDownCounter::from_handle(handles[1].clone()).unwrap();
    for labels in [["bad"], ["9223372036854775808"]] {
        assert_eq!(gauge.try_set(&labels, 1.0), Err(MetricError::InvalidLabels));
        assert_eq!(
            snapshot.try_set(&labels, 1.0),
            Err(MetricError::InvalidLabels)
        );
    }
    gauge.try_set(&["200"], 2.0).unwrap();
    snapshot.try_set(&["206"], 3.0).unwrap();
    let raw = provider
        .export_prometheus(Duration::from_secs(1))
        .await
        .unwrap();
    assert!(raw.contains("test_typed{status=\"200\"} 2"));
    assert!(raw.contains("test_snapshot{status=\"206\"} 3"));
    assert!(!raw.contains("bad"));
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[cfg(feature = "backend-prometheus")]
#[tokio::test]
async fn state_values_and_recording_failures_preserve_the_last_good_samples() {
    let provider = MetricsProvider::builder()
        .with_prometheus()
        .unwrap()
        .build()
        .unwrap();
    let metrics = StateMetrics::register(&provider).unwrap();
    metrics.active.add(&["task"], 3);
    metrics.active.add(&["task"], -2);
    metrics.cumulative.set(&["task"], 4.0);
    metrics.cumulative.set(&["task"], 4.0);
    metrics.current.set(&["task"], 3.0);
    metrics.current.set(&["task"], -2.0);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            metrics.cumulative.try_set(&["task"], value),
            Err(MetricError::InvalidValue)
        );
        assert_eq!(
            metrics.current.try_set(&["task"], value),
            Err(MetricError::InvalidValue)
        );
    }
    assert_eq!(
        metrics.cumulative.try_set(&["task"], -1.0),
        Err(MetricError::InvalidValue)
    );
    assert_eq!(
        metrics.cumulative.try_set(&["task"], 3.0),
        Err(MetricError::InvalidValue)
    );
    assert_eq!(
        metrics.active.try_add(&[], 1),
        Err(MetricError::InvalidLabels)
    );
    assert_eq!(
        metrics.cumulative.try_set(&[], 5.0),
        Err(MetricError::InvalidLabels)
    );
    assert_eq!(
        metrics.current.try_set(&[], 5.0),
        Err(MetricError::InvalidLabels)
    );
    assert_eq!(
        provider.observation_failures(),
        0,
        "try APIs report to the caller"
    );
    metrics.active.add(&[], 1);
    metrics.cumulative.set(&["task"], 0.0);
    metrics.current.set(&["task"], f64::NAN);
    assert_eq!(
        provider.observation_failures(),
        3,
        "convenience APIs diagnose rejected samples"
    );
    for _ in 0..2 {
        let raw = provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap();
        for line in [
            "test_active{kind=\"task\"} 1",
            "test_cumulative_total{kind=\"task\"} 4",
            "test_current{kind=\"task\"} -2",
        ] {
            assert!(
                raw.lines().any(|sample| sample == line),
                "missing {line}: {raw}"
            );
        }
    }
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[cfg(feature = "backend-prometheus")]
#[tokio::test]
async fn state_series_share_the_instance_budget_and_existing_series_remain_writable() {
    let provider = MetricsProvider::builder()
        .with_limits(MetricsLimits {
            max_total_series: 2,
            ..MetricsLimits::default()
        })
        .with_prometheus()
        .unwrap()
        .build()
        .unwrap();
    let metrics = StateMetrics::register(&provider).unwrap();
    metrics.active.try_add(&["task"], 1).unwrap();
    metrics.cumulative.try_set(&["task"], 1.0).unwrap();
    assert_eq!(
        metrics.current.try_set(&["task"], 1.0),
        Err(MetricError::SeriesLimit)
    );
    metrics.active.try_add(&["task"], -1).unwrap();
    metrics.cumulative.try_set(&["task"], 2.0).unwrap();
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
}

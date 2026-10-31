use aster_forge_metrics::*;
use std::time::Duration;

#[test]
fn duplicate_otel_names_reject_different_kinds_and_units_without_catalog_changes() {
    let counter = MetricDescriptor::counter("test", "events", "Events.", &[]).with_unit("{event}");
    for conflicting in [
        MetricDescriptor::gauge("test", "events", "Current events.", &[]).with_unit("{event}"),
        MetricDescriptor::counter("test", "events", "Bytes.", &[]).with_unit("By"),
        MetricDescriptor::observable_counter("test", "events", "Snapshot.", &[]).with_unit("s"),
    ] {
        assert_ne!(counter.prometheus_name(), conflicting.prometheus_name());
        let provider = MetricsProvider::builder().build().unwrap();
        assert_eq!(
            provider.register_batch(&[counter, conflicting]).err(),
            Some(MetricError::DuplicateRegistration)
        );
        assert!(provider.descriptors().is_empty());
        provider.register_batch(&[counter]).unwrap();
        let fresh = MetricDescriptor::gauge("test", "fresh", "Fresh.", &[]);
        assert_eq!(
            provider.register_batch(&[fresh, conflicting]).err(),
            Some(MetricError::DuplicateRegistration)
        );
        assert_eq!(provider.descriptors(), vec![counter]);
        provider.register_batch(&[fresh]).unwrap();
        assert_eq!(provider.descriptors().len(), 2);
    }
}

#[test]
fn integer_label_schema_must_be_unique_and_a_subset_of_declared_keys() {
    let provider = MetricsProvider::builder().build().unwrap();
    for labels in [&["missing"][..], &["status", "status"]] {
        let descriptor = MetricDescriptor::gauge("test", "status", "Status.", &["status"])
            .with_integer_labels(labels);
        assert_eq!(
            provider.register_batch(&[descriptor]).err(),
            Some(MetricError::InvalidDescriptor)
        );
        assert!(provider.descriptors().is_empty());
    }
}

#[test]
fn profile_rejects_missing_oversized_and_control_character_identity() {
    for (namespace, version) in [
        ("", "v1"),
        (" ", "v1"),
        ("aster", ""),
        ("aster", "\t"),
        ("aster\n", "v1"),
        ("aster", "v1\0"),
        (&"a".repeat(129), "v1"),
        ("aster", &"v".repeat(129)),
    ] {
        assert_eq!(
            MetricsProvider::builder()
                .with_profile(namespace, version)
                .err(),
            Some(MetricError::InvalidConfiguration)
        );
    }
    MetricsProvider::builder()
        .with_profile(&"a".repeat(128), &"v".repeat(128))
        .unwrap()
        .build()
        .unwrap();
}

#[test]
fn reserved_resource_labels_and_translated_attribute_collisions_are_atomic() {
    let provider = MetricsProvider::builder().build().unwrap();
    let good = MetricDescriptor::counter("test", "events", "Events.", &[]);
    for labels in [
        &["le"][..],
        &["job"],
        &["instance"],
        &["service.name"],
        &["service_name"],
        &["service.version"],
        &["service.namespace"],
        &["service.instance.id"],
        &["deployment.environment.name"],
        &["aster.role"],
        &["aster.node.mode"],
        &["aster_node_mode"],
        &["aster.metrics.schema.version"],
        &["http.route", "http_route"],
    ] {
        let invalid = MetricDescriptor::gauge("test", "labels", "Invalid labels.", labels);
        assert_eq!(
            provider.register_batch(&[good, invalid]).err(),
            Some(MetricError::InvalidDescriptor)
        );
        assert!(provider.descriptors().is_empty());
    }
    for name in ["target_info", "target.info"] {
        for unit in ["1", "{target}"] {
            let invalid =
                MetricDescriptor::gauge("", name, "Reserved family.", &[]).with_unit(unit);
            assert_eq!(
                provider.register_batch(&[invalid]).err(),
                Some(MetricError::InvalidDescriptor)
            );
        }
    }
    provider.register_batch(&[good]).unwrap();
}

#[tokio::test]
async fn instrument_budget_rejection_preserves_catalog_and_shutdown_blocks_new_schema() {
    let provider = MetricsProvider::builder()
        .with_limits(MetricsLimits {
            max_instruments: 1,
            ..MetricsLimits::default()
        })
        .build()
        .unwrap();
    let a = MetricDescriptor::counter("test", "first", "First.", &[]);
    let b = MetricDescriptor::gauge("test", "second", "Second.", &[]);
    assert_eq!(
        provider.register_batch(&[a, b]).err(),
        Some(MetricError::InstrumentLimit)
    );
    assert!(provider.descriptors().is_empty());
    provider.register_batch(&[a]).unwrap();
    assert_eq!(
        provider.register_batch(&[b]).err(),
        Some(MetricError::InstrumentLimit)
    );
    assert_eq!(provider.descriptors(), vec![a]);
    provider.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        provider.register_batch(&[b]).err(),
        Some(MetricError::Closed)
    );
    assert_eq!(
        provider.force_flush(Duration::from_secs(1)).await,
        Err(MetricError::Closed)
    );
}

#[cfg(feature = "backend-prometheus")]
#[tokio::test]
async fn profile_resource_merge_retains_all_explicit_identity_and_omits_empty_fields() {
    for profile_first in [false, true] {
        let builder = MetricsProvider::builder();
        let builder = if profile_first {
            builder.with_profile("aster", "aster-metrics/v1").unwrap()
        } else {
            builder
        };
        let builder = builder
            .with_resource(MetricsResource {
                service_name: "example".into(),
                service_version: "0.1.0".into(),
                instance_id: "boot-1".into(),
                ..MetricsResource::default()
            })
            .unwrap();
        let builder = if profile_first {
            builder
        } else {
            builder.with_profile("aster", "aster-metrics/v1").unwrap()
        };
        let provider = builder.with_prometheus().unwrap().build().unwrap();
        let handle = provider
            .register_batch(&[
                MetricDescriptor::counter("test", "events", "Events.", &[]).with_unit("{event}")
            ])
            .unwrap()
            .remove(0);
        ProductCounter::from_handle(handle).unwrap().inc(&[], 1);
        let raw = provider
            .export_prometheus(Duration::from_secs(1))
            .await
            .unwrap();
        let sample = raw
            .lines()
            .find(|line| line.starts_with("test_events_total{"))
            .unwrap();
        for label in [
            "service_name=\"example\"",
            "service_version=\"0.1.0\"",
            "service_instance_id=\"boot-1\"",
            "service_namespace=\"aster\"",
            "aster_metrics_schema_version=\"aster-metrics/v1\"",
        ] {
            assert!(sample.contains(label), "missing {label}: {sample}");
        }
        assert!(!sample.contains("deployment_environment_name="));
        assert!(!sample.contains("aster_role="));
        assert!(!sample.contains("aster_node_mode="));
        provider.shutdown(Duration::from_secs(1)).await.unwrap();
        assert_eq!(
            provider.export_prometheus(Duration::from_secs(1)).await,
            Err(MetricError::Closed)
        );
    }
    let noop = MetricsProvider::builder().build().unwrap();
    assert_eq!(
        noop.export_prometheus(Duration::from_secs(1)).await,
        Err(MetricError::Unavailable)
    );
}

#[test]
fn node_mode_identity_is_bounded_and_rejects_control_characters() {
    for node_mode in [
        "primary\n".to_string(),
        "follower\0".to_string(),
        "n".repeat(129),
    ] {
        assert!(matches!(
            MetricsProvider::builder().with_resource(MetricsResource {
                service_name: "example".into(),
                node_mode,
                ..Default::default()
            }),
            Err(MetricError::InvalidConfiguration)
        ));
    }
    for node_mode in [String::new(), "n".repeat(128)] {
        MetricsProvider::builder()
            .with_resource(MetricsResource {
                service_name: "example".into(),
                node_mode,
                ..Default::default()
            })
            .unwrap();
    }
}

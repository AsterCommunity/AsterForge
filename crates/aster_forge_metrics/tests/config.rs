//! Tests for validated observability configuration assembly.

use aster_forge_config::ValidatableConfig;
use aster_forge_metrics::{
    MetricsIdentity, MetricsProvider, ObservabilityConfig, ObservabilityConfigSource,
};

fn identity() -> MetricsIdentity {
    MetricsIdentity {
        service_name: "example".into(),
        service_version: "0.1.0".into(),
        profile_namespace: "com.astercosm.example".into(),
        role: "api".into(),
        node_mode: None,
        instance_id: "boot-1".into(),
    }
}

#[test]
fn disabled_exporters_do_not_require_deployment_identity() {
    let config = ObservabilityConfig::default();
    assert_eq!(config.environment, "production");
    assert!(ValidatableConfig::validate(&config).is_ok());
    let provider = config.create_provider(identity()).unwrap();
    assert!(!provider.enabled());
}

#[test]
fn enabled_uncompiled_exporter_is_rejected() {
    let mut config = ObservabilityConfig::default();
    config.prometheus.enabled = true;
    if !cfg!(feature = "backend-prometheus") {
        assert!(ValidatableConfig::validate(&config).is_err());
        assert!(config.create_provider(identity()).is_err());
    }

    config.prometheus.enabled = false;
    config.otlp_metrics.enabled = true;
    if !cfg!(feature = "otlp") {
        assert!(ValidatableConfig::validate(&config).is_err());
    }
}

#[test]
fn source_constructor_rejects_invalid_identity_before_building_provider() {
    let mut config = ObservabilityConfig::default();
    config.prometheus.enabled = cfg!(feature = "backend-prometheus");
    if config.prometheus.enabled {
        let mut invalid = identity();
        invalid.profile_namespace = "aster".into();
        let source = TestSource {
            config,
            identity: invalid,
        };
        assert!(MetricsProvider::create_from_source(&source).is_err());
    }
}

struct TestSource {
    config: ObservabilityConfig,
    identity: MetricsIdentity,
}

impl ObservabilityConfigSource for TestSource {
    fn validate(&self) -> aster_forge_metrics::MetricResult<()> {
        ValidatableConfig::validate(&self.config)
            .map_err(|_| aster_forge_metrics::MetricError::InvalidConfiguration)
    }

    fn environment(&self) -> &str {
        &self.config.environment
    }

    fn prometheus(&self) -> Option<aster_forge_metrics::PrometheusSettings> {
        Some(self.config.prometheus.clone())
    }

    fn otlp_metrics(&self) -> Option<aster_forge_metrics::OtlpMetricsSettings> {
        Some(self.config.otlp_metrics.clone())
    }

    fn identity(&self) -> MetricsIdentity {
        self.identity.clone()
    }
}

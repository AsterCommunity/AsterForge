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

#[test]
fn identity_validation_reports_the_invalid_field() {
    for (field, mutate) in [
        ("service_name", 0),
        ("service_version", 1),
        ("profile_namespace", 2),
        ("role", 3),
        ("instance_id", 4),
        ("node_mode", 5),
    ] {
        let mut invalid = identity();
        match mutate {
            0 => invalid.service_name.clear(),
            1 => invalid.service_version.clear(),
            2 => invalid.profile_namespace.clear(),
            3 => invalid.role.clear(),
            4 => invalid.instance_id.clear(),
            _ => invalid.node_mode = Some("\n".into()),
        }
        let error = ValidatableConfig::validate(&invalid).unwrap_err();
        assert!(error.to_string().contains(&format!("identity.{field}")));
        let provider_error = MetricsProvider::create_from_source(&TestSource {
            config: ObservabilityConfig {
                prometheus: aster_forge_metrics::PrometheusSettings {
                    enabled: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            identity: invalid.clone(),
        })
        .err()
        .unwrap();
        if cfg!(feature = "backend-prometheus") {
            assert!(
                provider_error
                    .to_string()
                    .contains(&format!("identity.{field}"))
            );
        }
        let loader_error = aster_forge_config::load_config(|| {
            Ok::<_, std::convert::Infallible>(IdentityRoot { identity: invalid })
        })
        .err()
        .unwrap();
        assert!(
            loader_error
                .to_string()
                .contains(&format!("identity.{field}"))
        );
    }
}

struct TestSource {
    config: ObservabilityConfig,
    identity: MetricsIdentity,
}

impl ObservabilityConfigSource for TestSource {
    fn validate(&self) -> aster_forge_metrics::MetricResult<()> {
        ValidatableConfig::validate(&self.config).map_err(|error| {
            aster_forge_metrics::MetricError::InvalidConfiguration(error.to_string())
        })
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

#[derive(aster_forge_config::ValidateConfig)]
struct IdentityRoot {
    identity: MetricsIdentity,
}
impl ValidatableConfig for IdentityRoot {
    fn validate(&self) -> aster_forge_config::Result<()> {
        Ok(())
    }
}

#[test]
fn provider_preserves_section_diagnostics() {
    let config = ObservabilityConfig {
        environment: "\n".into(),
        prometheus: aster_forge_metrics::PrometheusSettings {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let expected = config.validate().unwrap_err().to_string();
    let actual = config.create_provider(identity()).err().unwrap();
    assert!(actual.to_string().contains(&expected));
    if cfg!(feature = "backend-prometheus") {
        assert!(expected.contains("observability.environment"));
    } else {
        assert!(expected.contains("backend-prometheus"));
    }
}

#[test]
fn identity_valid_boundaries_preserve_optional_node_mode_semantics() {
    for node_mode in [None, Some(String::new()), Some("n".repeat(128))] {
        let mut valid = identity();
        valid.node_mode = node_mode;
        assert!(valid.validate().is_ok());
    }
    let mut invalid = identity();
    invalid.service_name = "s".repeat(129);
    assert!(
        invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("identity.service_name")
    );
}

#[cfg(feature = "otlp")]
#[test]
fn otlp_configuration_retains_field_diagnostics_without_secret_values() {
    let mut config = ObservabilityConfig::default();
    config.otlp_metrics.enabled = true;
    config.otlp_metrics.timeout_seconds = 0;
    let message = config
        .create_provider(identity())
        .err()
        .unwrap()
        .to_string();
    assert!(message.contains("otlp_metrics.timeout"));
    config.otlp_metrics.timeout_seconds = 5;
    config.otlp_metrics.endpoint = "https://user:do-not-expose@collector.test/v1/metrics".into();
    let message = config
        .create_provider(identity())
        .err()
        .unwrap()
        .to_string();
    assert!(message.contains("otlp_metrics.endpoint"));
    assert!(!message.contains("do-not-expose"));
}

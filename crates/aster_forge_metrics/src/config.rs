//! Shared observability configuration and validated provider assembly.

use std::collections::BTreeMap;

use aster_forge_config::{ConfigCoreError, Result as ConfigResult, ValidatableConfig};
use serde::{Deserialize, Serialize};

#[cfg(feature = "otlp")]
use crate::OtlpMetricsConfig;
use crate::{MetricError, MetricResult, MetricsProvider, MetricsResource, SharedMetricsRecorder};

/// Product identity attached to the metrics resource and profile.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsIdentity {
    /// Stable service name.
    pub service_name: String,
    /// Service build version.
    pub service_version: String,
    /// Reverse-DNS profile namespace.
    pub profile_namespace: String,
    /// Process role.
    pub role: String,
    /// Optional product node mode.
    #[serde(default)]
    pub node_mode: Option<String>,
    /// Stable process instance ID.
    pub instance_id: String,
}

/// Prometheus exporter settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrometheusSettings {
    /// Enables the pull reader when the crate feature is compiled.
    pub enabled: bool,
    /// Optional product-owned HTTP authentication settings.
    pub basic_auth: Option<MetricsBasicAuthConfig>,
}

/// Product-neutral Prometheus basic-auth material.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsBasicAuthConfig {
    /// Whether the route requires authentication.
    pub enabled: bool,
    /// HTTP Basic username.
    pub username: String,
    /// Argon2id PHC password hash.
    pub password_hash: String,
}

impl std::fmt::Debug for MetricsBasicAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetricsBasicAuthConfig")
            .field("enabled", &self.enabled)
            .field("username", &"[REDACTED]")
            .field("password_hash", &"[REDACTED]")
            .finish()
    }
}

/// OTLP metrics exporter settings.
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct OtlpMetricsSettings {
    /// Enables the push reader when the crate feature is compiled.
    pub enabled: bool,
    /// Full OTLP metrics endpoint.
    pub endpoint: String,
    /// Static export headers.
    pub headers: BTreeMap<String, String>,
    /// Export interval in seconds.
    pub interval_seconds: u64,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
}

impl Default for OtlpMetricsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:4318/v1/metrics".into(),
            headers: BTreeMap::new(),
            interval_seconds: 60,
            timeout_seconds: 5,
        }
    }
}

impl std::fmt::Debug for OtlpMetricsSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OtlpMetricsSettings")
            .field("enabled", &self.enabled)
            .field("endpoint", &"[REDACTED]")
            .field("headers", &"[REDACTED]")
            .field("interval_seconds", &self.interval_seconds)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

/// Product-neutral observability configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ObservabilityConfig {
    /// Deployment environment.
    pub environment: String,
    /// Prometheus settings.
    pub prometheus: PrometheusSettings,
    /// OTLP metrics settings.
    pub otlp_metrics: OtlpMetricsSettings,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            environment: "production".into(),
            prometheus: PrometheusSettings::default(),
            otlp_metrics: OtlpMetricsSettings::default(),
        }
    }
}

/// Input view used by the validated provider constructor.
pub trait ObservabilityConfigSource {
    /// Validates the source before any provider construction.
    ///
    /// # Errors
    /// Returns a configuration error when an enabled exporter or identity is invalid.
    fn validate(&self) -> MetricResult<()>;
    /// Deployment environment.
    fn environment(&self) -> &str;
    /// Prometheus settings.
    fn prometheus(&self) -> Option<PrometheusSettings>;
    /// OTLP settings.
    fn otlp_metrics(&self) -> Option<OtlpMetricsSettings>;
    /// Metrics identity.
    fn identity(&self) -> MetricsIdentity;
}

impl ValidatableConfig for ObservabilityConfig {
    fn validate(&self) -> ConfigResult<()> {
        if self.prometheus.enabled && !cfg!(feature = "backend-prometheus") {
            return Err(ConfigCoreError::invalid_value(
                "Prometheus metrics require the backend-prometheus feature",
            ));
        }
        if self.otlp_metrics.enabled && !cfg!(feature = "otlp") {
            return Err(ConfigCoreError::invalid_value(
                "OTLP metrics require the otlp feature",
            ));
        }
        let prometheus_enabled = self.prometheus.enabled && cfg!(feature = "backend-prometheus");
        let otlp_enabled = self.otlp_metrics.enabled && cfg!(feature = "otlp");
        if self
            .prometheus
            .basic_auth
            .as_ref()
            .is_some_and(|auth| auth.enabled)
            && !self.prometheus.enabled
        {
            return Err(ConfigCoreError::invalid_value(
                "observability.prometheus.basic_auth requires the Prometheus endpoint",
            ));
        }
        if !prometheus_enabled && !otlp_enabled {
            return Ok(());
        }
        if self.environment.trim().is_empty()
            || self.environment.len() > 128
            || self.environment.chars().any(char::is_control)
        {
            return Err(ConfigCoreError::invalid_value(
                "observability.environment is invalid",
            ));
        }
        if let Some(auth) = self
            .prometheus
            .basic_auth
            .as_ref()
            .filter(|auth| auth.enabled)
        {
            if !self.prometheus.enabled {
                return Err(ConfigCoreError::invalid_value(
                    "observability.prometheus.basic_auth requires the Prometheus endpoint",
                ));
            }
            #[cfg(feature = "basic-auth")]
            validate_basic_auth(auth)?;
            #[cfg(not(feature = "basic-auth"))]
            {
                let _ = auth;
                return Err(ConfigCoreError::invalid_value(
                    "observability basic-auth requires the basic-auth feature",
                ));
            }
        }
        #[cfg(feature = "otlp")]
        if otlp_enabled {
            let transport = self.otlp_transport();
            transport.validate().map_err(|_| {
                ConfigCoreError::invalid_value("invalid observability OTLP configuration")
            })?;
        }
        Ok(())
    }
}

impl ObservabilityConfig {
    /// Builds a provider for an explicit product identity.
    ///
    /// # Errors
    /// Returns a validation or exporter initialization error.
    pub fn create_provider(&self, identity: MetricsIdentity) -> MetricResult<MetricsProvider> {
        struct Source<'a> {
            config: &'a ObservabilityConfig,
            identity: MetricsIdentity,
        }
        impl ObservabilityConfigSource for Source<'_> {
            fn validate(&self) -> MetricResult<()> {
                ValidatableConfig::validate(self.config)
                    .map_err(|_| MetricError::InvalidConfiguration)
            }
            fn environment(&self) -> &str {
                &self.config.environment
            }
            fn prometheus(&self) -> Option<PrometheusSettings> {
                Some(self.config.prometheus.clone())
            }
            fn otlp_metrics(&self) -> Option<OtlpMetricsSettings> {
                Some(self.config.otlp_metrics.clone())
            }
            fn identity(&self) -> MetricsIdentity {
                self.identity.clone()
            }
        }
        MetricsProvider::create_from_source(&Source {
            config: self,
            identity,
        })
    }

    #[cfg(feature = "otlp")]
    fn otlp_transport(&self) -> crate::OtlpMetricsConfig {
        crate::OtlpMetricsConfig {
            endpoint: self.otlp_metrics.endpoint.clone(),
            headers: self.otlp_metrics.headers.clone(),
            timeout: std::time::Duration::from_secs(self.otlp_metrics.timeout_seconds),
            interval: std::time::Duration::from_secs(self.otlp_metrics.interval_seconds),
            max_request_bytes: 16 * 1024 * 1024,
            max_response_bytes: 64 * 1024,
        }
    }
}

impl MetricsProvider {
    /// Validates a source before constructing any exporter or provider.
    ///
    /// # Errors
    /// Returns a source validation, identity, exporter initialization, or provider construction error.
    pub fn create_from_source<S: ObservabilityConfigSource>(
        source: &S,
    ) -> MetricResult<MetricsProvider> {
        source.validate()?;
        let prometheus = source.prometheus();
        let otlp = source.otlp_metrics();
        let shared = ObservabilityConfig {
            environment: source.environment().to_owned(),
            prometheus: prometheus.clone().unwrap_or_default(),
            otlp_metrics: otlp.clone().unwrap_or_default(),
        };
        ValidatableConfig::validate(&shared).map_err(|_| MetricError::InvalidConfiguration)?;
        if prometheus
            .as_ref()
            .is_some_and(|settings| settings.enabled && !cfg!(feature = "backend-prometheus"))
        {
            return Err(MetricError::InvalidConfiguration);
        }
        let prometheus_enabled = prometheus.as_ref().is_some_and(|settings| settings.enabled);
        if otlp
            .as_ref()
            .is_some_and(|settings| settings.enabled && !cfg!(feature = "otlp"))
        {
            return Err(MetricError::InvalidConfiguration);
        }
        let otlp_enabled = otlp.as_ref().is_some_and(|settings| settings.enabled);
        if !prometheus_enabled && !otlp_enabled {
            return MetricsProvider::builder().build();
        }
        let identity = source.identity();
        validate_identity(&identity)?;
        let builder = MetricsProvider::builder().with_resource(MetricsResource {
            service_name: identity.service_name.clone(),
            service_version: identity.service_version.clone(),
            environment: shared.environment,
            node_mode: identity.node_mode.clone().unwrap_or_default(),
            role: identity.role.clone(),
            instance_id: identity.instance_id.clone(),
        })?;
        #[cfg(feature = "backend-prometheus")]
        let builder = if prometheus_enabled {
            builder.with_prometheus()?
        } else {
            builder
        };
        #[cfg(feature = "otlp")]
        let builder = if otlp_enabled {
            let settings = otlp.ok_or(MetricError::InvalidConfiguration)?;
            let transport = OtlpMetricsConfig {
                endpoint: settings.endpoint,
                headers: settings.headers,
                timeout: std::time::Duration::from_secs(settings.timeout_seconds),
                interval: std::time::Duration::from_secs(settings.interval_seconds),
                max_request_bytes: 16 * 1024 * 1024,
                max_response_bytes: 64 * 1024,
            };
            builder.with_otlp(transport)?.0
        } else {
            builder
        };
        builder
            .with_profile(&identity.profile_namespace, crate::METRICS_PROFILE_VERSION)?
            .build()
    }

    /// Creates a provider and shared recorder through the same validation path.
    ///
    /// # Errors
    /// Returns the same validation and provider errors as [`Self::create_from_source`].
    pub fn create_recorder<S: ObservabilityConfigSource>(
        source: &S,
    ) -> MetricResult<(MetricsProvider, SharedMetricsRecorder)> {
        let provider = Self::create_from_source(source)?;
        if provider.enabled() {
            let recorder = crate::InfrastructureMetricsRecorder::new(provider.clone())?;
            Ok((provider, std::sync::Arc::new(recorder)))
        } else {
            Ok((provider, crate::NoopMetrics::arc()))
        }
    }
}

fn validate_identity(identity: &MetricsIdentity) -> MetricResult<()> {
    for value in [
        &identity.service_name,
        &identity.service_version,
        &identity.profile_namespace,
        &identity.role,
        &identity.instance_id,
    ]
    .into_iter()
    .chain(
        identity
            .node_mode
            .as_ref()
            .filter(|value| !value.is_empty()),
    ) {
        if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
            return Err(MetricError::InvalidConfiguration);
        }
    }
    if identity.profile_namespace.trim() != identity.profile_namespace
        || !identity.profile_namespace.contains('.')
    {
        return Err(MetricError::InvalidConfiguration);
    }
    Ok(())
}

#[cfg(feature = "basic-auth")]
fn validate_basic_auth(config: &MetricsBasicAuthConfig) -> ConfigResult<()> {
    if config.username.trim().is_empty()
        || config.username.contains(':')
        || config.username.chars().any(char::is_control)
        || config.username.len() > 256
        || config.password_hash.len() > 1024
    {
        return Err(ConfigCoreError::invalid_value(
            "invalid observability basic-auth",
        ));
    }
    aster_forge_crypto::validate_password_hash(
        &config.password_hash,
        aster_forge_crypto::PasswordHashVerificationLimits::default(),
    )
    .map_err(|_| ConfigCoreError::invalid_value("invalid observability basic-auth"))
}

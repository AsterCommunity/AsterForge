//! Runtime health checks for cache backends.
//!
//! Cache construction can intentionally fall back to memory when a configured
//! remote backend is unavailable. The shared health check reports both states:
//! degraded when the active backend differs from static configuration, and
//! unhealthy when the active backend's own lightweight probe fails.

use std::sync::Arc;
use std::time::Duration;

use aster_forge_runtime::{
    HealthCheckOptions, HealthCheckScopes, HealthComponentReport, RuntimeComponentKind,
    RuntimeComponentRegistry,
};

use crate::{CacheBackend, CacheConfig};

/// Stable component name used for cache diagnostics.
pub const CACHE_COMPONENT: &str = "cache";
/// Stable health check name used for cache diagnostics.
pub const CACHE_HEALTH_CHECK: &str = "cache";
/// Default timeout for cache diagnostics checks.
pub const CACHE_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Registers cache diagnostics health checks.
pub fn register_cache_health_check(
    registry: &mut RuntimeComponentRegistry,
    config: CacheConfig,
    cache: Arc<dyn CacheBackend>,
) {
    registry.component_health_with_options(
        CACHE_COMPONENT,
        RuntimeComponentKind::Cache,
        CACHE_HEALTH_CHECK,
        cache_health_options(),
        move || {
            let config = config.clone();
            let cache = cache.clone();
            async move { check_cache_component(&config, cache.as_ref()).await }
        },
    );
}

/// Returns the standard cache health check options.
pub fn cache_health_options() -> HealthCheckOptions {
    HealthCheckOptions::optional(Some(CACHE_HEALTH_CHECK_TIMEOUT))
        .with_scopes(HealthCheckScopes::diagnostics())
}

/// Runs the standard cache backend health check.
pub async fn check_cache_component(
    config: &CacheConfig,
    cache: &dyn CacheBackend,
) -> HealthComponentReport {
    if config.backend != cache.backend_name() {
        tracing::debug!(
            configured_backend = %config.backend,
            active_backend = cache.backend_name(),
            "cache backend is using fallback"
        );
        return HealthComponentReport::degraded(
            CACHE_HEALTH_CHECK,
            format!(
                "configured cache backend '{}' is using active backend '{}'",
                config.backend,
                cache.backend_name()
            ),
        )
        .with_detail("configured_backend", config.backend.clone())
        .with_detail("active_backend", cache.backend_name());
    }

    match cache.health_check().await {
        Ok(()) => {
            tracing::debug!(
                backend = cache.backend_name(),
                "cache health check succeeded"
            );
            HealthComponentReport::healthy(CACHE_HEALTH_CHECK, "cache health check succeeded")
                .with_detail("active_backend", cache.backend_name())
        }
        Err(error) => {
            tracing::debug!(backend = cache.backend_name(), error = %error, "cache health check failed");
            HealthComponentReport::unhealthy(
                CACHE_HEALTH_CHECK,
                format!(
                    "cache backend '{}' health check failed: {error}",
                    cache.backend_name()
                ),
            )
            .with_detail("active_backend", cache.backend_name())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aster_forge_runtime::{HealthComponentDetailValue, HealthStatus};
    use async_trait::async_trait;

    use super::{check_cache_component, register_cache_health_check};
    use crate::{CacheBackend, CacheConfig};

    struct FakeCache {
        backend_name: &'static str,
        healthy: bool,
    }

    impl FakeCache {
        const fn new(backend_name: &'static str) -> Self {
            Self {
                backend_name,
                healthy: true,
            }
        }

        const fn unhealthy(backend_name: &'static str) -> Self {
            Self {
                backend_name,
                healthy: false,
            }
        }
    }

    #[async_trait]
    impl CacheBackend for FakeCache {
        fn backend_name(&self) -> &'static str {
            self.backend_name
        }

        async fn health_check(&self) -> crate::Result<()> {
            if self.healthy {
                Ok(())
            } else {
                Err(crate::CacheError::RedisHealthCheck(
                    "cache probe failed".to_string(),
                ))
            }
        }

        async fn get_bytes(&self, _key: &str) -> Option<Vec<u8>> {
            None
        }

        async fn take_bytes(&self, _key: &str) -> Option<Vec<u8>> {
            None
        }

        async fn set_bytes(&self, _key: &str, _value: Vec<u8>, _ttl_secs: Option<u64>) {}

        async fn set_bytes_if_absent(
            &self,
            _key: &str,
            _value: Vec<u8>,
            _ttl_secs: Option<u64>,
        ) -> bool {
            false
        }

        async fn delete(&self, _key: &str) {}

        async fn invalidate_prefix(&self, _prefix: &str) {}
    }

    #[tokio::test]
    async fn cache_component_reports_configured_backend_fallback() {
        let config = CacheConfig {
            backend: "redis".to_string(),
            redis_url: "redis://example.com:6379/0".to_string(),
            default_ttl: 60,
        };
        let cache = FakeCache::new("memory");

        let report = check_cache_component(&config, &cache).await;

        assert_eq!(report.name, "cache");
        assert_eq!(report.status, HealthStatus::Degraded);
        assert_eq!(
            report.message,
            "configured cache backend 'redis' is using active backend 'memory'"
        );
        assert_eq!(
            report
                .detail("configured_backend")
                .and_then(HealthComponentDetailValue::as_text),
            Some("redis")
        );
        assert_eq!(
            report
                .detail("active_backend")
                .and_then(HealthComponentDetailValue::as_text),
            Some("memory")
        );
    }

    #[tokio::test]
    async fn cache_component_reports_active_backend_probe_result() {
        let config = CacheConfig {
            backend: "redis".to_string(),
            redis_url: "redis://example.com:6379/0".to_string(),
            default_ttl: 60,
        };

        let healthy = check_cache_component(&config, &FakeCache::new("redis")).await;
        assert_eq!(healthy.status, HealthStatus::Healthy);
        assert_eq!(healthy.message, "cache health check succeeded");
        assert_eq!(
            healthy
                .detail("active_backend")
                .and_then(HealthComponentDetailValue::as_text),
            Some("redis")
        );

        let degraded = check_cache_component(&config, &FakeCache::unhealthy("redis")).await;
        assert_eq!(degraded.status, HealthStatus::Unhealthy);
        assert!(
            degraded
                .message
                .contains("cache backend 'redis' health check failed")
        );
        assert_eq!(
            degraded
                .detail("active_backend")
                .and_then(HealthComponentDetailValue::as_text),
            Some("redis")
        );
    }

    #[tokio::test]
    async fn cache_health_check_registers_diagnostics_component() {
        let config = CacheConfig::default();
        let cache = Arc::new(crate::MemoryCache::new(60)) as Arc<dyn CacheBackend>;
        let mut registry = aster_forge_runtime::RuntimeComponentRegistry::new();

        register_cache_health_check(&mut registry, config, cache);

        let descriptor = registry
            .descriptor(super::CACHE_COMPONENT)
            .expect("cache component should be registered");
        assert_eq!(
            descriptor.kind,
            aster_forge_runtime::RuntimeComponentKind::Cache
        );
        assert_eq!(descriptor.health_checks.len(), 1);

        let report = registry
            .run_health(aster_forge_runtime::HealthCheckScope::Diagnostics)
            .await;
        assert_eq!(report.components[0].name, super::CACHE_HEALTH_CHECK);
        assert_eq!(report.status(), HealthStatus::Healthy);
    }
}

//! Generic health and readiness checks.

use crate::cache::CacheBackend;
use crate::config::CacheConfig;
use crate::errors::{AsterError, MapAsterErr, Result};
use crate::runtime::SharedRuntimeState;
use sea_orm::DatabaseConnection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl HealthStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }

    pub const fn is_issue(self) -> bool {
        !matches!(self, Self::Healthy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthComponentReport {
    pub name: &'static str,
    pub status: HealthStatus,
    pub message: String,
}

impl HealthComponentReport {
    pub fn healthy(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Healthy,
            message: message.into(),
        }
    }

    pub fn degraded(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Degraded,
            message: message.into(),
        }
    }

    pub fn unhealthy(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Unhealthy,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SystemHealthReport {
    pub components: Vec<HealthComponentReport>,
}

impl SystemHealthReport {
    pub fn has_issues(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.status.is_issue())
    }

    pub fn status(&self) -> HealthStatus {
        if self
            .components
            .iter()
            .any(|component| matches!(component.status, HealthStatus::Unhealthy))
        {
            HealthStatus::Unhealthy
        } else if self
            .components
            .iter()
            .any(|component| matches!(component.status, HealthStatus::Degraded))
        {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }

    pub fn summary(&self) -> String {
        if self.components.is_empty() {
            return "system health check did not run any components".to_string();
        }

        self.components
            .iter()
            .map(|component| format!("{} {}", component.name, component.status.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub async fn ping_database(db: &DatabaseConnection) -> Result<()> {
    db.ping()
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn check_ready<S: SharedRuntimeState>(state: &S) -> Result<()> {
    ping_database(state.reader_db()).await
}

pub async fn run_system_health_checks<S: SharedRuntimeState>(state: &S) -> SystemHealthReport {
    let components = vec![
        check_database_component(state.reader_db()).await,
        check_cache_component(&state.config().cache, state.cache().as_ref()).await,
    ];
    SystemHealthReport { components }
}

async fn check_database_component(db: &DatabaseConnection) -> HealthComponentReport {
    match ping_database(db).await {
        Ok(()) => HealthComponentReport::healthy("database", "database ping succeeded"),
        Err(error) => {
            HealthComponentReport::unhealthy("database", format!("database ping failed: {error}"))
        }
    }
}

async fn check_cache_component(
    config: &CacheConfig,
    cache: &dyn CacheBackend,
) -> HealthComponentReport {
    if !config.enabled {
        return HealthComponentReport::healthy("cache", "cache is disabled by configuration");
    }

    match cache.health_check().await {
        Ok(()) => HealthComponentReport::healthy("cache", "cache health check succeeded"),
        Err(error) => HealthComponentReport::degraded(
            "cache",
            format!(
                "{} cache health check failed: {error}",
                cache.backend_name()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HealthComponentReport, HealthStatus, SystemHealthReport, check_cache_component,
        check_database_component,
    };
    use crate::cache::CacheBackend;
    use crate::config::{CacheConfig, DatabaseConfig};
    use crate::errors::{AsterError, Result};
    use async_trait::async_trait;

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

        async fn health_check(&self) -> Result<()> {
            if self.healthy {
                Ok(())
            } else {
                Err(AsterError::internal_error("cache probe failed"))
            }
        }

        async fn get_bytes(&self, _key: &str) -> Option<Vec<u8>> {
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

    #[test]
    fn health_status_reports_wire_values_and_issues() {
        assert_eq!(HealthStatus::Healthy.as_str(), "healthy");
        assert_eq!(HealthStatus::Degraded.as_str(), "degraded");
        assert_eq!(HealthStatus::Unhealthy.as_str(), "unhealthy");
        assert!(!HealthStatus::Healthy.is_issue());
        assert!(HealthStatus::Degraded.is_issue());
        assert!(HealthStatus::Unhealthy.is_issue());
    }

    #[test]
    fn component_constructors_preserve_name_status_and_message() {
        assert_eq!(
            HealthComponentReport::healthy("database", "ok"),
            HealthComponentReport {
                name: "database",
                status: HealthStatus::Healthy,
                message: "ok".to_string(),
            }
        );
        assert_eq!(
            HealthComponentReport::degraded("cache", "fallback").status,
            HealthStatus::Degraded
        );
        assert_eq!(
            HealthComponentReport::unhealthy("database", "down").status,
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn system_health_report_status_and_summary_follow_worst_component() {
        let healthy = SystemHealthReport {
            components: vec![
                HealthComponentReport::healthy("database", "ok"),
                HealthComponentReport::healthy("cache", "ok"),
            ],
        };
        assert!(!healthy.has_issues());
        assert_eq!(healthy.status(), HealthStatus::Healthy);
        assert_eq!(healthy.summary(), "database healthy, cache healthy");

        let degraded = SystemHealthReport {
            components: vec![
                HealthComponentReport::healthy("database", "ok"),
                HealthComponentReport::degraded("cache", "fallback"),
            ],
        };
        assert!(degraded.has_issues());
        assert_eq!(degraded.status(), HealthStatus::Degraded);
        assert_eq!(degraded.summary(), "database healthy, cache degraded");

        let unhealthy = SystemHealthReport {
            components: vec![
                HealthComponentReport::degraded("cache", "fallback"),
                HealthComponentReport::unhealthy("database", "down"),
            ],
        };
        assert!(unhealthy.has_issues());
        assert_eq!(unhealthy.status(), HealthStatus::Unhealthy);
        assert_eq!(unhealthy.summary(), "cache degraded, database unhealthy");
    }

    #[test]
    fn empty_system_health_report_has_explicit_summary() {
        let report = SystemHealthReport { components: vec![] };

        assert!(!report.has_issues());
        assert_eq!(report.status(), HealthStatus::Healthy);
        assert_eq!(
            report.summary(),
            "system health check did not run any components"
        );
    }

    #[tokio::test]
    async fn cache_component_is_healthy_when_cache_is_disabled() {
        let config = CacheConfig {
            enabled: false,
            backend: "redis".to_string(),
            redis_url: "redis://example.com:6379/0".to_string(),
            default_ttl: 60,
        };
        let cache = FakeCache::unhealthy("redis");

        let report = check_cache_component(&config, &cache).await;

        assert_eq!(report.name, "cache");
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.message, "cache is disabled by configuration");
    }

    #[tokio::test]
    async fn cache_component_reports_active_backend_probe_result() {
        let config = CacheConfig {
            enabled: true,
            backend: "redis".to_string(),
            redis_url: "redis://example.com:6379/0".to_string(),
            default_ttl: 60,
        };

        let healthy = check_cache_component(&config, &FakeCache::new("redis")).await;
        assert_eq!(healthy.status, HealthStatus::Healthy);
        assert_eq!(healthy.message, "cache health check succeeded");

        let degraded = check_cache_component(&config, &FakeCache::unhealthy("redis")).await;
        assert_eq!(degraded.status, HealthStatus::Degraded);
        assert!(degraded.message.contains("redis cache health check failed"));
    }

    #[tokio::test]
    async fn database_component_reports_ping_success_and_failure() {
        let db = crate::db::connect_with_metrics(
            &DatabaseConfig {
                url: "sqlite::memory:".to_string(),
                pool_size: 1,
                retry_count: 0,
            },
            crate::metrics_core::NoopMetrics::arc(),
        )
        .await
        .unwrap();

        let healthy = check_database_component(&db).await;
        assert_eq!(healthy.status, HealthStatus::Healthy);
        assert_eq!(healthy.message, "database ping succeeded");

        db.close_by_ref().await.unwrap();
        let unhealthy = check_database_component(&db).await;
        assert_eq!(unhealthy.status, HealthStatus::Unhealthy);
        assert!(unhealthy.message.contains("database ping failed"));
    }
}

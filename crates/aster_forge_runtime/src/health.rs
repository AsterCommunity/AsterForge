//! Product-neutral health report models and runner.
//!
//! The types in this module describe component health and aggregate status.
//! Product crates decide which components to probe and how to map the report
//! into HTTP responses, task results, metrics, or admin UI payloads.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

type HealthCheckFuture = Pin<Box<dyn Future<Output = HealthComponentReport> + Send>>;
type HealthCheckFn = dyn Fn() -> HealthCheckFuture + Send + Sync;

/// Coarse status for a health component or an aggregate system report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// The component is operating normally.
    Healthy,
    /// The component works with reduced capability or a fallback.
    Degraded,
    /// The component is unavailable or failed its probe.
    Unhealthy,
}

impl HealthStatus {
    /// Returns the stable lowercase wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }

    /// Returns whether this status should be treated as an operational issue.
    pub const fn is_issue(self) -> bool {
        !matches!(self, Self::Healthy)
    }
}

/// Timeout classification for a registered health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCheckCriticality {
    /// Timeout or framework-level failure should make the component unhealthy.
    Critical,
    /// Timeout or framework-level failure should make the component degraded.
    NonCritical,
}

impl HealthCheckCriticality {
    const fn timeout_status(self) -> HealthStatus {
        match self {
            Self::Critical => HealthStatus::Unhealthy,
            Self::NonCritical => HealthStatus::Degraded,
        }
    }
}

/// Health status for one named component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthComponentReport {
    /// Stable component name, such as `database`, `cache`, or `storage`.
    pub name: &'static str,
    /// Component status.
    pub status: HealthStatus,
    /// Human-facing diagnostic message.
    pub message: String,
}

impl HealthComponentReport {
    /// Builds a healthy component report.
    pub fn healthy(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Healthy,
            message: message.into(),
        }
    }

    /// Builds a degraded component report.
    pub fn degraded(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Degraded,
            message: message.into(),
        }
    }

    /// Builds an unhealthy component report.
    pub fn unhealthy(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Unhealthy,
            message: message.into(),
        }
    }
}

struct RegisteredHealthCheck {
    name: &'static str,
    criticality: HealthCheckCriticality,
    timeout: Option<Duration>,
    check: Box<HealthCheckFn>,
}

/// Registry and sequential runner for product-provided health checks.
///
/// The registry owns check ordering, timeout handling, and report aggregation.
/// Product code owns the actual probe logic and should return a
/// `HealthComponentReport` with product-specific diagnostics.
#[derive(Default)]
pub struct HealthCheckRegistry {
    checks: Vec<RegisteredHealthCheck>,
}

impl HealthCheckRegistry {
    /// Creates an empty health check registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a health check.
    ///
    /// `name` is also used for timeout reports. The check future should return
    /// a component report with the same stable name.
    pub fn register<F, Fut>(
        &mut self,
        name: &'static str,
        criticality: HealthCheckCriticality,
        timeout: Option<Duration>,
        check: F,
    ) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.checks.push(RegisteredHealthCheck {
            name,
            criticality,
            timeout,
            check: Box::new(move || Box::pin(check())),
        });
        self
    }

    /// Runs registered checks in registration order and returns an aggregate report.
    pub async fn run(&self) -> SystemHealthReport {
        let mut components = Vec::with_capacity(self.checks.len());

        for check in &self.checks {
            let future = (check.check)();
            let component = match check.timeout {
                Some(timeout) => match tokio::time::timeout(timeout, future).await {
                    Ok(component) => component,
                    Err(_) => timeout_component(check.name, check.criticality, timeout),
                },
                None => future.await,
            };
            components.push(component);
        }

        SystemHealthReport::new(components)
    }

    /// Returns how many health checks are registered.
    pub fn len(&self) -> usize {
        self.checks.len()
    }

    /// Returns whether no health checks are registered.
    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }
}

fn timeout_component(
    name: &'static str,
    criticality: HealthCheckCriticality,
    timeout: Duration,
) -> HealthComponentReport {
    let message = format!("health check timed out after {}ms", timeout.as_millis());
    match criticality.timeout_status() {
        HealthStatus::Healthy => HealthComponentReport::healthy(name, message),
        HealthStatus::Degraded => HealthComponentReport::degraded(name, message),
        HealthStatus::Unhealthy => HealthComponentReport::unhealthy(name, message),
    }
}

/// Aggregate health report for a service instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemHealthReport {
    /// Component reports included in this health check run.
    pub components: Vec<HealthComponentReport>,
}

impl SystemHealthReport {
    /// Returns a report from component entries.
    pub fn new(components: Vec<HealthComponentReport>) -> Self {
        Self { components }
    }

    /// Returns whether any component is degraded or unhealthy.
    pub fn has_issues(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.status.is_issue())
    }

    /// Returns the worst status across all components.
    ///
    /// `Unhealthy` dominates `Degraded`, and an empty report is considered
    /// healthy because no product probe reported an issue.
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

    /// Returns a compact operator-facing summary.
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

    /// Returns component status and diagnostic messages for every component.
    pub fn details(&self) -> String {
        self.components
            .iter()
            .map(|component| {
                format!(
                    "{}={}: {}",
                    component.name,
                    component.status.as_str(),
                    component.message
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Returns a compact summary of only degraded or unhealthy components.
    ///
    /// When no component reports an issue, this falls back to [`Self::summary`].
    pub fn issue_summary(&self) -> String {
        let summary = self
            .components
            .iter()
            .filter(|component| component.status.is_issue())
            .map(|component| format!("{} {}", component.name, component.status.as_str()))
            .collect::<Vec<_>>()
            .join(", ");

        if summary.is_empty() {
            self.summary()
        } else {
            summary
        }
    }

    /// Returns diagnostic details for only degraded or unhealthy components.
    ///
    /// When no component reports an issue, this falls back to [`Self::details`].
    pub fn issue_details(&self) -> String {
        let details = self
            .components
            .iter()
            .filter(|component| component.status.is_issue())
            .map(|component| {
                format!(
                    "{}={}: {}",
                    component.name,
                    component.status.as_str(),
                    component.message
                )
            })
            .collect::<Vec<_>>()
            .join("; ");

        if details.is_empty() {
            self.details()
        } else {
            details
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HealthCheckCriticality, HealthCheckRegistry, HealthComponentReport, HealthStatus,
        SystemHealthReport,
    };
    use std::time::Duration;

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
        let healthy = SystemHealthReport::new(vec![
            HealthComponentReport::healthy("database", "ok"),
            HealthComponentReport::healthy("cache", "ok"),
        ]);
        assert!(!healthy.has_issues());
        assert_eq!(healthy.status(), HealthStatus::Healthy);
        assert_eq!(healthy.summary(), "database healthy, cache healthy");

        let degraded = SystemHealthReport::new(vec![
            HealthComponentReport::healthy("database", "ok"),
            HealthComponentReport::degraded("cache", "fallback"),
        ]);
        assert!(degraded.has_issues());
        assert_eq!(degraded.status(), HealthStatus::Degraded);
        assert_eq!(degraded.summary(), "database healthy, cache degraded");
        assert_eq!(
            degraded.details(),
            "database=healthy: ok; cache=degraded: fallback"
        );
        assert_eq!(degraded.issue_summary(), "cache degraded");
        assert_eq!(degraded.issue_details(), "cache=degraded: fallback");

        let unhealthy = SystemHealthReport::new(vec![
            HealthComponentReport::degraded("cache", "fallback"),
            HealthComponentReport::unhealthy("database", "down"),
        ]);
        assert!(unhealthy.has_issues());
        assert_eq!(unhealthy.status(), HealthStatus::Unhealthy);
        assert_eq!(unhealthy.summary(), "cache degraded, database unhealthy");
        assert_eq!(
            unhealthy.issue_summary(),
            "cache degraded, database unhealthy"
        );
        assert_eq!(
            unhealthy.issue_details(),
            "cache=degraded: fallback; database=unhealthy: down"
        );
    }

    #[test]
    fn empty_system_health_report_has_explicit_summary() {
        let report = SystemHealthReport::new(Vec::new());

        assert!(!report.has_issues());
        assert_eq!(report.status(), HealthStatus::Healthy);
        assert_eq!(
            report.summary(),
            "system health check did not run any components"
        );
        assert_eq!(report.details(), "");
        assert_eq!(
            report.issue_summary(),
            "system health check did not run any components"
        );
        assert_eq!(report.issue_details(), "");
    }

    #[tokio::test]
    async fn health_check_registry_runs_registered_checks_in_order() {
        let mut registry = HealthCheckRegistry::new();
        registry
            .register(
                "database",
                HealthCheckCriticality::Critical,
                None,
                || async { HealthComponentReport::healthy("database", "ok") },
            )
            .register(
                "cache",
                HealthCheckCriticality::NonCritical,
                None,
                || async { HealthComponentReport::degraded("cache", "fallback") },
            );

        let report = registry.run().await;

        assert_eq!(registry.len(), 2);
        assert_eq!(report.status(), HealthStatus::Degraded);
        assert_eq!(report.summary(), "database healthy, cache degraded");
    }

    #[tokio::test]
    async fn health_check_registry_maps_timeouts_by_criticality() {
        let mut registry = HealthCheckRegistry::new();
        registry
            .register(
                "critical",
                HealthCheckCriticality::Critical,
                Some(Duration::from_millis(1)),
                || async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    HealthComponentReport::healthy("critical", "late")
                },
            )
            .register(
                "optional",
                HealthCheckCriticality::NonCritical,
                Some(Duration::from_millis(1)),
                || async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    HealthComponentReport::healthy("optional", "late")
                },
            );

        let report = registry.run().await;

        assert_eq!(report.components[0].status, HealthStatus::Unhealthy);
        assert_eq!(report.components[1].status, HealthStatus::Degraded);
        assert!(
            report.components[0]
                .message
                .contains("health check timed out")
        );
    }
}

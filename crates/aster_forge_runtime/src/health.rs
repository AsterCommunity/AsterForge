//! Product-neutral health report models.
//!
//! The types in this module describe component health and aggregate status.
//! Product crates decide which components to probe and how to map the report
//! into HTTP responses, task results, metrics, or admin UI payloads.

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
}

#[cfg(test)]
mod tests {
    use super::{HealthComponentReport, HealthStatus, SystemHealthReport};

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

        let unhealthy = SystemHealthReport::new(vec![
            HealthComponentReport::degraded("cache", "fallback"),
            HealthComponentReport::unhealthy("database", "down"),
        ]);
        assert!(unhealthy.has_issues());
        assert_eq!(unhealthy.status(), HealthStatus::Unhealthy);
        assert_eq!(unhealthy.summary(), "cache degraded, database unhealthy");
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
    }
}

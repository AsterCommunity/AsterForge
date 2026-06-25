//! Product-neutral health report models and runner.
//!
//! The types in this module describe component health and aggregate status.
//! Product crates decide which components to probe and how to map the report
//! into HTTP responses, task results, metrics, or admin UI payloads.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::future::join_all;

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

/// Runtime view used when selecting which registered checks to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCheckScope {
    /// Minimal process liveness checks.
    Liveness,
    /// Readiness checks used by load balancers and orchestrators.
    Readiness,
    /// Full diagnostic checks used by admin pages and runtime tasks.
    Diagnostics,
}

impl HealthCheckScope {
    /// Returns the stable lowercase wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Liveness => "liveness",
            Self::Readiness => "readiness",
            Self::Diagnostics => "diagnostics",
        }
    }
}

/// Scope membership for a registered health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCheckScopes {
    liveness: bool,
    readiness: bool,
    diagnostics: bool,
}

impl HealthCheckScopes {
    /// Includes the check in every health scope.
    pub const fn all() -> Self {
        Self {
            liveness: true,
            readiness: true,
            diagnostics: true,
        }
    }

    /// Includes the check only in liveness runs.
    pub const fn liveness() -> Self {
        Self {
            liveness: true,
            readiness: false,
            diagnostics: false,
        }
    }

    /// Includes the check only in readiness runs.
    pub const fn readiness() -> Self {
        Self {
            liveness: false,
            readiness: true,
            diagnostics: false,
        }
    }

    /// Includes the check only in diagnostics runs.
    pub const fn diagnostics() -> Self {
        Self {
            liveness: false,
            readiness: false,
            diagnostics: true,
        }
    }

    /// Includes the check in readiness and diagnostics runs.
    pub const fn readiness_and_diagnostics() -> Self {
        Self {
            liveness: false,
            readiness: true,
            diagnostics: true,
        }
    }

    /// Returns whether this set includes `scope`.
    pub const fn contains(self, scope: HealthCheckScope) -> bool {
        match scope {
            HealthCheckScope::Liveness => self.liveness,
            HealthCheckScope::Readiness => self.readiness,
            HealthCheckScope::Diagnostics => self.diagnostics,
        }
    }
}

impl Default for HealthCheckScopes {
    fn default() -> Self {
        Self::all()
    }
}

/// Requirement level for a registered health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCheckRequirement {
    /// Framework-level failures should make the component unhealthy.
    Required,
    /// Framework-level failures should make the component degraded.
    Optional,
}

impl HealthCheckRequirement {
    const fn runtime_failure_status(self) -> HealthStatus {
        match self {
            Self::Required => HealthStatus::Unhealthy,
            Self::Optional => HealthStatus::Degraded,
        }
    }
}

/// Options applied to a registered health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCheckOptions {
    /// Whether framework-level failures make the component unhealthy or degraded.
    pub requirement: HealthCheckRequirement,
    /// Optional per-component timeout.
    pub timeout: Option<Duration>,
    /// Health scopes that should include this check.
    pub scopes: HealthCheckScopes,
}

impl HealthCheckOptions {
    /// Creates required-check options.
    pub const fn required(timeout: Option<Duration>) -> Self {
        Self {
            requirement: HealthCheckRequirement::Required,
            timeout,
            scopes: HealthCheckScopes::all(),
        }
    }

    /// Creates optional-check options.
    pub const fn optional(timeout: Option<Duration>) -> Self {
        Self {
            requirement: HealthCheckRequirement::Optional,
            timeout,
            scopes: HealthCheckScopes::all(),
        }
    }

    /// Returns options with a different timeout.
    pub const fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// Returns options with different scope membership.
    pub const fn with_scopes(mut self, scopes: HealthCheckScopes) -> Self {
        self.scopes = scopes;
        self
    }
}

impl Default for HealthCheckOptions {
    fn default() -> Self {
        Self::required(None)
    }
}

/// Static description of a registered health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCheckDescriptor {
    /// Stable component name, such as `database`, `cache`, or `storage`.
    pub name: &'static str,
    /// Whether framework-level failures make the component unhealthy or degraded.
    pub requirement: HealthCheckRequirement,
    /// Optional per-component timeout.
    pub timeout: Option<Duration>,
    /// Health scopes that include this check.
    pub scopes: HealthCheckScopes,
}

/// Typed diagnostic value attached to a component detail.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum HealthComponentDetailValue {
    /// Human-facing text such as a backend, driver, region, or mode.
    Text(String),
    /// Signed integer value.
    Integer(i64),
    /// Unsigned counter or depth value.
    Unsigned(u64),
    /// Boolean flag.
    Boolean(bool),
    /// Duration value in milliseconds for latency, age, lag, or timeout diagnostics.
    DurationMillis(u64),
}

impl HealthComponentDetailValue {
    /// Returns the stable lowercase type name for product DTOs.
    pub const fn value_type(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Integer(_) => "integer",
            Self::Unsigned(_) => "unsigned",
            Self::Boolean(_) => "boolean",
            Self::DurationMillis(_) => "duration_millis",
        }
    }

    /// Returns the text value when this detail stores text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the signed integer value when this detail stores one.
    pub const fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the unsigned integer value when this detail stores one.
    pub const fn as_unsigned(&self) -> Option<u64> {
        match self {
            Self::Unsigned(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the boolean value when this detail stores one.
    pub const fn as_boolean(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the duration value in milliseconds when this detail stores one.
    pub const fn as_duration_millis(&self) -> Option<u64> {
        match self {
            Self::DurationMillis(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns a stable human-facing display value.
    pub fn display_value(&self) -> String {
        match self {
            Self::Text(value) => value.clone(),
            Self::Integer(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
            Self::DurationMillis(value) => duration_millis_display_value(*value),
        }
    }
}

impl From<String> for HealthComponentDetailValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for HealthComponentDetailValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<i64> for HealthComponentDetailValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<u64> for HealthComponentDetailValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for HealthComponentDetailValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<Duration> for HealthComponentDetailValue {
    fn from(value: Duration) -> Self {
        Self::DurationMillis(saturating_duration_millis(value))
    }
}

/// Structured diagnostic detail attached to a component report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HealthComponentDetail {
    /// Stable detail key.
    pub key: String,
    /// Typed detail value.
    pub value: HealthComponentDetailValue,
}

impl HealthComponentDetail {
    /// Builds a typed component detail.
    pub fn new(key: impl Into<String>, value: impl Into<HealthComponentDetailValue>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
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
    /// Duration spent running this component check.
    pub duration: Option<Duration>,
    /// Optional structured diagnostics.
    pub details: Vec<HealthComponentDetail>,
}

impl HealthComponentReport {
    /// Builds a healthy component report.
    pub fn healthy(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Healthy,
            message: message.into(),
            duration: None,
            details: Vec::new(),
        }
    }

    /// Builds a degraded component report.
    pub fn degraded(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Degraded,
            message: message.into(),
            duration: None,
            details: Vec::new(),
        }
    }

    /// Builds an unhealthy component report.
    pub fn unhealthy(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: HealthStatus::Unhealthy,
            message: message.into(),
            duration: None,
            details: Vec::new(),
        }
    }

    /// Returns this report with runtime duration attached.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Returns this report with a structured diagnostic detail appended.
    pub fn with_detail(
        mut self,
        key: impl Into<String>,
        value: impl Into<HealthComponentDetailValue>,
    ) -> Self {
        self.details.push(HealthComponentDetail::new(key, value));
        self
    }

    /// Returns the first structured detail value for `key`.
    pub fn detail(&self, key: &str) -> Option<&HealthComponentDetailValue> {
        self.details
            .iter()
            .find(|detail| detail.key == key)
            .map(|detail| &detail.value)
    }

    /// Returns component duration in seconds, if present.
    pub fn duration_seconds(&self) -> Option<f64> {
        self.duration.map(duration_seconds)
    }
}

struct RegisteredHealthCheck {
    name: &'static str,
    options: HealthCheckOptions,
    check: Box<HealthCheckFn>,
}

impl RegisteredHealthCheck {
    fn descriptor(&self) -> HealthCheckDescriptor {
        HealthCheckDescriptor {
            name: self.name,
            requirement: self.options.requirement,
            timeout: self.options.timeout,
            scopes: self.options.scopes,
        }
    }
}

/// Builder for health check registries with shared defaults.
#[derive(Default)]
pub struct HealthCheckRegistryBuilder {
    default_timeout: Option<Duration>,
    default_scopes: HealthCheckScopes,
    registry: HealthCheckRegistry,
}

impl HealthCheckRegistryBuilder {
    /// Creates an empty registry builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the default timeout used by [`Self::register_required`] and
    /// [`Self::register_optional`].
    pub const fn default_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Sets the default scope membership used by [`Self::register_required`]
    /// and [`Self::register_optional`].
    pub const fn default_scopes(mut self, scopes: HealthCheckScopes) -> Self {
        self.default_scopes = scopes;
        self
    }

    /// Registers a required check using builder defaults.
    pub fn register_required<F, Fut>(&mut self, name: &'static str, check: F) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.registry.register_with_options(
            name,
            HealthCheckOptions::required(self.default_timeout).with_scopes(self.default_scopes),
            check,
        );
        self
    }

    /// Registers an optional check using builder defaults.
    pub fn register_optional<F, Fut>(&mut self, name: &'static str, check: F) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.registry.register_with_options(
            name,
            HealthCheckOptions::optional(self.default_timeout).with_scopes(self.default_scopes),
            check,
        );
        self
    }

    /// Registers a check with explicit options.
    pub fn register_with_options<F, Fut>(
        &mut self,
        name: &'static str,
        options: HealthCheckOptions,
        check: F,
    ) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.registry.register_with_options(name, options, check);
        self
    }

    /// Consumes the builder and returns the registry.
    pub fn build(self) -> HealthCheckRegistry {
        self.registry
    }
}

/// Registry and concurrent runner for product-provided health checks.
///
/// The registry owns scope selection, timeout handling, panic-to-report
/// conversion, concurrent execution, registration-order output, and aggregate
/// status calculation. Product code owns the actual probe logic and should
/// return a `HealthComponentReport` with product-specific diagnostics.
#[derive(Default)]
pub struct HealthCheckRegistry {
    checks: Vec<RegisteredHealthCheck>,
}

impl HealthCheckRegistry {
    /// Creates an empty health check registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry and applies one registration function.
    ///
    /// This is the lightweight path for product code that only needs to run
    /// health probes. Use [`RuntimeComponentRegistry`](crate::RuntimeComponentRegistry)
    /// only when the caller also needs component metadata or shutdown phases.
    pub fn configured<F>(configure: F) -> Self
    where
        F: FnOnce(&mut Self),
    {
        let mut registry = Self::new();
        registry.configure(configure);
        registry
    }

    /// Applies one registration function and returns the registry.
    ///
    /// The shape intentionally mirrors Actix Web's `configure` pattern, so
    /// subsystem modules can expose small registration functions without owning
    /// the root registry.
    pub fn configure<F>(&mut self, configure: F) -> &mut Self
    where
        F: FnOnce(&mut Self),
    {
        configure(self);
        self
    }

    /// Registers a health check with full options.
    ///
    /// `name` is also used for timeout and panic reports. The check future
    /// should return a component report with the same stable name.
    pub fn register_with_options<F, Fut>(
        &mut self,
        name: &'static str,
        options: HealthCheckOptions,
        check: F,
    ) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.checks.push(RegisteredHealthCheck {
            name,
            options,
            check: Box::new(move || Box::pin(check())),
        });
        self
    }

    /// Registers a required health check.
    pub fn register_required<F, Fut>(
        &mut self,
        name: &'static str,
        timeout: Option<Duration>,
        check: F,
    ) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.register_with_options(name, HealthCheckOptions::required(timeout), check)
    }

    /// Registers an optional health check.
    pub fn register_optional<F, Fut>(
        &mut self,
        name: &'static str,
        timeout: Option<Duration>,
        check: F,
    ) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.register_with_options(name, HealthCheckOptions::optional(timeout), check)
    }

    /// Runs registered checks concurrently and returns an aggregate report.
    ///
    /// Component reports are returned in registration order even though checks
    /// run concurrently.
    pub async fn run(&self) -> SystemHealthReport {
        self.run_selected(|_| true).await
    }

    /// Runs only checks registered for `scope`.
    pub async fn run_scope(&self, scope: HealthCheckScope) -> SystemHealthReport {
        self.run_selected(|check| check.options.scopes.contains(scope))
            .await
    }

    async fn run_selected<F>(&self, include: F) -> SystemHealthReport
    where
        F: Fn(&RegisteredHealthCheck) -> bool,
    {
        let started = Instant::now();
        let futures = self
            .checks
            .iter()
            .filter(|check| include(check))
            .map(run_registered_check);
        let components = join_all(futures).await;

        SystemHealthReport::with_duration(components, started.elapsed())
    }

    /// Returns how many health checks are registered.
    pub fn len(&self) -> usize {
        self.checks.len()
    }

    /// Returns whether no health checks are registered.
    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }

    /// Returns registered check descriptors in registration order.
    pub fn descriptors(&self) -> Vec<HealthCheckDescriptor> {
        self.checks
            .iter()
            .map(RegisteredHealthCheck::descriptor)
            .collect()
    }

    /// Returns registered descriptors that belong to `scope`.
    pub fn descriptors_for_scope(&self, scope: HealthCheckScope) -> Vec<HealthCheckDescriptor> {
        self.checks
            .iter()
            .filter(|check| check.options.scopes.contains(scope))
            .map(RegisteredHealthCheck::descriptor)
            .collect()
    }
}

async fn run_registered_check(check: &RegisteredHealthCheck) -> HealthComponentReport {
    let started = Instant::now();
    let outcome = AssertUnwindSafe(async {
        let future = (check.check)();
        match check.options.timeout {
            Some(timeout) => match tokio::time::timeout(timeout, future).await {
                Ok(component) => component,
                Err(_) => timeout_component(check.name, check.options.requirement, timeout),
            },
            None => future.await,
        }
    })
    .catch_unwind()
    .await;
    let duration = started.elapsed();

    match outcome {
        Ok(component) => {
            if component.duration.is_some() {
                component
            } else {
                component.with_duration(duration)
            }
        }
        Err(_) => runtime_failure_component(
            check.name,
            check.options.requirement,
            "health check panicked",
        )
        .with_duration(duration),
    }
}

fn timeout_component(
    name: &'static str,
    requirement: HealthCheckRequirement,
    timeout: Duration,
) -> HealthComponentReport {
    let message = format!("health check timed out after {}ms", timeout.as_millis());
    runtime_failure_component(name, requirement, message)
}

fn runtime_failure_component(
    name: &'static str,
    requirement: HealthCheckRequirement,
    message: impl Into<String>,
) -> HealthComponentReport {
    match requirement.runtime_failure_status() {
        HealthStatus::Healthy => HealthComponentReport::healthy(name, message),
        HealthStatus::Degraded => HealthComponentReport::degraded(name, message),
        HealthStatus::Unhealthy => HealthComponentReport::unhealthy(name, message),
    }
}

fn duration_seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

fn saturating_duration_millis(duration: Duration) -> u64 {
    aster_forge_utils::numbers::u128_to_u64_saturating(duration.as_millis())
}

fn duration_millis_display_value(duration_millis: u64) -> String {
    if duration_millis < 1_000 {
        format!("{duration_millis}ms")
    } else {
        format!("{:.3}s", duration_millis as f64 / 1_000.0)
    }
}

/// Product-side bridge for recording health reports into a metrics backend.
///
/// Forge does not depend on a concrete metrics exporter here. Product crates
/// implement this trait for their own recorder or a small adapter, then call
/// [`SystemHealthReport::record_metrics`] after a health run.
pub trait HealthMetricsRecorder {
    /// Records the aggregate health result for `scope`.
    fn record_health_report(
        &self,
        scope: &'static str,
        status: HealthStatus,
        duration_seconds: f64,
    );

    /// Records one component result for `scope`.
    fn record_health_component(
        &self,
        scope: &'static str,
        component: &HealthComponentReport,
        duration_seconds: f64,
    );
}

/// Aggregate health report for a service instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemHealthReport {
    /// Component reports included in this health check run.
    pub components: Vec<HealthComponentReport>,
    /// Total duration of the aggregate health run.
    pub duration: Option<Duration>,
}

impl SystemHealthReport {
    /// Returns a report from component entries.
    pub fn new(components: Vec<HealthComponentReport>) -> Self {
        Self {
            components,
            duration: None,
        }
    }

    /// Returns a report from component entries and aggregate duration.
    pub fn with_duration(components: Vec<HealthComponentReport>, duration: Duration) -> Self {
        Self {
            components,
            duration: Some(duration),
        }
    }

    /// Returns aggregate duration in seconds, if present.
    pub fn duration_seconds(&self) -> Option<f64> {
        self.duration.map(duration_seconds)
    }

    /// Records this report through a product-provided metrics bridge.
    pub fn record_metrics<R>(&self, scope: &'static str, recorder: &R)
    where
        R: HealthMetricsRecorder + ?Sized,
    {
        recorder.record_health_report(
            scope,
            self.status(),
            self.duration_seconds().unwrap_or_default(),
        );

        for component in &self.components {
            recorder.record_health_component(
                scope,
                component,
                component.duration_seconds().unwrap_or_default(),
            );
        }
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
        HealthCheckOptions, HealthCheckRegistry, HealthCheckRegistryBuilder,
        HealthCheckRequirement, HealthCheckScope, HealthCheckScopes, HealthComponentDetail,
        HealthComponentDetailValue, HealthComponentReport, HealthMetricsRecorder, HealthStatus,
        SystemHealthReport,
    };
    use std::sync::{Arc, Mutex};
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
                duration: None,
                details: Vec::new(),
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
        assert_eq!(
            HealthComponentReport::healthy("cache", "ok")
                .with_duration(Duration::from_millis(3))
                .with_detail("backend", "memory"),
            HealthComponentReport {
                name: "cache",
                status: HealthStatus::Healthy,
                message: "ok".to_string(),
                duration: Some(Duration::from_millis(3)),
                details: vec![HealthComponentDetail {
                    key: "backend".to_string(),
                    value: HealthComponentDetailValue::Text("memory".to_string()),
                }],
            }
        );
        let report = HealthComponentReport::healthy("cache", "ok")
            .with_detail("backend", "memory")
            .with_detail("queue_depth", 7_u64)
            .with_detail("healthy", true)
            .with_detail("latency", Duration::from_millis(42));
        assert_eq!(
            report
                .detail("backend")
                .and_then(HealthComponentDetailValue::as_text),
            Some("memory")
        );
        assert_eq!(
            report
                .detail("queue_depth")
                .and_then(HealthComponentDetailValue::as_unsigned),
            Some(7)
        );
        assert_eq!(
            report
                .detail("healthy")
                .and_then(HealthComponentDetailValue::as_boolean),
            Some(true)
        );
        assert_eq!(
            report
                .detail("latency")
                .and_then(HealthComponentDetailValue::as_duration_millis),
            Some(42)
        );
        assert_eq!(report.detail("missing"), None);
    }

    #[test]
    fn component_details_serialize_as_typed_schema() {
        let details = vec![
            HealthComponentDetail::new("backend", "redis"),
            HealthComponentDetail::new("queue_depth", 12_u64),
            HealthComponentDetail::new("healthy", true),
            HealthComponentDetail::new("latency", Duration::from_millis(42)),
        ];

        let encoded = serde_json::to_value(&details).unwrap();

        assert_eq!(
            encoded,
            serde_json::json!([
                { "key": "backend", "value": { "type": "text", "value": "redis" } },
                { "key": "queue_depth", "value": { "type": "unsigned", "value": 12 } },
                { "key": "healthy", "value": { "type": "boolean", "value": true } },
                { "key": "latency", "value": { "type": "duration_millis", "value": 42 } }
            ])
        );
    }

    #[test]
    fn health_check_scopes_select_expected_views() {
        assert_eq!(HealthCheckScope::Readiness.as_str(), "readiness");
        assert!(HealthCheckScopes::all().contains(HealthCheckScope::Liveness));
        assert!(HealthCheckScopes::all().contains(HealthCheckScope::Readiness));
        assert!(HealthCheckScopes::all().contains(HealthCheckScope::Diagnostics));
        assert!(
            HealthCheckScopes::readiness_and_diagnostics().contains(HealthCheckScope::Readiness)
        );
        assert!(
            HealthCheckScopes::readiness_and_diagnostics().contains(HealthCheckScope::Diagnostics)
        );
        assert!(
            !HealthCheckScopes::readiness_and_diagnostics().contains(HealthCheckScope::Liveness)
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
    async fn health_check_registry_applies_configure_function() {
        let registry = HealthCheckRegistry::configured(|registry| {
            registry
                .register_with_options(
                    "database",
                    HealthCheckOptions::required(None)
                        .with_scopes(HealthCheckScopes::readiness_and_diagnostics()),
                    || async { HealthComponentReport::healthy("database", "ok") },
                )
                .configure(|registry| {
                    registry.register_with_options(
                        "cache",
                        HealthCheckOptions::optional(None)
                            .with_scopes(HealthCheckScopes::diagnostics()),
                        || async { HealthComponentReport::healthy("cache", "ok") },
                    );
                });
        });

        let readiness = registry.run_scope(HealthCheckScope::Readiness).await;
        let diagnostics = registry.run_scope(HealthCheckScope::Diagnostics).await;

        assert_eq!(readiness.components.len(), 1);
        assert_eq!(readiness.components[0].name, "database");
        assert_eq!(diagnostics.components.len(), 2);
        assert_eq!(
            registry
                .descriptors_for_scope(HealthCheckScope::Readiness)
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn health_check_registry_runs_registered_checks_concurrently_in_registration_order() {
        let mut registry = HealthCheckRegistry::new();
        registry
            .register_required("database", None, || async {
                tokio::time::sleep(Duration::from_millis(40)).await;
                HealthComponentReport::healthy("database", "ok")
            })
            .register_optional("cache", None, || async {
                HealthComponentReport::degraded("cache", "fallback")
            });

        let started = std::time::Instant::now();
        let report = registry.run().await;

        assert_eq!(registry.len(), 2);
        assert_eq!(report.status(), HealthStatus::Degraded);
        assert_eq!(report.summary(), "database healthy, cache degraded");
        assert!(started.elapsed() < Duration::from_millis(80));
        assert_eq!(report.components[0].name, "database");
        assert_eq!(report.components[1].name, "cache");
        assert!(report.duration.is_some());
        assert!(
            report
                .components
                .iter()
                .all(|component| component.duration.is_some())
        );
    }

    #[tokio::test]
    async fn health_check_registry_runs_selected_scope_only() {
        let mut registry = HealthCheckRegistry::new();
        registry
            .register_with_options(
                "database",
                HealthCheckOptions::required(None)
                    .with_scopes(HealthCheckScopes::readiness_and_diagnostics()),
                || async { HealthComponentReport::healthy("database", "ok") },
            )
            .register_with_options(
                "cache",
                HealthCheckOptions::optional(None).with_scopes(HealthCheckScopes::diagnostics()),
                || async { HealthComponentReport::healthy("cache", "ok") },
            );

        let readiness = registry.run_scope(HealthCheckScope::Readiness).await;
        let diagnostics = registry.run_scope(HealthCheckScope::Diagnostics).await;

        assert_eq!(readiness.components.len(), 1);
        assert_eq!(readiness.components[0].name, "database");
        assert_eq!(diagnostics.components.len(), 2);
    }

    #[tokio::test]
    async fn health_check_registry_exposes_descriptors_by_scope() {
        let mut registry = HealthCheckRegistry::new();
        registry
            .register_with_options(
                "database",
                HealthCheckOptions::required(Some(Duration::from_secs(5)))
                    .with_scopes(HealthCheckScopes::readiness_and_diagnostics()),
                || async { HealthComponentReport::healthy("database", "ok") },
            )
            .register_with_options(
                "cache",
                HealthCheckOptions::optional(None).with_scopes(HealthCheckScopes::diagnostics()),
                || async { HealthComponentReport::healthy("cache", "ok") },
            );

        let all = registry.descriptors();
        let readiness = registry.descriptors_for_scope(HealthCheckScope::Readiness);

        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "database");
        assert_eq!(all[0].timeout, Some(Duration::from_secs(5)));
        assert_eq!(all[1].requirement, HealthCheckRequirement::Optional);
        assert_eq!(readiness.len(), 1);
        assert_eq!(readiness[0].name, "database");
    }

    #[tokio::test]
    async fn health_check_registry_builder_applies_defaults() {
        let mut builder = HealthCheckRegistryBuilder::new()
            .default_timeout(Some(Duration::from_secs(2)))
            .default_scopes(HealthCheckScopes::diagnostics());
        builder
            .register_required("database", || async {
                HealthComponentReport::healthy("database", "ok")
            })
            .register_optional("cache", || async {
                HealthComponentReport::healthy("cache", "ok")
            });
        let registry = builder.build();

        let descriptors = registry.descriptors();
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].timeout, Some(Duration::from_secs(2)));
        assert!(
            descriptors[0]
                .scopes
                .contains(HealthCheckScope::Diagnostics)
        );
        assert!(!descriptors[0].scopes.contains(HealthCheckScope::Readiness));
        assert_eq!(descriptors[1].requirement, HealthCheckRequirement::Optional);
    }

    #[tokio::test]
    async fn health_check_registry_maps_timeouts_by_requirement() {
        let mut registry = HealthCheckRegistry::new();
        registry
            .register_required("critical", Some(Duration::from_millis(1)), || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                HealthComponentReport::healthy("critical", "late")
            })
            .register_optional("optional", Some(Duration::from_millis(1)), || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                HealthComponentReport::healthy("optional", "late")
            });

        let report = registry.run().await;

        assert_eq!(report.components[0].status, HealthStatus::Unhealthy);
        assert_eq!(report.components[1].status, HealthStatus::Degraded);
        assert!(
            report.components[0]
                .message
                .contains("health check timed out")
        );
    }

    #[tokio::test]
    async fn health_check_registry_maps_panics_by_requirement() {
        let mut registry = HealthCheckRegistry::new();
        registry
            .register_required("critical", None, || async {
                panic!("critical health check panic")
            })
            .register_optional("optional", None, || async {
                panic!("optional health check panic")
            });

        let report = registry.run().await;

        assert_eq!(report.components[0].status, HealthStatus::Unhealthy);
        assert_eq!(report.components[0].message, "health check panicked");
        assert_eq!(report.components[1].status, HealthStatus::Degraded);
        assert_eq!(report.components[1].message, "health check panicked");
    }

    #[test]
    fn system_health_report_records_metrics_through_bridge() {
        #[derive(Default)]
        struct Recorder {
            events: Arc<Mutex<Vec<String>>>,
        }

        impl HealthMetricsRecorder for Recorder {
            fn record_health_report(
                &self,
                scope: &'static str,
                status: HealthStatus,
                duration_seconds: f64,
            ) {
                self.events.lock().unwrap().push(format!(
                    "report:{scope}:{}:{duration_seconds:.3}",
                    status.as_str()
                ));
            }

            fn record_health_component(
                &self,
                scope: &'static str,
                component: &HealthComponentReport,
                duration_seconds: f64,
            ) {
                self.events.lock().unwrap().push(format!(
                    "component:{scope}:{}:{}:{duration_seconds:.3}",
                    component.name,
                    component.status.as_str()
                ));
            }
        }

        let recorder = Recorder::default();
        let report = SystemHealthReport::with_duration(
            vec![
                HealthComponentReport::healthy("database", "ok")
                    .with_duration(Duration::from_millis(10)),
                HealthComponentReport::degraded("cache", "fallback")
                    .with_duration(Duration::from_millis(20)),
            ],
            Duration::from_millis(25),
        );

        report.record_metrics("diagnostics", &recorder);

        let events = recorder.events.lock().unwrap();
        assert_eq!(
            events.as_slice(),
            [
                "report:diagnostics:degraded:0.025",
                "component:diagnostics:database:healthy:0.010",
                "component:diagnostics:cache:degraded:0.020",
            ]
        );
    }
}

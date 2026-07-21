//! Runtime component registration primitives.
//!
//! This module ties together the reusable runtime registries that already live
//! in this crate. It lets product crates describe subsystems once, then attach
//! health checks and shutdown phases without duplicating central dispatch
//! tables. Product crates still own resource construction, application state
//! assembly, business-specific startup ordering, and how reports are exposed.
//!
//! Component dependencies are enforced by [`RuntimeComponentRegistry`] when it
//! runs component-owned shutdown phases. Lower-level coordinators such as
//! [`crate::ShutdownCoordinator`] remain simple ordered executors for callers
//! that already have a fixed sequence.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::{
    HealthCheckDescriptor, HealthCheckOptions, HealthCheckRegistry, HealthCheckScope,
    HealthComponentReport, ShutdownPhaseReport, ShutdownPhaseStatus, ShutdownReport,
    StartupCoordinator, StartupPhaseFailurePolicy, StartupReport, SystemHealthReport,
};

type RuntimeShutdownFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type RuntimeShutdownPhaseFn = dyn FnMut() -> RuntimeShutdownFuture + Send;

/// Product-owned runtime component bundle.
///
/// A bundle is useful when registration needs to consume owned handles such as database pools,
/// background task collections, or other shutdown-only resources. Product subsystems should expose
/// component factory functions that return a bundle registration instead of asking entrypoints to
/// call low-level registry functions directly.
pub trait RuntimeComponentBundle {
    /// Registers this bundle into the runtime component registry.
    fn register(self, registry: &mut RuntimeComponentRegistry);
}

impl<F> RuntimeComponentBundle for F
where
    F: FnOnce(&mut RuntimeComponentRegistry),
{
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        self(registry);
    }
}

/// Broad category for a registered runtime component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeComponentKind {
    /// Core process-level component.
    Core,
    /// Database or database connection pool component.
    Database,
    /// Cache component.
    Cache,
    /// Object storage or file storage component.
    Storage,
    /// Mail sender, outbox, or delivery component.
    Mail,
    /// Background task scheduler or worker component.
    Tasks,
    /// External authentication connector component.
    ExternalAuth,
    /// Product-specific component that does not fit another shared kind.
    Product,
}

impl RuntimeComponentKind {
    /// Returns a stable lowercase wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Database => "database",
            Self::Cache => "cache",
            Self::Storage => "storage",
            Self::Mail => "mail",
            Self::Tasks => "tasks",
            Self::ExternalAuth => "external_auth",
            Self::Product => "product",
        }
    }
}

/// Static shutdown metadata for a registered component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeShutdownDescriptor {
    /// Stable shutdown phase name.
    pub phase_name: &'static str,
    /// Optional phase timeout.
    pub timeout: Option<Duration>,
}

/// Static startup metadata for a registered component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStartupDescriptor {
    /// Stable startup phase name.
    pub phase_name: &'static str,
    /// Failure policy used by this startup phase.
    pub failure_policy: StartupPhaseFailurePolicy,
}

/// Static runtime task metadata for a registered component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTaskDescriptor {
    /// Stable task name used in logs, persisted runtime payloads, or admin UI.
    pub task_name: &'static str,
    /// Operator-facing display name.
    pub display_name: &'static str,
}

/// Static metadata for a registered runtime component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeComponentDescriptor {
    /// Stable component name.
    pub name: &'static str,
    /// Broad component category.
    pub kind: RuntimeComponentKind,
    /// Stable names of components that should be initialized before this one.
    pub dependencies: Vec<&'static str>,
    /// Registered startup phases owned by this component.
    pub startup: Vec<RuntimeStartupDescriptor>,
    /// Registered health checks owned by this component.
    pub health_checks: Vec<HealthCheckDescriptor>,
    /// Registered runtime tasks owned by this component.
    pub tasks: Vec<RuntimeTaskDescriptor>,
    /// Registered shutdown phases owned by this component.
    pub shutdown: Vec<RuntimeShutdownDescriptor>,
}

impl RuntimeComponentDescriptor {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            kind: RuntimeComponentKind::Product,
            dependencies: Vec::new(),
            startup: Vec::new(),
            health_checks: Vec::new(),
            tasks: Vec::new(),
            shutdown: Vec::new(),
        }
    }
}

/// Component dependency graph validation error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeComponentGraphError {
    /// A component depends on a component that was never registered.
    #[error("runtime component '{component}' depends on missing component '{dependency}'")]
    MissingDependency {
        /// Component declaring the dependency.
        component: &'static str,
        /// Missing dependency component name.
        dependency: &'static str,
    },
    /// Component dependencies contain a cycle.
    #[error("runtime component dependency cycle detected: {cycle}")]
    Cycle {
        /// Human-readable cycle path.
        cycle: String,
    },
}

struct RuntimeComponentShutdownPhase {
    component_name: &'static str,
    phase_name: &'static str,
    timeout: Option<Duration>,
    phase: Box<RuntimeShutdownPhaseFn>,
}

/// Registry for runtime component metadata and lifecycle hooks.
#[derive(Default)]
pub struct RuntimeComponentRegistry {
    components: Vec<RuntimeComponentDescriptor>,
    startup: StartupCoordinator,
    health: HealthCheckRegistry,
    shutdown: Vec<RuntimeComponentShutdownPhase>,
}

impl RuntimeComponentRegistry {
    /// Creates an empty component registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry and applies one product or subsystem registration function.
    pub fn configured<F>(configure: F) -> Self
    where
        F: FnOnce(&mut Self),
    {
        let mut registry = Self::new();
        registry.configure(configure);
        registry
    }

    /// Applies a product or subsystem registration function.
    pub fn configure<F>(&mut self, configure: F) -> &mut Self
    where
        F: FnOnce(&mut Self),
    {
        configure(self);
        self
    }

    /// Registers one product-owned component bundle.
    pub fn register_bundle<B>(&mut self, bundle: B) -> &mut Self
    where
        B: RuntimeComponentBundle,
    {
        bundle.register(self);
        self
    }

    /// Registers a component health check with explicit options.
    pub fn component_health_with_options<F, Fut>(
        &mut self,
        component_name: &'static str,
        kind: RuntimeComponentKind,
        check_name: &'static str,
        options: HealthCheckOptions,
        check: F,
    ) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.component(component_name)
            .kind(kind)
            .health_with_options(check_name, options, check);
        self
    }

    /// Registers a component startup phase.
    pub fn component_startup<F, Fut>(
        &mut self,
        component_name: &'static str,
        kind: RuntimeComponentKind,
        phase_name: &'static str,
        failure_policy: StartupPhaseFailurePolicy,
        phase: F,
    ) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.component(component_name)
            .kind(kind)
            .startup(phase_name, failure_policy, phase);
        self
    }

    /// Registers a component-owned runtime task descriptor.
    pub fn component_task(
        &mut self,
        component_name: &'static str,
        kind: RuntimeComponentKind,
        task_name: &'static str,
        display_name: &'static str,
    ) -> &mut Self {
        self.component(component_name)
            .kind(kind)
            .task(task_name, display_name);
        self
    }

    /// Registers a component shutdown phase.
    pub fn component_shutdown<F, Fut>(
        &mut self,
        component_name: &'static str,
        kind: RuntimeComponentKind,
        phase_name: &'static str,
        timeout: Option<Duration>,
        phase: F,
    ) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.component(component_name)
            .kind(kind)
            .shutdown(phase_name, timeout, phase);
        self
    }

    /// Registers a component shutdown phase that consumes one owned value at most once.
    pub fn component_shutdown_once<T, F, Fut>(
        &mut self,
        component_name: &'static str,
        kind: RuntimeComponentKind,
        phase_name: &'static str,
        timeout: Option<Duration>,
        value: T,
        phase: F,
    ) -> &mut Self
    where
        T: Send + 'static,
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.component(component_name)
            .kind(kind)
            .shutdown_once(phase_name, timeout, value, phase);
        self
    }

    /// Returns a builder for `name`, creating the component when needed.
    pub fn component(&mut self, name: &'static str) -> RuntimeComponentBuilder<'_> {
        let index = match self
            .components
            .iter()
            .position(|component| component.name == name)
        {
            Some(index) => index,
            None => {
                self.components.push(RuntimeComponentDescriptor::new(name));
                self.components.len() - 1
            }
        };

        RuntimeComponentBuilder {
            registry: self,
            index,
        }
    }

    /// Returns registered component descriptors in registration order.
    pub fn descriptors(&self) -> &[RuntimeComponentDescriptor] {
        &self.components
    }

    /// Returns one descriptor by component name.
    pub fn descriptor(&self, name: &str) -> Option<&RuntimeComponentDescriptor> {
        self.components
            .iter()
            .find(|component| component.name == name)
    }

    /// Returns the underlying health registry.
    pub const fn health_registry(&self) -> &HealthCheckRegistry {
        &self.health
    }

    /// Returns the underlying health registry mutably.
    pub const fn health_registry_mut(&mut self) -> &mut HealthCheckRegistry {
        &mut self.health
    }

    /// Runs health checks registered for `scope`.
    pub async fn run_health(&mut self, scope: HealthCheckScope) -> SystemHealthReport {
        self.health.run_scope(scope).await
    }

    /// Runs registered startup phases.
    pub async fn startup(&mut self) -> StartupReport {
        self.startup.run().await
    }

    /// Validates that the component dependency graph is resolvable.
    pub fn validate(&self) -> Result<(), RuntimeComponentGraphError> {
        let descriptor_by_name = self
            .components
            .iter()
            .map(|component| (component.name, component))
            .collect::<HashMap<_, _>>();

        for component in &self.components {
            for dependency in &component.dependencies {
                if !descriptor_by_name.contains_key(dependency) {
                    return Err(RuntimeComponentGraphError::MissingDependency {
                        component: component.name,
                        dependency,
                    });
                }
            }
        }

        let mut visiting = Vec::new();
        let mut visited = HashSet::new();
        for component in &self.components {
            self.validate_component_dependencies(
                component.name,
                &descriptor_by_name,
                &mut visiting,
                &mut visited,
            )?;
        }

        Ok(())
    }

    fn validate_component_dependencies(
        &self,
        component_name: &'static str,
        descriptor_by_name: &HashMap<&'static str, &RuntimeComponentDescriptor>,
        visiting: &mut Vec<&'static str>,
        visited: &mut HashSet<&'static str>,
    ) -> Result<(), RuntimeComponentGraphError> {
        if visited.contains(component_name) {
            return Ok(());
        }
        if let Some(position) = visiting
            .iter()
            .position(|visiting_name| *visiting_name == component_name)
        {
            let mut cycle = visiting[position..].to_vec();
            cycle.push(component_name);
            return Err(RuntimeComponentGraphError::Cycle {
                cycle: cycle.join(" -> "),
            });
        }

        visiting.push(component_name);
        if let Some(descriptor) = descriptor_by_name.get(component_name) {
            for dependency in &descriptor.dependencies {
                self.validate_component_dependencies(
                    dependency,
                    descriptor_by_name,
                    visiting,
                    visited,
                )?;
            }
        }
        visiting.pop();
        visited.insert(component_name);
        Ok(())
    }

    /// Runs registered shutdown phases in component dependency order.
    ///
    /// A component's dependencies run before that component when both sides
    /// have shutdown phases. Multiple phases registered by one component run in
    /// registration order. Dependencies without shutdown phases are kept as
    /// descriptor metadata and do not block execution. Cycles are reported as
    /// warnings and the registry still makes best-effort progress without
    /// executing a phase more than once.
    pub async fn shutdown(&mut self) -> ShutdownReport {
        let mut reports = Vec::with_capacity(self.shutdown.len());
        for index in self.shutdown_order() {
            let registered = &mut self.shutdown[index];
            tracing::info!(phase = registered.phase_name, "starting shutdown phase");
            let started_at = std::time::Instant::now();
            let future = (registered.phase)();
            let status = match registered.timeout {
                Some(timeout) => match tokio::time::timeout(timeout, future).await {
                    Ok(Ok(())) => ShutdownPhaseStatus::Succeeded,
                    Ok(Err(error)) => ShutdownPhaseStatus::Failed(error),
                    Err(_) => ShutdownPhaseStatus::TimedOut,
                },
                None => match future.await {
                    Ok(()) => ShutdownPhaseStatus::Succeeded,
                    Err(error) => ShutdownPhaseStatus::Failed(error),
                },
            };
            let duration = started_at.elapsed();
            match &status {
                ShutdownPhaseStatus::Succeeded => {
                    tracing::info!(
                        phase = registered.phase_name,
                        ?duration,
                        "shutdown phase completed"
                    );
                }
                ShutdownPhaseStatus::Failed(error) => {
                    tracing::error!(
                        phase = registered.phase_name,
                        ?duration,
                        %error,
                        "shutdown phase failed"
                    );
                }
                ShutdownPhaseStatus::TimedOut => {
                    tracing::error!(
                        phase = registered.phase_name,
                        ?duration,
                        "shutdown phase timed out"
                    );
                }
            }
            reports.push(ShutdownPhaseReport {
                name: registered.phase_name,
                status,
                duration,
            });
        }

        ShutdownReport::new(reports)
    }

    fn shutdown_order(&self) -> Vec<usize> {
        let mut phase_indices_by_component: HashMap<&'static str, Vec<usize>> = HashMap::new();
        for (index, phase) in self.shutdown.iter().enumerate() {
            phase_indices_by_component
                .entry(phase.component_name)
                .or_default()
                .push(index);
        }
        let descriptor_by_name = self
            .components
            .iter()
            .map(|component| (component.name, component))
            .collect::<HashMap<_, _>>();
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut ordered = Vec::with_capacity(self.shutdown.len());

        for phase in &self.shutdown {
            self.push_shutdown_component_order(
                phase.component_name,
                &phase_indices_by_component,
                &descriptor_by_name,
                &mut visiting,
                &mut visited,
                &mut ordered,
            );
        }

        ordered
    }

    fn push_shutdown_component_order(
        &self,
        component_name: &'static str,
        phase_indices_by_component: &HashMap<&'static str, Vec<usize>>,
        descriptor_by_name: &HashMap<&'static str, &RuntimeComponentDescriptor>,
        visiting: &mut HashSet<&'static str>,
        visited: &mut HashSet<&'static str>,
        ordered: &mut Vec<usize>,
    ) {
        if visited.contains(component_name) {
            return;
        }
        if !visiting.insert(component_name) {
            tracing::warn!(
                component = component_name,
                "runtime component dependency cycle detected during shutdown ordering"
            );
            return;
        }

        if let Some(descriptor) = descriptor_by_name.get(component_name) {
            for dependency in &descriptor.dependencies {
                if phase_indices_by_component.contains_key(dependency) {
                    self.push_shutdown_component_order(
                        dependency,
                        phase_indices_by_component,
                        descriptor_by_name,
                        visiting,
                        visited,
                        ordered,
                    );
                }
            }
        }

        visiting.remove(component_name);
        visited.insert(component_name);
        // Dependencies are per-component, so the DFS stays per-component; when a
        // component is emitted, every phase it registered runs in registration order.
        if let Some(indices) = phase_indices_by_component.get(component_name) {
            ordered.extend(indices);
        }
    }

    /// Returns how many components are registered.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// Returns whether no components are registered.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

/// Builder for one runtime component registration.
pub struct RuntimeComponentBuilder<'a> {
    registry: &'a mut RuntimeComponentRegistry,
    index: usize,
}

impl RuntimeComponentBuilder<'_> {
    /// Sets the component category.
    pub fn kind(&mut self, kind: RuntimeComponentKind) -> &mut Self {
        self.descriptor_mut().kind = kind;
        self
    }

    /// Adds one component dependency.
    pub fn depends_on(&mut self, dependency: &'static str) -> &mut Self {
        let descriptor = self.descriptor_mut();
        if !descriptor.dependencies.contains(&dependency) {
            descriptor.dependencies.push(dependency);
        }
        self
    }

    /// Adds component dependencies in order.
    pub fn depends_on_all(&mut self, dependencies: &[&'static str]) -> &mut Self {
        for dependency in dependencies {
            self.depends_on(dependency);
        }
        self
    }

    /// Registers a component startup phase.
    pub fn startup<F, Fut>(
        &mut self,
        phase_name: &'static str,
        failure_policy: StartupPhaseFailurePolicy,
        phase: F,
    ) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.registry
            .startup
            .phase(phase_name, failure_policy, phase);
        self.descriptor_mut()
            .startup
            .push(RuntimeStartupDescriptor {
                phase_name,
                failure_policy,
            });
        self
    }

    /// Registers a component-owned runtime task descriptor.
    pub fn task(&mut self, task_name: &'static str, display_name: &'static str) -> &mut Self {
        self.descriptor_mut().tasks.push(RuntimeTaskDescriptor {
            task_name,
            display_name,
        });
        self
    }

    /// Registers a component health check with explicit options.
    pub fn health_with_options<F, Fut>(
        &mut self,
        check_name: &'static str,
        options: HealthCheckOptions,
        check: F,
    ) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthComponentReport> + Send + 'static,
    {
        self.registry
            .health
            .register_with_options(check_name, options, check);
        self.descriptor_mut()
            .health_checks
            .push(HealthCheckDescriptor {
                name: check_name,
                requirement: options.requirement,
                timeout: options.timeout,
                scopes: options.scopes,
            });
        self
    }

    /// Registers a component shutdown phase.
    ///
    /// A component may register multiple shutdown phases; they run in registration
    /// order, after the shutdown phases of the component's dependencies. Registering
    /// the same phase name twice for one component is almost always a duplicated
    /// registration accident, so it is reported with a warning, but every registered
    /// phase still runs.
    pub fn shutdown<F, Fut>(
        &mut self,
        phase_name: &'static str,
        timeout: Option<Duration>,
        mut phase: F,
    ) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let component_name = self.descriptor_mut().name;
        if self.registry.shutdown.iter().any(|registered| {
            registered.component_name == component_name && registered.phase_name == phase_name
        }) {
            tracing::warn!(
                component = component_name,
                phase = phase_name,
                "duplicate shutdown phase registration"
            );
        }
        self.registry.shutdown.push(RuntimeComponentShutdownPhase {
            component_name,
            phase_name,
            timeout,
            phase: Box::new(move || Box::pin(phase())),
        });
        self.descriptor_mut()
            .shutdown
            .push(RuntimeShutdownDescriptor {
                phase_name,
                timeout,
            });
        self
    }

    /// Registers a shutdown phase that consumes one owned value at most once.
    ///
    /// This is the common shape for database handles, background task sets,
    /// and other shutdown-only resources. Re-running the registry after the
    /// value has already been consumed becomes a no-op success.
    pub fn shutdown_once<T, F, Fut>(
        &mut self,
        phase_name: &'static str,
        timeout: Option<Duration>,
        value: T,
        phase: F,
    ) -> &mut Self
    where
        T: Send + 'static,
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let mut value = Some(value);
        let mut phase = Some(phase);
        self.shutdown(phase_name, timeout, move || {
            let value = value.take();
            let phase = phase.take();
            async move {
                if let (Some(value), Some(phase)) = (value, phase) {
                    phase(value).await?;
                }
                Ok(())
            }
        })
    }

    fn descriptor_mut(&mut self) -> &mut RuntimeComponentDescriptor {
        &mut self.registry.components[self.index]
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{RuntimeComponentBundle, RuntimeComponentKind, RuntimeComponentRegistry};
    use crate::{
        HealthCheckOptions, HealthCheckScope, HealthCheckScopes, HealthComponentReport,
        HealthStatus, ShutdownPhaseStatus, StartupPhaseFailurePolicy,
    };

    #[tokio::test]
    async fn registry_runs_component_health_checks_by_scope() {
        let mut registry = RuntimeComponentRegistry::new();
        registry.component_health_with_options(
            "database",
            RuntimeComponentKind::Database,
            "database",
            HealthCheckOptions::required(Some(Duration::from_secs(1)))
                .with_scopes(HealthCheckScopes::readiness_and_diagnostics()),
            || async { HealthComponentReport::healthy("database", "ok") },
        );
        registry.component_health_with_options(
            "cache",
            RuntimeComponentKind::Cache,
            "cache",
            HealthCheckOptions::optional(None).with_scopes(HealthCheckScopes::diagnostics()),
            || async { HealthComponentReport::degraded("cache", "fallback") },
        );

        let readiness = registry.run_health(HealthCheckScope::Readiness).await;
        let diagnostics = registry.run_health(HealthCheckScope::Diagnostics).await;

        assert_eq!(readiness.components.len(), 1);
        assert_eq!(readiness.components[0].name, "database");
        assert_eq!(diagnostics.components.len(), 2);
        assert_eq!(diagnostics.status(), HealthStatus::Degraded);
        assert_eq!(registry.descriptors()[0].health_checks[0].name, "database");
        assert_eq!(registry.descriptors()[1].health_checks[0].name, "cache");
    }

    #[tokio::test]
    async fn registry_runs_component_startup_and_records_task_descriptors() {
        let startup_events = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();
        registry
            .component("mail")
            .kind(RuntimeComponentKind::Mail)
            .startup("mail_templates", StartupPhaseFailurePolicy::Required, {
                let startup_events = Arc::clone(&startup_events);
                move || {
                    let startup_events = Arc::clone(&startup_events);
                    async move {
                        startup_events.lock().unwrap().push("mail_templates");
                        Ok(())
                    }
                }
            })
            .task("mail-outbox-dispatch", "Mail outbox dispatch");

        let report = registry.startup().await;

        assert!(!report.aborted());
        assert_eq!(
            startup_events.lock().unwrap().as_slice(),
            ["mail_templates"]
        );
        let descriptor = registry.descriptor("mail").expect("mail descriptor");
        assert_eq!(descriptor.startup[0].phase_name, "mail_templates");
        assert_eq!(
            descriptor.startup[0].failure_policy,
            StartupPhaseFailurePolicy::Required
        );
        assert_eq!(descriptor.tasks[0].task_name, "mail-outbox-dispatch");
        assert_eq!(descriptor.tasks[0].display_name, "Mail outbox dispatch");
    }

    fn register_database_component(registry: &mut RuntimeComponentRegistry) {
        registry.component_health_with_options(
            "database",
            RuntimeComponentKind::Database,
            "database",
            HealthCheckOptions::required(None),
            || async { HealthComponentReport::healthy("database", "ok") },
        );
    }

    fn register_cache_component(registry: &mut RuntimeComponentRegistry) {
        registry
            .component("cache")
            .kind(RuntimeComponentKind::Cache)
            .depends_on("database")
            .health_with_options(
                "cache",
                HealthCheckOptions::optional(None).with_scopes(HealthCheckScopes::diagnostics()),
                || async { HealthComponentReport::healthy("cache", "ok") },
            );
    }

    #[tokio::test]
    async fn registry_runs_shutdown_phases_in_dependency_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        registry
            .component("database")
            .kind(RuntimeComponentKind::Database)
            .depends_on_all(&["tasks"])
            .shutdown("database", None, {
                let order = Arc::clone(&order);
                move || {
                    let order = Arc::clone(&order);
                    async move {
                        order.lock().unwrap().push("database");
                        Err("close failed".to_string())
                    }
                }
            });
        registry.component_shutdown("tasks", RuntimeComponentKind::Tasks, "tasks", None, {
            let order = Arc::clone(&order);
            move || {
                let order = Arc::clone(&order);
                async move {
                    order.lock().unwrap().push("tasks");
                    Ok(())
                }
            }
        });

        let report = registry.shutdown().await;

        assert_eq!(order.lock().unwrap().as_slice(), ["tasks", "database"]);
        assert!(report.has_failures());
        assert_eq!(report.phases[0].status, ShutdownPhaseStatus::Succeeded);
        assert_eq!(
            report.phases[1].status,
            ShutdownPhaseStatus::Failed("close failed".to_string())
        );
        assert_eq!(
            registry
                .descriptor("database")
                .expect("database component should exist")
                .dependencies,
            vec!["tasks"]
        );
    }

    #[tokio::test]
    async fn registry_runs_deep_shutdown_graph_before_dependents() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        for (component, kind, dependencies) in [
            (
                "database",
                RuntimeComponentKind::Database,
                &["audit_manager"][..],
            ),
            (
                "audit_manager",
                RuntimeComponentKind::Product,
                &["audit_logs"][..],
            ),
            (
                "audit_logs",
                RuntimeComponentKind::Product,
                &["mail_outbox"][..],
            ),
            (
                "mail_outbox",
                RuntimeComponentKind::Mail,
                &["background_tasks"][..],
            ),
            ("background_tasks", RuntimeComponentKind::Tasks, &[][..]),
        ] {
            registry
                .component(component)
                .kind(kind)
                .depends_on_all(dependencies)
                .shutdown(component, None, {
                    let order = Arc::clone(&order);
                    move || {
                        let order = Arc::clone(&order);
                        async move {
                            order.lock().unwrap().push(component);
                            Ok(())
                        }
                    }
                });
        }

        let report = registry.shutdown().await;

        assert!(!report.has_failures());
        assert_eq!(
            order.lock().unwrap().as_slice(),
            [
                "background_tasks",
                "mail_outbox",
                "audit_logs",
                "audit_manager",
                "database"
            ]
        );
        assert_eq!(
            report
                .phases
                .iter()
                .map(|phase| phase.name)
                .collect::<Vec<_>>(),
            vec![
                "background_tasks",
                "mail_outbox",
                "audit_logs",
                "audit_manager",
                "database"
            ]
        );
    }

    #[tokio::test]
    async fn registry_runs_all_shutdown_phases_of_one_component_in_registration_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        {
            let mut component = registry.component("audit");
            for phase_name in ["audit_stop", "audit_flush", "audit_close"] {
                component.shutdown(phase_name, None, {
                    let order = Arc::clone(&order);
                    move || {
                        let order = Arc::clone(&order);
                        async move {
                            order.lock().unwrap().push(phase_name);
                            Ok(())
                        }
                    }
                });
            }
        }

        let report = registry.shutdown().await;

        assert!(!report.has_failures());
        assert_eq!(
            order.lock().unwrap().as_slice(),
            ["audit_stop", "audit_flush", "audit_close"]
        );
        assert_eq!(
            report
                .phases
                .iter()
                .map(|phase| phase.name)
                .collect::<Vec<_>>(),
            vec!["audit_stop", "audit_flush", "audit_close"]
        );
        assert_eq!(
            registry
                .descriptor("audit")
                .expect("audit component should exist")
                .shutdown
                .iter()
                .map(|descriptor| descriptor.phase_name)
                .collect::<Vec<_>>(),
            vec!["audit_stop", "audit_flush", "audit_close"]
        );
    }

    #[tokio::test]
    async fn registry_runs_dependency_phases_before_all_dependent_phases() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        {
            let mut database = registry.component("database");
            for phase_name in ["database_stop", "database_close"] {
                database.shutdown(phase_name, None, {
                    let order = Arc::clone(&order);
                    move || {
                        let order = Arc::clone(&order);
                        async move {
                            order.lock().unwrap().push(phase_name);
                            Ok(())
                        }
                    }
                });
            }
        }
        {
            let mut audit = registry.component("audit");
            audit.depends_on("database");
            for phase_name in ["audit_flush", "audit_close"] {
                audit.shutdown(phase_name, None, {
                    let order = Arc::clone(&order);
                    move || {
                        let order = Arc::clone(&order);
                        async move {
                            order.lock().unwrap().push(phase_name);
                            Ok(())
                        }
                    }
                });
            }
        }

        let report = registry.shutdown().await;

        assert!(!report.has_failures());
        assert_eq!(
            order.lock().unwrap().as_slice(),
            [
                "database_stop",
                "database_close",
                "audit_flush",
                "audit_close"
            ]
        );
    }

    #[derive(Clone)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for SharedLogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn duplicate_shutdown_phase_registration_warns_and_still_runs_both() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let buffer = SharedLogBuffer(Arc::new(Mutex::new(Vec::new())));
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .without_time()
            .finish();

        let mut registry = RuntimeComponentRegistry::new();
        tracing::subscriber::with_default(subscriber, || {
            let mut component = registry.component("audit");
            for run in [1_u8, 2] {
                component.shutdown("audit_flush", None, {
                    let order = Arc::clone(&order);
                    move || {
                        let order = Arc::clone(&order);
                        async move {
                            order.lock().unwrap().push(run);
                            Ok(())
                        }
                    }
                });
            }
        });

        let logs = String::from_utf8(buffer.0.lock().unwrap().clone())
            .expect("log output should be valid UTF-8");
        assert!(
            logs.contains("duplicate shutdown phase registration"),
            "expected duplicate registration warning, got: {logs}"
        );

        let report = registry.shutdown().await;
        assert!(!report.has_failures());
        assert_eq!(order.lock().unwrap().as_slice(), [1, 2]);
        assert_eq!(report.phases.len(), 2);
    }

    #[tokio::test]
    async fn registry_ignores_shutdown_dependencies_without_shutdown_phase() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        registry
            .component("cache")
            .kind(RuntimeComponentKind::Cache);
        registry
            .component("database")
            .kind(RuntimeComponentKind::Database)
            .depends_on("cache")
            .shutdown("database", None, {
                let order = Arc::clone(&order);
                move || {
                    let order = Arc::clone(&order);
                    async move {
                        order.lock().unwrap().push("database");
                        Ok(())
                    }
                }
            });

        let report = registry.shutdown().await;

        assert!(!report.has_failures());
        assert_eq!(order.lock().unwrap().as_slice(), ["database"]);
        assert_eq!(report.phases.len(), 1);
        assert_eq!(report.phases[0].name, "database");
    }

    #[test]
    fn registry_validate_rejects_missing_component_dependencies() {
        let mut registry = RuntimeComponentRegistry::new();
        registry
            .component("database")
            .kind(RuntimeComponentKind::Database)
            .depends_on("cache");

        let error = registry
            .validate()
            .expect_err("missing dependency should fail validation");

        assert_eq!(
            error,
            crate::RuntimeComponentGraphError::MissingDependency {
                component: "database",
                dependency: "cache"
            }
        );
    }

    #[test]
    fn registry_validate_rejects_dependency_cycles() {
        let mut registry = RuntimeComponentRegistry::new();
        registry.component("database").depends_on("audit");
        registry.component("audit").depends_on("database");

        let error = registry
            .validate()
            .expect_err("dependency cycle should fail validation");

        assert_eq!(
            error,
            crate::RuntimeComponentGraphError::Cycle {
                cycle: "database -> audit -> database".to_string()
            }
        );
    }

    #[tokio::test]
    async fn registry_shutdown_dependency_cycle_does_not_repeat_phases() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        for (component, dependency) in [("database", "audit"), ("audit", "database")] {
            registry
                .component(component)
                .kind(RuntimeComponentKind::Product)
                .depends_on(dependency)
                .shutdown(component, None, {
                    let order = Arc::clone(&order);
                    move || {
                        let order = Arc::clone(&order);
                        async move {
                            order.lock().unwrap().push(component);
                            Ok(())
                        }
                    }
                });
        }

        let report = registry.shutdown().await;
        let order = order.lock().unwrap();

        assert!(!report.has_failures());
        assert_eq!(order.len(), 2);
        assert!(order.contains(&"database"));
        assert!(order.contains(&"audit"));
        assert_eq!(report.phases.len(), 2);
    }

    #[tokio::test]
    async fn registry_shutdown_once_consumes_owned_value_once() {
        let values = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        registry.component_shutdown_once(
            "database",
            RuntimeComponentKind::Database,
            "database",
            None,
            "writer",
            {
                let values = Arc::clone(&values);
                move |value| async move {
                    values.lock().unwrap().push(value);
                    Ok(())
                }
            },
        );

        let first = registry.shutdown().await;
        let second = registry.shutdown().await;

        assert!(!first.has_failures());
        assert!(!second.has_failures());
        assert_eq!(values.lock().unwrap().as_slice(), ["writer"]);
        assert_eq!(
            registry
                .descriptor("database")
                .map(|descriptor| descriptor.kind),
            Some(RuntimeComponentKind::Database)
        );
    }

    struct TestShutdownBundle {
        values: Arc<Mutex<Vec<&'static str>>>,
    }

    impl RuntimeComponentBundle for TestShutdownBundle {
        fn register(self, registry: &mut RuntimeComponentRegistry) {
            registry.component_shutdown("audit", RuntimeComponentKind::Product, "audit", None, {
                let values = Arc::clone(&self.values);
                move || {
                    let values = Arc::clone(&values);
                    async move {
                        values.lock().unwrap().push("audit");
                        Ok(())
                    }
                }
            });
            registry
                .component("database")
                .kind(RuntimeComponentKind::Database)
                .depends_on("audit")
                .shutdown_once("database", None, self.values, |values| async move {
                    values.lock().unwrap().push("database");
                    Ok(())
                });
        }
    }

    #[tokio::test]
    async fn registry_accepts_owned_component_bundle() {
        let values = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();
        registry.register_bundle(TestShutdownBundle {
            values: Arc::clone(&values),
        });

        assert_eq!(registry.len(), 2);
        assert_eq!(
            registry
                .descriptor("database")
                .expect("database descriptor should exist")
                .dependencies,
            vec!["audit"]
        );

        let report = registry.shutdown().await;

        assert!(!report.has_failures());
        assert_eq!(values.lock().unwrap().as_slice(), ["audit", "database"]);
    }

    #[test]
    fn registry_accepts_closure_component_bundle() {
        let mut registry = RuntimeComponentRegistry::new();
        registry.register_bundle(|registry: &mut RuntimeComponentRegistry| {
            register_database_component(registry);
        });

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.descriptors()[0].name, "database");
    }

    #[test]
    fn registry_register_bundle_chains_multiple_component_bundles() {
        let mut registry = RuntimeComponentRegistry::new();
        registry
            .register_bundle(register_database_component)
            .register_bundle(register_cache_component);

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.descriptors()[0].name, "database");
        assert_eq!(registry.descriptors()[1].name, "cache");
    }

    #[tokio::test]
    async fn registry_can_shutdown_registered_component_bundle() {
        let values = Arc::new(Mutex::new(Vec::new()));

        let mut registry = RuntimeComponentRegistry::new();
        registry.register_bundle(TestShutdownBundle {
            values: Arc::clone(&values),
        });
        let report = registry.shutdown().await;

        assert!(!report.has_failures());
        assert_eq!(values.lock().unwrap().as_slice(), ["audit", "database"]);
    }
}

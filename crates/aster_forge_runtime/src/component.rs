//! Runtime component registration primitives.
//!
//! This module ties together the reusable runtime registries that already live
//! in this crate. It lets product crates describe subsystems once, then attach
//! health checks and shutdown phases without duplicating central dispatch
//! tables. Product crates still own resource construction, application state
//! assembly, business-specific startup ordering, and how reports are exposed.

use std::future::Future;
use std::time::Duration;

use crate::{
    HealthCheckDescriptor, HealthCheckOptions, HealthCheckRegistry, HealthCheckScope,
    HealthComponentReport, ShutdownCoordinator, ShutdownReport, SystemHealthReport,
};

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

/// Static metadata for a registered runtime component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeComponentDescriptor {
    /// Stable component name.
    pub name: &'static str,
    /// Broad component category.
    pub kind: RuntimeComponentKind,
    /// Stable names of components that should be initialized before this one.
    pub dependencies: Vec<&'static str>,
    /// Registered health checks owned by this component.
    pub health_checks: Vec<HealthCheckDescriptor>,
    /// Registered shutdown phase owned by this component, if any.
    pub shutdown: Option<RuntimeShutdownDescriptor>,
}

impl RuntimeComponentDescriptor {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            kind: RuntimeComponentKind::Product,
            dependencies: Vec::new(),
            health_checks: Vec::new(),
            shutdown: None,
        }
    }
}

/// Registry for runtime component metadata and lifecycle hooks.
#[derive(Default)]
pub struct RuntimeComponentRegistry {
    components: Vec<RuntimeComponentDescriptor>,
    health: HealthCheckRegistry,
    shutdown: ShutdownCoordinator,
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
    ///
    /// This mirrors Actix Web's `configure` style: subsystem modules receive a
    /// mutable registry and attach their own components without creating or
    /// owning the root registry.
    pub fn configure<F>(&mut self, configure: F) -> &mut Self
    where
        F: FnOnce(&mut Self),
    {
        configure(self);
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

    /// Runs registered shutdown phases.
    pub async fn shutdown(&mut self) -> ShutdownReport {
        self.shutdown.run().await
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
    pub fn shutdown<F, Fut>(
        &mut self,
        phase_name: &'static str,
        timeout: Option<Duration>,
        phase: F,
    ) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.registry.shutdown.phase(phase_name, timeout, phase);
        self.descriptor_mut().shutdown = Some(RuntimeShutdownDescriptor {
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

    use super::{RuntimeComponentKind, RuntimeComponentRegistry};
    use crate::{
        HealthCheckOptions, HealthCheckScope, HealthCheckScopes, HealthComponentReport,
        HealthStatus, ShutdownPhaseStatus,
    };

    #[tokio::test]
    async fn registry_runs_component_health_checks_by_scope() {
        let mut registry = RuntimeComponentRegistry::new();
        registry
            .component("database")
            .kind(RuntimeComponentKind::Database)
            .health_with_options(
                "database",
                HealthCheckOptions::required(Some(Duration::from_secs(1)))
                    .with_scopes(HealthCheckScopes::readiness_and_diagnostics()),
                || async { HealthComponentReport::healthy("database", "ok") },
            );
        registry
            .component("cache")
            .kind(RuntimeComponentKind::Cache)
            .health_with_options(
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
    async fn registry_runs_shutdown_phases_in_registration_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        registry
            .component("tasks")
            .kind(RuntimeComponentKind::Tasks)
            .shutdown("tasks", None, {
                let order = Arc::clone(&order);
                move || {
                    let order = Arc::clone(&order);
                    async move {
                        order.lock().unwrap().push("tasks");
                        Ok(())
                    }
                }
            });
        registry
            .component("database")
            .kind(RuntimeComponentKind::Database)
            .depends_on("tasks")
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
    async fn registry_shutdown_once_consumes_owned_value_once() {
        let values = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RuntimeComponentRegistry::new();

        registry
            .component("database")
            .shutdown_once("database", None, "writer", {
                let values = Arc::clone(&values);
                move |value| async move {
                    values.lock().unwrap().push(value);
                    Ok(())
                }
            });

        let first = registry.shutdown().await;
        let second = registry.shutdown().await;

        assert!(!first.has_failures());
        assert!(!second.has_failures());
        assert_eq!(values.lock().unwrap().as_slice(), ["writer"]);
    }
}

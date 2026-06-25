//! Service lifecycle runner.
//!
//! This module contains the shared entrypoint mechanics for Aster services.
//! Product crates still build their HTTP server, application state, background
//! workers, and business hooks, while Forge owns the repeated runtime flow:
//! register components, run startup phases, wait for termination, stop the main
//! service, run product before-shutdown hooks, run component shutdown phases,
//! and return the service output.

use std::future::Future;
use std::pin::Pin;

use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeComponentBundle, RuntimeComponentKind, RuntimeComponentRegistry, log_shutdown_report,
    spawn_termination_signal_handler,
};

type RuntimeHookFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type RuntimeHook = Box<dyn FnOnce() -> RuntimeHookFuture + Send>;
type RuntimeComponentRegistration = Box<dyn FnOnce(&mut RuntimeComponentRegistry) + Send>;

fn empty_runtime_hook() -> RuntimeHook {
    Box::new(|| Box::pin(async {}))
}

/// Runs one service future with shared Aster shutdown mechanics.
///
/// This lower-level helper exists for tests and uncommon one-off runners. New
/// product entrypoints should prefer [`AsterRuntime::builder`] and register the
/// main service through [`RuntimeServiceComponent`].
pub struct ServiceLifecycle<S> {
    server: S,
    shutdown_token: CancellationToken,
}

impl<S> ServiceLifecycle<S> {
    /// Creates a lifecycle runner for `server` and `shutdown_token`.
    pub fn new(server: S, shutdown_token: CancellationToken) -> Self {
        Self {
            server,
            shutdown_token,
        }
    }
}

/// Component that provides the main runtime service future.
///
/// Exactly one runtime service component must be registered. HTTP products
/// usually expose a small `http_component(...)` constructor that builds this
/// value from an Actix `Server`, its `ServerHandle`, and the shared shutdown
/// token.
pub struct RuntimeServiceComponent<S> {
    component_name: &'static str,
    kind: RuntimeComponentKind,
    service: S,
    shutdown_token: CancellationToken,
    stop_on_signal: RuntimeHook,
}

impl<S> RuntimeServiceComponent<S> {
    /// Creates a service component with a product-provided stop hook.
    pub fn new<F, Fut>(
        component_name: &'static str,
        kind: RuntimeComponentKind,
        service: S,
        shutdown_token: CancellationToken,
        stop_on_signal: F,
    ) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self {
            component_name,
            kind,
            service,
            shutdown_token,
            stop_on_signal: Box::new(move || Box::pin(stop_on_signal())),
        }
    }

    fn into_parts(self) -> RuntimeServiceParts<S> {
        RuntimeServiceParts {
            service: self.service,
            shutdown_token: self.shutdown_token,
            stop_on_signal: self.stop_on_signal,
        }
    }
}

struct RuntimeServiceParts<S> {
    service: S,
    shutdown_token: CancellationToken,
    stop_on_signal: RuntimeHook,
}

/// Runtime component adapter used by [`AsterRuntimeBuilder::component`].
///
/// Service components can change the builder's service future type. Registry
/// components keep the current builder type and only add lifecycle descriptors
/// and hooks.
pub trait AsterRuntimeComponent<S> {
    /// Builder type returned after this component is applied.
    type Output;

    /// Applies the component to the runtime builder.
    fn apply(self, builder: AsterRuntimeBuilder<S>) -> Self::Output;
}

/// Wrapper for components that only register runtime descriptors and hooks.
pub struct RuntimeComponentBundleRegistration<B> {
    bundle: B,
}

/// Adapts a [`RuntimeComponentBundle`] for [`AsterRuntimeBuilder::component`].
pub const fn runtime_component<B>(bundle: B) -> RuntimeComponentBundleRegistration<B> {
    RuntimeComponentBundleRegistration { bundle }
}

impl<S, Service> AsterRuntimeComponent<S> for RuntimeServiceComponent<Service> {
    type Output = AsterRuntimeBuilder<Service>;

    fn apply(self, mut builder: AsterRuntimeBuilder<S>) -> Self::Output {
        let component_name = self.component_name;
        let kind = self.kind;
        let assembly_error = if builder.service.is_some() {
            Some(AsterRuntimeError::DuplicateService)
        } else {
            builder.assembly_error
        };
        builder.components.push(Box::new(move |registry| {
            registry.component(component_name).kind(kind);
        }));

        AsterRuntimeBuilder {
            service: Some(self.into_parts()),
            before_shutdown: builder.before_shutdown,
            components: builder.components,
            assembly_error,
        }
    }
}

impl<S, B> AsterRuntimeComponent<S> for RuntimeComponentBundleRegistration<B>
where
    B: RuntimeComponentBundle + Send + 'static,
{
    type Output = AsterRuntimeBuilder<S>;

    fn apply(self, mut builder: AsterRuntimeBuilder<S>) -> Self::Output {
        builder
            .components
            .push(Box::new(move |registry| self.bundle.register(registry)));
        builder
    }
}

impl<B> RuntimeComponentBundle for RuntimeComponentBundleRegistration<B>
where
    B: RuntimeComponentBundle,
{
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        self.bundle.register(registry);
    }
}

/// Error returned when runtime assembly or required startup phases fail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AsterRuntimeError {
    /// No service component was registered.
    #[error("runtime requires exactly one service component")]
    MissingService,
    /// More than one service component was registered.
    #[error("runtime only supports one service component")]
    DuplicateService,
    /// Runtime component dependencies cannot be resolved.
    #[error("runtime component graph is invalid: {source}")]
    ComponentGraph {
        /// Underlying component graph validation error.
        source: crate::RuntimeComponentGraphError,
    },
    /// A required startup phase aborted runtime startup.
    #[error("runtime startup aborted by a required component phase")]
    Startup {
        /// Startup report that contains the failing phase.
        report: crate::StartupReport,
    },
}

/// Product-facing runtime runner built from service and component registrations.
///
/// Product entrypoints should register the main service as a component, then
/// register database, task, mail, product, or shutdown bundles in the same
/// chain:
///
/// ```ignore
/// AsterRuntime::builder()
///     .component(http_component(...))
///     .component(database_component(...))
///     .component(task_component(...))
///     .run()
///     .await?;
/// ```
pub struct AsterRuntime<S> {
    service: S,
    shutdown_token: CancellationToken,
    stop_on_signal: RuntimeHook,
    before_shutdown: RuntimeHook,
    registry: RuntimeComponentRegistry,
}

impl AsterRuntime<()> {
    /// Creates a runtime builder.
    pub fn builder() -> AsterRuntimeBuilder<()> {
        AsterRuntimeBuilder::new()
    }
}

impl<S> AsterRuntime<S>
where
    S: Future,
{
    /// Runs startup, the main service, before-shutdown hooks, and component shutdown.
    pub async fn run(mut self) -> Result<S::Output, AsterRuntimeError> {
        let startup_report = self.registry.startup().await;
        if startup_report.aborted() {
            return Err(AsterRuntimeError::Startup {
                report: startup_report,
            });
        }

        let _signal_task =
            spawn_termination_signal_handler(self.shutdown_token, self.stop_on_signal);

        let output = self.service.await;
        tracing::info!("service stopped");

        (self.before_shutdown)().await;

        let report = self.registry.shutdown().await;
        log_shutdown_report(&report);

        Ok(output)
    }
}

/// Builder for [`AsterRuntime`].
pub struct AsterRuntimeBuilder<S = ()> {
    service: Option<RuntimeServiceParts<S>>,
    before_shutdown: RuntimeHook,
    components: Vec<RuntimeComponentRegistration>,
    assembly_error: Option<AsterRuntimeError>,
}

impl AsterRuntimeBuilder<()> {
    fn new() -> Self {
        Self {
            service: None,
            before_shutdown: empty_runtime_hook(),
            components: Vec::new(),
            assembly_error: None,
        }
    }
}

impl<S> AsterRuntimeBuilder<S> {
    /// Adds one runtime component.
    pub fn component<C>(self, component: C) -> C::Output
    where
        C: AsterRuntimeComponent<S>,
    {
        component.apply(self)
    }

    /// Registers a product hook that runs after the service future stops and before components.
    pub fn before_shutdown<F, Fut>(mut self, before_shutdown: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.before_shutdown = Box::new(move || Box::pin(before_shutdown()));
        self
    }

    /// Builds the runtime runner.
    pub fn build(self) -> Result<AsterRuntime<S>, AsterRuntimeError> {
        if let Some(error) = self.assembly_error {
            return Err(error);
        }

        let Some(service) = self.service else {
            return Err(AsterRuntimeError::MissingService);
        };

        let mut registry = RuntimeComponentRegistry::new();
        for component in self.components {
            component(&mut registry);
        }
        registry
            .validate()
            .map_err(|source| AsterRuntimeError::ComponentGraph { source })?;

        Ok(AsterRuntime {
            service: service.service,
            shutdown_token: service.shutdown_token,
            stop_on_signal: service.stop_on_signal,
            before_shutdown: self.before_shutdown,
            registry,
        })
    }
}

impl<S> AsterRuntimeBuilder<S>
where
    S: Future,
{
    /// Builds and runs the runtime.
    pub async fn run(self) -> Result<S::Output, AsterRuntimeError> {
        self.build()?.run().await
    }
}

impl<S> ServiceLifecycle<S>
where
    S: Future,
{
    /// Runs the service future and product cleanup hooks.
    pub async fn run<Stop, StopFut, AfterStop, AfterStopFut>(
        self,
        stop_on_signal: Stop,
        after_stop: AfterStop,
    ) -> S::Output
    where
        Stop: FnOnce() -> StopFut + Send + 'static,
        StopFut: Future<Output = ()> + Send + 'static,
        AfterStop: FnOnce() -> AfterStopFut,
        AfterStopFut: Future<Output = ()>,
    {
        let _signal_task = spawn_termination_signal_handler(self.shutdown_token, stop_on_signal);

        let server_result = self.server.await;
        tracing::info!("server stopped");
        after_stop().await;
        server_result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::{AsterRuntime, AsterRuntimeError, RuntimeServiceComponent, ServiceLifecycle};
    use crate::{RuntimeComponentBundle, RuntimeComponentKind, RuntimeComponentRegistry};

    #[tokio::test]
    async fn lifecycle_runs_after_stop_and_returns_server_result() {
        let after_stop_ran = Arc::new(AtomicBool::new(false));
        let observed_after_stop = Arc::clone(&after_stop_ran);

        let result = ServiceLifecycle::new(async { Ok::<_, &'static str>(42) }, Default::default())
            .run(
                || async {},
                move || {
                    let observed_after_stop = Arc::clone(&observed_after_stop);
                    async move {
                        observed_after_stop.store(true, Ordering::SeqCst);
                    }
                },
            )
            .await;

        assert_eq!(result, Ok(42));
        assert!(after_stop_ran.load(Ordering::SeqCst));
    }

    struct TestShutdownComponent {
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    impl RuntimeComponentBundle for TestShutdownComponent {
        fn register(self, registry: &mut RuntimeComponentRegistry) {
            registry.component_shutdown(
                "test",
                RuntimeComponentKind::Product,
                "test_shutdown",
                None,
                move || {
                    let events = Arc::clone(&self.events);
                    async move {
                        events.lock().unwrap().push("component");
                        Ok(())
                    }
                },
            );
        }
    }

    #[tokio::test]
    async fn aster_runtime_runs_before_shutdown_and_registered_components() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let before_events = Arc::clone(&events);
        let component_events = Arc::clone(&events);

        let result = AsterRuntime::builder()
            .component(RuntimeServiceComponent::new(
                "http",
                RuntimeComponentKind::Core,
                async { Ok::<_, &'static str>(7) },
                Default::default(),
                || async {},
            ))
            .before_shutdown(move || {
                let before_events = Arc::clone(&before_events);
                async move {
                    before_events.lock().unwrap().push("before");
                }
            })
            .component(crate::runtime_component(TestShutdownComponent {
                events: component_events,
            }))
            .run()
            .await
            .expect("runtime should run");

        assert_eq!(result, Ok(7));
        assert_eq!(events.lock().unwrap().as_slice(), ["before", "component"]);
    }

    #[test]
    fn aster_runtime_requires_service_component() {
        let result = AsterRuntime::builder().build();
        assert!(matches!(result, Err(AsterRuntimeError::MissingService)));
    }

    #[test]
    fn aster_runtime_rejects_duplicate_service_components() {
        let result = AsterRuntime::builder()
            .component(RuntimeServiceComponent::new(
                "http",
                RuntimeComponentKind::Core,
                async {},
                Default::default(),
                || async {},
            ))
            .component(RuntimeServiceComponent::new(
                "worker",
                RuntimeComponentKind::Core,
                async {},
                Default::default(),
                || async {},
            ))
            .build();

        assert!(matches!(result, Err(AsterRuntimeError::DuplicateService)));
    }

    #[test]
    fn aster_runtime_rejects_invalid_component_graph() {
        let result = AsterRuntime::builder()
            .component(RuntimeServiceComponent::new(
                "http",
                RuntimeComponentKind::Core,
                async {},
                Default::default(),
                || async {},
            ))
            .component(crate::runtime_component(
                |registry: &mut RuntimeComponentRegistry| {
                    registry.component("database").depends_on("missing");
                },
            ))
            .build();

        assert!(matches!(
            result,
            Err(AsterRuntimeError::ComponentGraph {
                source: crate::RuntimeComponentGraphError::MissingDependency {
                    component: "database",
                    dependency: "missing"
                }
            })
        ));
    }
}

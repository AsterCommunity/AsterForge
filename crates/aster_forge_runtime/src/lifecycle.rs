//! Service lifecycle runner.
//!
//! This module contains the shared entrypoint mechanics for Aster services.
//! Product crates still build their HTTP server, application state, background
//! workers, and business hooks, while Forge owns the repeated runtime flow:
//! register components, run startup phases, wait for termination, stop the main
//! service, run product before-shutdown hooks, run component shutdown phases,
//! and return the service output.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeComponentBundle, RuntimeComponentKind, RuntimeComponentRegistry, log_shutdown_report,
    spawn_termination_signal_handler,
};

type RuntimeHookFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type RuntimeHook = Box<dyn FnOnce() -> RuntimeHookFuture + Send>;
type RuntimeComponentRegistration = Box<dyn FnOnce(&mut RuntimeComponentRegistry) + Send>;
type ShutdownResourceFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type ShutdownResourceFn<T> = Box<dyn FnOnce(T) -> ShutdownResourceFuture + Send>;

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

/// Component factory that receives the runtime's shared shutdown token.
pub struct RuntimeComponentWithShutdown<C, F> {
    build: F,
    _component: PhantomData<fn() -> C>,
}

/// Fallible component factory that receives the runtime's shared shutdown token.
pub struct TryRuntimeComponentWithShutdown<C, F, E> {
    build: F,
    _component: PhantomData<fn() -> C>,
    _error: PhantomData<fn() -> E>,
}

/// Adapts a [`RuntimeComponentBundle`] for [`AsterRuntimeBuilder::component`].
pub const fn runtime_component<B>(bundle: B) -> RuntimeComponentBundleRegistration<B> {
    RuntimeComponentBundleRegistration { bundle }
}

/// Builds one runtime component from the runtime's shared shutdown token.
///
/// Use this when a product component needs the same token that `AsterRuntime`
/// cancels on termination signals, for example an HTTP server, config reload
/// subscription, or background worker group.
pub fn runtime_component_with_shutdown<C, F>(build: F) -> RuntimeComponentWithShutdown<C, F>
where
    F: FnOnce(CancellationToken) -> C,
{
    RuntimeComponentWithShutdown {
        build,
        _component: PhantomData,
    }
}

/// Fallible variant of [`runtime_component_with_shutdown`].
pub fn try_runtime_component_with_shutdown<C, F, E>(
    build: F,
) -> TryRuntimeComponentWithShutdown<C, F, E>
where
    F: FnOnce(CancellationToken) -> Result<C, E>,
{
    TryRuntimeComponentWithShutdown {
        build,
        _component: PhantomData,
        _error: PhantomData,
    }
}

/// Runtime component for one shutdown-only owned resource.
///
/// This adapter is useful for product-owned resources whose lifecycle is
/// otherwise simple: declare a component, optional dependencies, and a shutdown
/// phase that consumes the resource exactly once. Product crates still own the
/// resource type and the shutdown closure; Forge owns the component boilerplate.
pub struct ShutdownResourceComponent<T> {
    component_name: &'static str,
    kind: RuntimeComponentKind,
    phase_name: &'static str,
    dependencies: &'static [&'static str],
    resource: T,
    shutdown: ShutdownResourceFn<T>,
}

impl<T> ShutdownResourceComponent<T> {
    /// Creates a shutdown-only resource component.
    pub fn new<F, Fut>(
        component_name: &'static str,
        kind: RuntimeComponentKind,
        phase_name: &'static str,
        resource: T,
        shutdown: F,
    ) -> Self
    where
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Self {
            component_name,
            kind,
            phase_name,
            dependencies: &[],
            resource,
            shutdown: Box::new(move |resource| Box::pin(shutdown(resource))),
        }
    }

    /// Declares components that must shut down before this resource.
    pub const fn depends_on_all(mut self, dependencies: &'static [&'static str]) -> Self {
        self.dependencies = dependencies;
        self
    }
}

impl<T> RuntimeComponentBundle for ShutdownResourceComponent<T>
where
    T: Send + 'static,
{
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        let Self {
            component_name,
            kind,
            phase_name,
            dependencies,
            resource,
            shutdown,
        } = self;
        registry
            .component(component_name)
            .kind(kind)
            .depends_on_all(dependencies)
            .shutdown_once(phase_name, None, resource, shutdown);
    }
}

/// Creates a shutdown-only resource component registration.
pub fn shutdown_resource_component<T, F, Fut>(
    component_name: &'static str,
    kind: RuntimeComponentKind,
    phase_name: &'static str,
    resource: T,
    shutdown: F,
) -> RuntimeComponentBundleRegistration<ShutdownResourceComponent<T>>
where
    T: Send + 'static,
    F: FnOnce(T) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    runtime_component(ShutdownResourceComponent::new(
        component_name,
        kind,
        phase_name,
        resource,
        shutdown,
    ))
}

/// Creates a shutdown-only resource component registration with dependencies.
pub fn shutdown_resource_component_after<T, F, Fut>(
    component_name: &'static str,
    kind: RuntimeComponentKind,
    phase_name: &'static str,
    dependencies: &'static [&'static str],
    resource: T,
    shutdown: F,
) -> RuntimeComponentBundleRegistration<ShutdownResourceComponent<T>>
where
    T: Send + 'static,
    F: FnOnce(T) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    runtime_component(
        ShutdownResourceComponent::new(component_name, kind, phase_name, resource, shutdown)
            .depends_on_all(dependencies),
    )
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
            shutdown_token: builder.shutdown_token,
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

impl<S, C, F> AsterRuntimeComponent<S> for RuntimeComponentWithShutdown<C, F>
where
    C: AsterRuntimeComponent<S>,
    F: FnOnce(CancellationToken) -> C,
{
    type Output = C::Output;

    fn apply(self, builder: AsterRuntimeBuilder<S>) -> Self::Output {
        let component = (self.build)(builder.shutdown_token.clone());
        component.apply(builder)
    }
}

impl<S, C, F, E> AsterRuntimeComponent<S> for TryRuntimeComponentWithShutdown<C, F, E>
where
    C: AsterRuntimeComponent<S>,
    F: FnOnce(CancellationToken) -> Result<C, E>,
{
    type Output = Result<C::Output, E>;

    fn apply(self, builder: AsterRuntimeBuilder<S>) -> Self::Output {
        let component = (self.build)(builder.shutdown_token.clone())?;
        Ok(component.apply(builder))
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
    shutdown_token: CancellationToken,
    before_shutdown: RuntimeHook,
    components: Vec<RuntimeComponentRegistration>,
    assembly_error: Option<AsterRuntimeError>,
}

impl AsterRuntimeBuilder<()> {
    fn new() -> Self {
        Self {
            service: None,
            shutdown_token: CancellationToken::new(),
            before_shutdown: empty_runtime_hook(),
            components: Vec::new(),
            assembly_error: None,
        }
    }
}

impl<S> AsterRuntimeBuilder<S> {
    /// Returns the runtime-owned shutdown token.
    ///
    /// This accessor is primarily for shared component crates that implement
    /// [`AsterRuntimeComponent`] and need to spawn work using the same token the runtime cancels
    /// when the process receives a termination signal.
    pub fn shutdown_token(&self) -> &CancellationToken {
        &self.shutdown_token
    }

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

    #[tokio::test]
    async fn aster_runtime_builder_shares_shutdown_token_with_components() {
        let observed = Arc::new(AtomicBool::new(false));
        let observed_component = Arc::clone(&observed);

        let result = AsterRuntime::builder()
            .component(crate::runtime_component_with_shutdown(|shutdown| {
                let component_token = shutdown.clone();
                RuntimeServiceComponent::new(
                    "http",
                    RuntimeComponentKind::Core,
                    async move {
                        component_token.cancel();
                        Ok::<_, &'static str>(())
                    },
                    shutdown,
                    || async {},
                )
            }))
            .component(crate::runtime_component_with_shutdown(|shutdown| {
                crate::runtime_component(move |registry: &mut RuntimeComponentRegistry| {
                    registry.component_shutdown(
                        "observer",
                        RuntimeComponentKind::Product,
                        "observe_shared_shutdown",
                        None,
                        move || {
                            let observed_component = Arc::clone(&observed_component);
                            let shutdown = shutdown.clone();
                            async move {
                                observed_component.store(shutdown.is_cancelled(), Ordering::SeqCst);
                                Ok(())
                            }
                        },
                    );
                })
            }))
            .run()
            .await
            .expect("runtime should run");

        assert_eq!(result, Ok(()));
        assert!(observed.load(Ordering::SeqCst));
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

    #[test]
    fn shutdown_resource_component_registers_dependencies_and_shutdown() {
        let registry = RuntimeComponentRegistry::configured(|registry| {
            crate::shutdown_resource_component_after(
                "mail_outbox",
                RuntimeComponentKind::Mail,
                "mail_outbox_drain",
                &["background_tasks"],
                42_u8,
                |_| async { Ok(()) },
            )
            .register(registry);
        });

        let descriptor = registry
            .descriptor("mail_outbox")
            .expect("shutdown resource component should be registered");
        assert_eq!(descriptor.kind, RuntimeComponentKind::Mail);
        assert_eq!(descriptor.dependencies, vec!["background_tasks"]);
        assert_eq!(
            descriptor
                .shutdown
                .expect("shutdown phase should be registered")
                .phase_name,
            "mail_outbox_drain"
        );
    }
}

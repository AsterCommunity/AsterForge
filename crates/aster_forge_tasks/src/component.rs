//! Runtime component integration for background task collections.
//!
//! Products still decide which workers to spawn and which task descriptors they
//! expose to operators. Forge owns the common lifecycle mechanics for the
//! spawned handle collection: registering the `background_tasks` component,
//! attaching static task metadata, and shutting down all workers exactly once
//! during dependency-aware runtime shutdown.

use aster_forge_runtime::{
    AsterRuntimeBuilder, AsterRuntimeComponent, RuntimeComponentBundle,
    RuntimeComponentBundleRegistration, RuntimeComponentKind, RuntimeComponentRegistry,
    runtime_component,
};
use tokio_util::sync::CancellationToken;

use crate::{BACKGROUND_TASKS_COMPONENT, BackgroundTasks, RuntimeTaskDefinition};

/// Stable shutdown phase name for background task workers.
pub const BACKGROUND_TASKS_SHUTDOWN_PHASE: &str = "background_tasks";

/// Runtime component that owns spawned background task handles.
pub struct BackgroundTaskRuntimeComponent {
    background_tasks: BackgroundTasks,
}

/// Runtime component that owns spawned task handles and registers task definitions.
pub struct BackgroundTaskRuntimeDefinitionsComponent<Kind: 'static, PresentationCode: 'static> {
    background_tasks: BackgroundTasks,
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
}

/// Runtime builder component that spawns background tasks from the shared shutdown token.
pub struct BackgroundTaskRuntimeComponentFromShutdown<F> {
    spawn: F,
}

/// Runtime builder component that spawns background tasks and registers task definitions.
pub struct BackgroundTaskRuntimeDefinitionsComponentFromShutdown<
    Kind: 'static,
    PresentationCode: 'static,
    F,
> {
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
    spawn: F,
}

impl BackgroundTaskRuntimeComponent {
    /// Creates a background task runtime component from spawned task handles.
    pub const fn new(background_tasks: BackgroundTasks) -> Self {
        Self { background_tasks }
    }
}

impl<F> BackgroundTaskRuntimeComponentFromShutdown<F> {
    /// Creates a component from a worker-spawning function.
    pub const fn new(spawn: F) -> Self {
        Self { spawn }
    }
}

impl<Kind: 'static, PresentationCode: 'static>
    BackgroundTaskRuntimeDefinitionsComponent<Kind, PresentationCode>
{
    /// Creates a background task runtime component with product-owned task definitions.
    pub const fn new(
        background_tasks: BackgroundTasks,
        definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
    ) -> Self {
        Self {
            background_tasks,
            definitions,
        }
    }
}

impl<Kind: 'static, PresentationCode: 'static, F>
    BackgroundTaskRuntimeDefinitionsComponentFromShutdown<Kind, PresentationCode, F>
{
    /// Creates a component from static task definitions and a worker-spawning function.
    pub const fn new(
        definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
        spawn: F,
    ) -> Self {
        Self { definitions, spawn }
    }
}

impl RuntimeComponentBundle for BackgroundTaskRuntimeComponent {
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        register_background_tasks_shutdown(registry, self.background_tasks);
    }
}

impl<Kind: 'static, PresentationCode: 'static> RuntimeComponentBundle
    for BackgroundTaskRuntimeDefinitionsComponent<Kind, PresentationCode>
{
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        register_background_task_definitions(registry, self.definitions);
        register_background_tasks_shutdown(registry, self.background_tasks);
    }
}

impl<S, F> AsterRuntimeComponent<S> for BackgroundTaskRuntimeComponentFromShutdown<F>
where
    F: FnOnce(CancellationToken) -> BackgroundTasks,
{
    type Output = AsterRuntimeBuilder<S>;

    fn apply(self, builder: AsterRuntimeBuilder<S>) -> Self::Output {
        let background_tasks = (self.spawn)(builder.shutdown_token().clone());
        background_task_component(background_tasks).apply(builder)
    }
}

impl<S, Kind, PresentationCode, F> AsterRuntimeComponent<S>
    for BackgroundTaskRuntimeDefinitionsComponentFromShutdown<Kind, PresentationCode, F>
where
    Kind: Sync + 'static,
    PresentationCode: Sync + 'static,
    F: FnOnce(CancellationToken) -> BackgroundTasks,
{
    type Output = AsterRuntimeBuilder<S>;

    fn apply(self, builder: AsterRuntimeBuilder<S>) -> Self::Output {
        let background_tasks = (self.spawn)(builder.shutdown_token().clone());
        background_task_component_with_definitions(background_tasks, self.definitions)
            .apply(builder)
    }
}

/// Creates the background task runtime component used by product entrypoints.
pub fn background_task_component(
    background_tasks: BackgroundTasks,
) -> RuntimeComponentBundleRegistration<BackgroundTaskRuntimeComponent> {
    runtime_component(BackgroundTaskRuntimeComponent::new(background_tasks))
}

/// Creates a runtime component that spawns background tasks from the shared shutdown token.
///
/// Use this from product entrypoints when worker creation needs the same shutdown token owned by
/// `AsterRuntime`. Forge handles the runtime-component adapter; product code only supplies the
/// worker spawning function.
pub fn background_task_component_from_shutdown<F>(
    spawn: F,
) -> BackgroundTaskRuntimeComponentFromShutdown<F>
where
    F: FnOnce(CancellationToken) -> BackgroundTasks,
{
    BackgroundTaskRuntimeComponentFromShutdown::new(spawn)
}

/// Creates the background task runtime component with product task definitions.
///
/// This is the preferred companion for task catalogs generated by
/// [`crate::runtime_task_registry!`]. Products keep their enum and presentation
/// code, while Forge registers the runtime task descriptor fields from the
/// shared definition list.
pub fn background_task_component_with_definitions<Kind, PresentationCode>(
    background_tasks: BackgroundTasks,
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
) -> RuntimeComponentBundleRegistration<
    BackgroundTaskRuntimeDefinitionsComponent<Kind, PresentationCode>,
>
where
    Kind: Sync + 'static,
    PresentationCode: Sync + 'static,
{
    runtime_component(BackgroundTaskRuntimeDefinitionsComponent::new(
        background_tasks,
        definitions,
    ))
}

/// Creates a task-definition component that spawns workers from the shared shutdown token.
///
/// This is the high-level component factory for Aster products that use
/// `AsterRuntime::builder().component(...)`: product code passes its static task definitions and
/// one worker-spawning function, while Forge owns the adapter between the runtime shutdown token and
/// the background-task shutdown component.
pub fn background_task_component_with_definitions_from_shutdown<Kind, PresentationCode, F>(
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
    spawn: F,
) -> BackgroundTaskRuntimeDefinitionsComponentFromShutdown<Kind, PresentationCode, F>
where
    Kind: Sync + 'static,
    PresentationCode: Sync + 'static,
    F: FnOnce(CancellationToken) -> BackgroundTasks,
{
    BackgroundTaskRuntimeDefinitionsComponentFromShutdown::new(definitions, spawn)
}

/// Registers graceful shutdown for all spawned runtime background tasks.
fn register_background_tasks_shutdown(
    registry: &mut RuntimeComponentRegistry,
    background_tasks: BackgroundTasks,
) {
    registry.component_shutdown_once(
        BACKGROUND_TASKS_COMPONENT,
        RuntimeComponentKind::Tasks,
        BACKGROUND_TASKS_SHUTDOWN_PHASE,
        None,
        background_tasks,
        |background_tasks| async move {
            background_tasks.shutdown().await;
            Ok(())
        },
    );
}

/// Registers static metadata from product runtime task definitions.
fn register_background_task_definitions<Kind, PresentationCode>(
    registry: &mut RuntimeComponentRegistry,
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
) {
    for task in definitions {
        registry.component_task(
            BACKGROUND_TASKS_COMPONENT,
            RuntimeComponentKind::Tasks,
            task.wire_value,
            task.display_name,
        );
    }
}

#[cfg(test)]
mod tests {
    use aster_forge_runtime::{RuntimeComponentBundle, RuntimeComponentKind};

    use super::{
        BACKGROUND_TASKS_COMPONENT, BACKGROUND_TASKS_SHUTDOWN_PHASE,
        background_task_component_from_shutdown, background_task_component_with_definitions,
    };
    use crate::{BackgroundTasks, RuntimeTaskDefinition};

    const TEST_DEFINITIONS: &[RuntimeTaskDefinition<TestRuntimeTask, TestPresentationCode>] = &[
        RuntimeTaskDefinition {
            kind: TestRuntimeTask::Cleanup,
            wire_value: "cleanup",
            display_name: "Cleanup",
            presentation_code: TestPresentationCode::Cleanup,
        },
        RuntimeTaskDefinition {
            kind: TestRuntimeTask::Dispatch,
            wire_value: "dispatch",
            display_name: "Dispatch",
            presentation_code: TestPresentationCode::Dispatch,
        },
    ];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestRuntimeTask {
        Cleanup,
        Dispatch,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestPresentationCode {
        Cleanup,
        Dispatch,
    }

    #[test]
    fn background_task_component_registers_definitions_and_shutdown() {
        let registry = aster_forge_runtime::RuntimeComponentRegistry::configured(|registry| {
            background_task_component_with_definitions(BackgroundTasks::new(), TEST_DEFINITIONS)
                .register(registry);
        });

        let descriptor = registry
            .descriptor(BACKGROUND_TASKS_COMPONENT)
            .expect("background task component should be registered");
        assert_eq!(descriptor.kind, RuntimeComponentKind::Tasks);
        assert_eq!(
            descriptor
                .shutdown
                .expect("background task shutdown should be registered")
                .phase_name,
            BACKGROUND_TASKS_SHUTDOWN_PHASE
        );
        assert_eq!(
            descriptor
                .tasks
                .iter()
                .map(|task| (task.task_name, task.display_name))
                .collect::<Vec<_>>(),
            vec![("cleanup", "Cleanup"), ("dispatch", "Dispatch")]
        );
    }

    #[tokio::test]
    async fn background_task_component_from_shutdown_uses_runtime_shutdown_token() {
        let observed = std::sync::Arc::new(std::sync::Mutex::new(false));
        let observed_spawn = observed.clone();
        let runtime = aster_forge_runtime::AsterRuntime::builder()
            .component(aster_forge_runtime::RuntimeServiceComponent::new(
                "test_service",
                RuntimeComponentKind::Core,
                async {},
                Default::default(),
                || async {},
            ))
            .component(background_task_component_from_shutdown(move |shutdown| {
                let mut tasks = BackgroundTasks::with_shutdown_token(shutdown.clone());
                let observed_task = observed_spawn.clone();
                tasks.push(async move {
                    shutdown.cancelled().await;
                    match observed_task.lock() {
                        Ok(mut value) => *value = true,
                        Err(poisoned) => *poisoned.into_inner() = true,
                    }
                });
                tasks
            }))
            .build()
            .expect("runtime should build with spawned background task component");

        runtime
            .run()
            .await
            .expect("runtime should shut down cleanly");
        assert!(
            *observed
                .lock()
                .expect("observed mutex should not be poisoned")
        );
    }
}

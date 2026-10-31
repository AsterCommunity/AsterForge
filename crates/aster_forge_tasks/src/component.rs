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
    name: &'static str,
}

/// Runtime component that owns spawned task handles and registers task definitions.
pub struct BackgroundTaskRuntimeDefinitionsComponent<Kind: 'static, PresentationCode: 'static> {
    background_tasks: BackgroundTasks,
    filter: fn(&Kind) -> bool,
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
    name: &'static str,
}

/// Runtime builder component that spawns background tasks from the shared shutdown token.
pub struct BackgroundTaskRuntimeComponentFromShutdown<F> {
    stop_on_signal: bool,
    spawn: F,
    name: &'static str,
}

/// Runtime builder component that spawns background tasks and registers task definitions.
pub struct BackgroundTaskRuntimeDefinitionsComponentFromShutdown<
    Kind: 'static,
    PresentationCode: 'static,
    F,
> {
    filter: fn(&Kind) -> bool,
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
    stop_on_signal: bool,
    spawn: F,
    name: &'static str,
}

impl BackgroundTaskRuntimeComponent {
    /// Gives this task group an independent runtime identity and shutdown phase owner.
    #[must_use]
    pub const fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Creates a background task runtime component from spawned task handles.
    #[must_use]
    pub const fn new(background_tasks: BackgroundTasks) -> Self {
        Self {
            background_tasks,
            name: BACKGROUND_TASKS_COMPONENT,
        }
    }
}

impl<F> BackgroundTaskRuntimeComponentFromShutdown<F> {
    /// Keeps process-owned maintenance alive until the main service and its producers stop.
    #[must_use]
    pub const fn after_service(mut self) -> Self {
        self.stop_on_signal = false;
        self
    }

    /// Gives this task group an independent runtime identity and shutdown phase owner.
    #[must_use]
    pub const fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Creates a component from a worker-spawning function.
    pub const fn new(spawn: F) -> Self {
        Self {
            spawn,
            stop_on_signal: true,
            name: BACKGROUND_TASKS_COMPONENT,
        }
    }
}

impl<Kind: 'static, PresentationCode: 'static>
    BackgroundTaskRuntimeDefinitionsComponent<Kind, PresentationCode>
{
    /// Registers only metadata for the tasks actually mounted in this group.
    #[must_use]
    pub fn with_definition_filter(mut self, filter: fn(&Kind) -> bool) -> Self {
        self.filter = filter;
        self
    }

    /// Gives this task group an independent runtime identity and shutdown phase owner.
    #[must_use]
    pub const fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Creates a background task runtime component with product-owned task definitions.
    pub const fn new(
        background_tasks: BackgroundTasks,
        definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
    ) -> Self {
        Self {
            background_tasks,
            definitions,
            filter: |_| true,
            name: BACKGROUND_TASKS_COMPONENT,
        }
    }
}

impl<Kind: 'static, PresentationCode: 'static, F>
    BackgroundTaskRuntimeDefinitionsComponentFromShutdown<Kind, PresentationCode, F>
{
    /// Keeps process-owned maintenance alive until the main service and its producers stop.
    #[must_use]
    pub const fn after_service(mut self) -> Self {
        self.stop_on_signal = false;
        self
    }

    /// Registers only metadata for the tasks actually mounted in this group.
    #[must_use]
    pub fn with_definition_filter(mut self, filter: fn(&Kind) -> bool) -> Self {
        self.filter = filter;
        self
    }

    /// Gives this task group an independent runtime identity and shutdown phase owner.
    #[must_use]
    pub const fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Creates a component from static task definitions and a worker-spawning function.
    pub const fn new(
        definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
        spawn: F,
    ) -> Self {
        Self {
            definitions,
            filter: |_| true,
            spawn,
            stop_on_signal: true,
            name: BACKGROUND_TASKS_COMPONENT,
        }
    }
}

impl RuntimeComponentBundle for BackgroundTaskRuntimeComponent {
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        register_background_tasks_shutdown(registry, self.name, self.background_tasks);
    }
}

impl<Kind: 'static, PresentationCode: 'static> RuntimeComponentBundle
    for BackgroundTaskRuntimeDefinitionsComponent<Kind, PresentationCode>
{
    fn register(self, registry: &mut RuntimeComponentRegistry) {
        register_background_task_definitions(registry, self.name, self.definitions, self.filter);
        register_background_tasks_shutdown(registry, self.name, self.background_tasks);
    }
}

impl<S, F> AsterRuntimeComponent<S> for BackgroundTaskRuntimeComponentFromShutdown<F>
where
    F: FnOnce(CancellationToken) -> BackgroundTasks,
{
    type Output = AsterRuntimeBuilder<S>;

    fn apply(self, builder: AsterRuntimeBuilder<S>) -> Self::Output {
        let stop = if self.stop_on_signal {
            builder.shutdown_token().clone()
        } else {
            CancellationToken::new()
        };
        let background_tasks = (self.spawn)(stop);
        runtime_component(BackgroundTaskRuntimeComponent::new(background_tasks).named(self.name))
            .apply(builder)
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
        let stop = if self.stop_on_signal {
            builder.shutdown_token().clone()
        } else {
            CancellationToken::new()
        };
        let background_tasks = (self.spawn)(stop);
        runtime_component(
            BackgroundTaskRuntimeDefinitionsComponent::new(background_tasks, self.definitions)
                .named(self.name)
                .with_definition_filter(self.filter),
        )
        .apply(builder)
    }
}

/// Creates the background task runtime component used by product entrypoints.
#[must_use]
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
    name: &'static str,
    background_tasks: BackgroundTasks,
) {
    registry.component_shutdown_once(
        name,
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
    name: &'static str,
    definitions: &'static [RuntimeTaskDefinition<Kind, PresentationCode>],
    filter: fn(&Kind) -> bool,
) {
    for task in definitions.iter().filter(|task| filter(&task.kind)) {
        registry.component_task(
            name,
            RuntimeComponentKind::Tasks,
            task.wire_value,
            task.display_name,
        );
    }
}

#[cfg(test)]
mod tests {
    use aster_forge_runtime::{RuntimeComponentBundle, RuntimeComponentKind};
    use tokio_util::sync::CancellationToken;

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
                .first()
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
    async fn named_groups_register_only_their_own_definitions_and_shutdown_resources() {
        let mut registry = aster_forge_runtime::RuntimeComponentRegistry::new();
        super::BackgroundTaskRuntimeDefinitionsComponent::new(
            BackgroundTasks::new(),
            TEST_DEFINITIONS,
        )
        .named("consumer")
        .with_definition_filter(|kind| *kind == TestRuntimeTask::Dispatch)
        .register(&mut registry);
        super::BackgroundTaskRuntimeDefinitionsComponent::new(
            BackgroundTasks::new(),
            TEST_DEFINITIONS,
        )
        .named("scheduler")
        .with_definition_filter(|kind| *kind == TestRuntimeTask::Cleanup)
        .register(&mut registry);
        registry.validate().unwrap();
        assert!(registry.descriptor(BACKGROUND_TASKS_COMPONENT).is_none());
        assert_eq!(
            registry.descriptor("consumer").unwrap().tasks[0].task_name,
            "dispatch"
        );
        assert_eq!(
            registry.descriptor("scheduler").unwrap().tasks[0].task_name,
            "cleanup"
        );
        assert!(!registry.shutdown().await.has_failures());
    }

    #[tokio::test]
    async fn process_maintenance_stops_after_the_main_service_instead_of_on_signal() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let capture = captured.clone();
        let builder = aster_forge_runtime::AsterRuntime::builder();
        let signal = builder.shutdown_token().clone();
        let builder = builder.component(
            super::background_task_component_with_definitions_from_shutdown(
                TEST_DEFINITIONS,
                move |stop: CancellationToken| {
                    *capture.lock().unwrap() = Some(stop.clone());
                    BackgroundTasks::with_shutdown_token(stop)
                },
            )
            .after_service(),
        );
        signal.cancel();
        assert!(!captured.lock().unwrap().as_ref().unwrap().is_cancelled());
        builder
            .component(aster_forge_runtime::RuntimeServiceComponent::new(
                "test_service",
                RuntimeComponentKind::Core,
                async {},
                CancellationToken::new(),
                || async {},
            ))
            .run()
            .await
            .unwrap();
        assert!(captured.lock().unwrap().as_ref().unwrap().is_cancelled());
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
                CancellationToken::default(),
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
    #[tokio::test]
    async fn named_factories_isolate_signal_and_process_maintenance_tokens() {
        for after_service in [false, true] {
            let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let capture = observed.clone();
            let builder = aster_forge_runtime::AsterRuntime::builder();
            let signal = builder.shutdown_token().clone();
            let component = super::background_task_component_from_shutdown(move |stop| {
                capture.lock().unwrap().push(stop.clone());
                BackgroundTasks::with_shutdown_token(stop)
            })
            .named("local_maintenance");
            let builder = if after_service {
                builder.component(component.after_service())
            } else {
                builder.component(component)
            };
            let capture = observed.clone();
            let builder = builder.component(
                super::background_task_component_with_definitions_from_shutdown(
                    TEST_DEFINITIONS,
                    move |stop| {
                        capture.lock().unwrap().push(stop.clone());
                        BackgroundTasks::with_shutdown_token(stop)
                    },
                )
                .named("task_consumers")
                .with_definition_filter(|kind| *kind == TestRuntimeTask::Dispatch),
            );
            signal.cancel();
            {
                let tokens = observed.lock().unwrap();
                assert_eq!(tokens[0].is_cancelled(), !after_service);
                assert!(tokens[1].is_cancelled());
            }
            let metadata_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let checked = metadata_seen.clone();
            let builder = builder.component(aster_forge_runtime::runtime_component(
                move |registry: &mut aster_forge_runtime::RuntimeComponentRegistry| {
                    assert!(registry.descriptor(BACKGROUND_TASKS_COMPONENT).is_none());
                    assert_eq!(
                        registry.descriptor("task_consumers").unwrap().tasks.len(),
                        1
                    );
                    assert_eq!(
                        registry.descriptor("task_consumers").unwrap().tasks[0].task_name,
                        "dispatch"
                    );
                    assert!(
                        registry
                            .descriptor("local_maintenance")
                            .unwrap()
                            .tasks
                            .is_empty()
                    );
                    checked.store(true, std::sync::atomic::Ordering::SeqCst);
                },
            ));
            builder
                .component(aster_forge_runtime::RuntimeServiceComponent::new(
                    "test_service",
                    RuntimeComponentKind::Core,
                    async {},
                    CancellationToken::new(),
                    || async {},
                ))
                .run()
                .await
                .unwrap();
            assert!(metadata_seen.load(std::sync::atomic::Ordering::SeqCst));
            assert!(
                observed
                    .lock()
                    .unwrap()
                    .iter()
                    .all(CancellationToken::is_cancelled)
            );
        }
    }
}

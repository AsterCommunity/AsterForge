//! Background task runtime component integration.
//!
//! Keep product task kind, payload, result, lane policy, and execution bodies in this module tree.
//! Forge owns the handle collection and shutdown mechanics.

/// Creates the background task component used by the product entrypoint.
pub fn background_tasks_component() -> aster_forge_tasks::BackgroundTaskRuntimeComponentFromShutdown<
    impl FnOnce(tokio_util::sync::CancellationToken) -> aster_forge_tasks::BackgroundTasks,
> {
    aster_forge_tasks::background_task_component_from_shutdown(|_shutdown_token| {
        // Register spawned workers here. Use the shutdown token for long-running loops.
        aster_forge_tasks::BackgroundTasks::new()
    })
}

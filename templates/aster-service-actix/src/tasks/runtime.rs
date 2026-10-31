//! Background task runtime component integration.
//!
//! Keep product task kind, payload, result, lane policy, and execution bodies in this module tree.
//! Forge owns the handle collection and shutdown mechanics.

/// Creates the background task component used by the product entrypoint.
pub fn background_tasks_component(
    metrics: aster_forge_metrics::SharedMetricsRecorder,
) -> aster_forge_tasks::BackgroundTaskRuntimeComponentFromShutdown<
    impl FnOnce(tokio_util::sync::CancellationToken) -> aster_forge_tasks::BackgroundTasks,
> {
    aster_forge_tasks::background_task_component_from_shutdown(move |shutdown_token| {
        // Register process-owned loops here. Durable TaskConsumer runners belong outside
        // LeasedScheduledRuntimeConfig; only singleton scheduling belongs in that group.
        // See docs/crates/aster_forge_tasks.md for atomic claim and cancellation contracts.
        let mut tasks = aster_forge_tasks::BackgroundTasks::with_shutdown_token(shutdown_token);
        if let Some(task) = metrics.system_metrics_updater_task(tasks.shutdown_token()) {
            tasks.push(task);
        }
        tasks
    })
}

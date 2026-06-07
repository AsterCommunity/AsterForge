//! Background task spec registry.

use crate::config::RuntimeConfig;
use crate::entities::background_task;
use crate::errors::{AsterError, Result};
use crate::runtime::AppState;
use crate::types::{BackgroundTaskKind, BackgroundTaskStatus};

use super::retry::TaskRetryClass;
use super::spec::{
    ErasedBackgroundTaskSpec, SystemRuntimeTask, TaskProcessFuture, TaskSpecAdapter,
};
use super::steps::initial_task_steps_from_specs;
use super::types::{TaskPayload, TaskResult, TaskStepInfo};
use super::{TaskExecutionContext, dispatch::TaskLane};

static SYSTEM_RUNTIME: TaskSpecAdapter<SystemRuntimeTask> = TaskSpecAdapter::new();

pub(super) fn spec_for_kind(kind: BackgroundTaskKind) -> &'static dyn ErasedBackgroundTaskSpec {
    match kind {
        BackgroundTaskKind::SystemRuntime => &SYSTEM_RUNTIME,
    }
}

pub(super) fn decode_task_payload(task: &background_task::Model) -> Result<TaskPayload> {
    spec_for_kind(task.kind).decode_payload(task)
}

pub(super) fn decode_task_result(task: &background_task::Model) -> Result<Option<TaskResult>> {
    spec_for_kind(task.kind).decode_result(task)
}

pub(super) fn task_retry_class(kind: BackgroundTaskKind, error: &AsterError) -> TaskRetryClass {
    spec_for_kind(kind).retry_class(error)
}

pub(super) fn process_task<'a>(
    state: &'a AppState,
    task: &'a background_task::Model,
    context: TaskExecutionContext,
) -> TaskProcessFuture<'a> {
    spec_for_kind(task.kind).process(state, task, context)
}

pub(super) fn initial_task_steps(kind: BackgroundTaskKind) -> Vec<TaskStepInfo> {
    initial_task_steps_from_specs(spec_for_kind(kind).step_specs())
}

pub(super) fn max_attempts(runtime_config: &RuntimeConfig, kind: BackgroundTaskKind) -> i32 {
    spec_for_kind(kind).max_attempts(runtime_config)
}

pub(in crate::services::task_service) fn task_lane(kind: BackgroundTaskKind) -> TaskLane {
    spec_for_kind(kind).lane()
}

pub(in crate::services::task_service) fn task_lane_kinds(
    lane: TaskLane,
) -> &'static [BackgroundTaskKind] {
    match lane {
        TaskLane::Fallback => &[BackgroundTaskKind::SystemRuntime],
    }
}

pub(super) fn _assert_status_is_imported(_status: BackgroundTaskStatus) {}

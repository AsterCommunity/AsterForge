//! Strongly typed background task specifications.

use std::future::Future;
use std::pin::Pin;

use sea_orm::ActiveEnum;
use serde::{Serialize, de::DeserializeOwned};

use crate::config::{RuntimeConfig, operations};
use crate::entities::background_task;
use crate::errors::{AsterError, Result};
use crate::runtime::AppState;
use crate::types::BackgroundTaskKind;

use super::TaskExecutionContext;
use super::dispatch::TaskLane;
use super::retry::{TaskRetryClass, default_retry_class};
use super::steps::TaskStepSpec;
use super::types::{TaskPayload, TaskResult};

pub(super) type TaskProcessFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

pub(super) trait BackgroundTaskSpec {
    type Payload: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    type Result: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;

    const KIND: BackgroundTaskKind;

    fn step_specs() -> &'static [TaskStepSpec];

    fn lane() -> TaskLane;

    fn max_attempts(runtime_config: &RuntimeConfig) -> i32 {
        operations::background_task_max_attempts(runtime_config)
    }

    fn wrap_payload(payload: Self::Payload) -> TaskPayload;

    fn wrap_result(result: Self::Result) -> TaskResult;

    fn process<'a>(
        state: &'a AppState,
        task: &'a background_task::Model,
        context: TaskExecutionContext,
    ) -> TaskProcessFuture<'a>;

    fn retry_class(error: &AsterError) -> TaskRetryClass {
        default_retry_class(error)
    }
}

pub(super) fn serialize_payload<S: BackgroundTaskSpec>(
    payload: &S::Payload,
) -> Result<crate::types::StoredTaskPayload> {
    serde_json::to_string(payload)
        .map(crate::types::StoredTaskPayload)
        .map_err(|error| {
            AsterError::internal_error(format!(
                "serialize {} task payload: {error}",
                S::KIND.to_value()
            ))
        })
}

pub(super) fn serialize_result<S: BackgroundTaskSpec>(
    result: &S::Result,
) -> Result<crate::types::StoredTaskResult> {
    serde_json::to_string(result)
        .map(crate::types::StoredTaskResult)
        .map_err(|error| {
            AsterError::internal_error(format!(
                "serialize {} task result: {error}",
                S::KIND.to_value()
            ))
        })
}

pub(super) fn decode_payload_as<S: BackgroundTaskSpec>(
    task: &background_task::Model,
) -> Result<S::Payload> {
    if task.kind != S::KIND {
        return Err(AsterError::internal_error(format!(
            "task #{} kind mismatch: expected {}, got {}",
            task.id,
            S::KIND.to_value(),
            task.kind.to_value()
        )));
    }

    serde_json::from_str(task.payload_json.as_ref()).map_err(|error| {
        AsterError::internal_error(format!(
            "parse payload for task #{} ({}): {error}",
            task.id,
            task.kind.to_value()
        ))
    })
}

pub(super) fn decode_result_as<S: BackgroundTaskSpec>(
    task: &background_task::Model,
) -> Result<Option<S::Result>> {
    if task.kind != S::KIND {
        return Err(AsterError::internal_error(format!(
            "task #{} kind mismatch: expected {}, got {}",
            task.id,
            S::KIND.to_value(),
            task.kind.to_value()
        )));
    }

    let Some(raw) = task.result_json.as_ref() else {
        return Ok(None);
    };

    serde_json::from_str(raw.as_ref())
        .map(Some)
        .map_err(|error| {
            AsterError::internal_error(format!(
                "parse result for task #{} ({}): {error}",
                task.id,
                task.kind.to_value()
            ))
        })
}

pub(super) trait ErasedBackgroundTaskSpec: Sync {
    fn step_specs(&self) -> &'static [TaskStepSpec];

    fn lane(&self) -> TaskLane;

    fn max_attempts(&self, runtime_config: &RuntimeConfig) -> i32;

    fn decode_payload(&self, task: &background_task::Model) -> Result<TaskPayload>;

    fn decode_result(&self, task: &background_task::Model) -> Result<Option<TaskResult>>;

    fn retry_class(&self, error: &AsterError) -> TaskRetryClass;

    fn process<'a>(
        &self,
        state: &'a AppState,
        task: &'a background_task::Model,
        context: TaskExecutionContext,
    ) -> TaskProcessFuture<'a>;
}

pub(super) struct TaskSpecAdapter<S>(std::marker::PhantomData<S>);

impl<S> TaskSpecAdapter<S> {
    pub(super) const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<S> ErasedBackgroundTaskSpec for TaskSpecAdapter<S>
where
    S: BackgroundTaskSpec + Sync,
{
    fn step_specs(&self) -> &'static [TaskStepSpec] {
        S::step_specs()
    }

    fn lane(&self) -> TaskLane {
        S::lane()
    }

    fn max_attempts(&self, runtime_config: &RuntimeConfig) -> i32 {
        S::max_attempts(runtime_config)
    }

    fn decode_payload(&self, task: &background_task::Model) -> Result<TaskPayload> {
        Ok(S::wrap_payload(decode_payload_as::<S>(task)?))
    }

    fn decode_result(&self, task: &background_task::Model) -> Result<Option<TaskResult>> {
        Ok(decode_result_as::<S>(task)?.map(S::wrap_result))
    }

    fn retry_class(&self, error: &AsterError) -> TaskRetryClass {
        S::retry_class(error)
    }

    fn process<'a>(
        &self,
        state: &'a AppState,
        task: &'a background_task::Model,
        context: TaskExecutionContext,
    ) -> TaskProcessFuture<'a> {
        S::process(state, task, context)
    }
}

pub(crate) mod runtime;

pub(crate) use runtime::SystemRuntimeTask;

#[cfg(test)]
mod tests {
    use super::{
        BackgroundTaskSpec, ErasedBackgroundTaskSpec, TaskSpecAdapter, decode_payload_as,
        decode_result_as, serialize_payload, serialize_result,
    };
    use crate::entities::background_task;
    use crate::services::task_service::runtime::SystemRuntimeTaskKind;
    use crate::services::task_service::spec::SystemRuntimeTask;
    use crate::services::task_service::types::{
        RuntimeTaskName, RuntimeTaskPayload, RuntimeTaskResult, TaskPayload, TaskResult,
    };
    use crate::types::{
        BackgroundTaskKind, BackgroundTaskStatus, StoredTaskPayload, StoredTaskResult,
    };
    use chrono::Utc;

    fn task_model(
        kind: BackgroundTaskKind,
        payload_json: StoredTaskPayload,
        result_json: Option<StoredTaskResult>,
    ) -> background_task::Model {
        let now = Utc::now();
        background_task::Model {
            id: 7,
            kind,
            status: BackgroundTaskStatus::Succeeded,
            creator_user_id: None,
            display_name: "Task".to_string(),
            payload_json,
            result_json,
            runtime_json: None,
            steps_json: None,
            progress_current: 1,
            progress_total: 1,
            status_text: None,
            attempt_count: 0,
            max_attempts: 1,
            next_run_at: now,
            processing_token: 0,
            processing_started_at: None,
            last_heartbeat_at: None,
            lease_expires_at: None,
            started_at: Some(now),
            finished_at: Some(now),
            last_error: None,
            failure_can_retry: None,
            expires_at: now,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn serialize_and_decode_system_runtime_payload_and_result() {
        let payload = RuntimeTaskPayload {
            task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::SystemHealthCheck),
        };
        let result = RuntimeTaskResult {
            duration_ms: 12,
            summary: Some("ok".to_string()),
            system_health: None,
        };
        let payload_json = serialize_payload::<SystemRuntimeTask>(&payload).unwrap();
        let result_json = serialize_result::<SystemRuntimeTask>(&result).unwrap();
        let task = task_model(
            BackgroundTaskKind::SystemRuntime,
            payload_json,
            Some(result_json),
        );

        assert_eq!(
            decode_payload_as::<SystemRuntimeTask>(&task).unwrap(),
            payload
        );
        assert_eq!(
            decode_result_as::<SystemRuntimeTask>(&task).unwrap(),
            Some(result)
        );

        let adapter = TaskSpecAdapter::<SystemRuntimeTask>::new();
        assert_eq!(
            adapter.decode_payload(&task).unwrap(),
            TaskPayload::SystemRuntime(payload)
        );
        assert_eq!(
            adapter.decode_result(&task).unwrap(),
            Some(TaskResult::SystemRuntime(RuntimeTaskResult {
                duration_ms: 12,
                summary: Some("ok".to_string()),
                system_health: None,
            }))
        );
    }

    #[test]
    fn decode_helpers_surface_kind_mismatch_and_invalid_json() {
        let bad_payload = task_model(
            BackgroundTaskKind::SystemRuntime,
            StoredTaskPayload("not json".to_string()),
            None,
        );
        let error = decode_payload_as::<SystemRuntimeTask>(&bad_payload).unwrap_err();
        assert!(error.message().contains("parse payload for task #7"));

        let bad_result = task_model(
            BackgroundTaskKind::SystemRuntime,
            serialize_payload::<SystemRuntimeTask>(&RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::TaskCleanup),
            })
            .unwrap(),
            Some(StoredTaskResult("not json".to_string())),
        );
        let error = decode_result_as::<SystemRuntimeTask>(&bad_result).unwrap_err();
        assert!(error.message().contains("parse result for task #7"));
    }

    #[test]
    fn system_runtime_spec_contract_is_never_dispatched_by_regular_workers() {
        let runtime_config = crate::config::RuntimeConfig::new();
        let error = crate::errors::AsterError::database_connection("temporary");

        assert_eq!(SystemRuntimeTask::KIND, BackgroundTaskKind::SystemRuntime);
        assert!(SystemRuntimeTask::step_specs().is_empty());
        assert_eq!(SystemRuntimeTask::max_attempts(&runtime_config), 1);
        assert!(!SystemRuntimeTask::retry_class(&error).can_manual_retry());
    }
}

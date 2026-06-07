//! Stable API presentation metadata for background tasks.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::types::{
    RuntimeSystemHealthResult, RuntimeSystemHealthStatus, TaskPayload, TaskPresentation,
    TaskPresentationCode, TaskPresentationMessage, TaskResult,
};
use crate::types::BackgroundTaskStatus;

pub(super) fn build_task_presentation(
    payload: &TaskPayload,
    result: Option<&TaskResult>,
    status: BackgroundTaskStatus,
    last_error: Option<&str>,
) -> Option<TaskPresentation> {
    let title = title_message(payload);
    let status = status_message(payload, result, status, last_error);

    (title.is_some() || status.is_some()).then_some(TaskPresentation { title, status })
}

fn title_message(payload: &TaskPayload) -> Option<TaskPresentationMessage> {
    match payload {
        TaskPayload::SystemRuntime(payload) => payload.task_name.known().map(|kind| {
            message(
                kind.presentation_code(),
                BTreeMap::<String, serde_json::Value>::new(),
            )
        }),
    }
}

fn status_message(
    payload: &TaskPayload,
    result: Option<&TaskResult>,
    status: BackgroundTaskStatus,
    last_error: Option<&str>,
) -> Option<TaskPresentationMessage> {
    match (payload, result) {
        (TaskPayload::SystemRuntime(_), Some(TaskResult::SystemRuntime(result))) => result
            .system_health
            .as_ref()
            .map(system_health_status_message),
        _ => fallback_status_message(status, last_error),
    }
}

fn fallback_status_message(
    status: BackgroundTaskStatus,
    last_error: Option<&str>,
) -> Option<TaskPresentationMessage> {
    match status {
        BackgroundTaskStatus::Succeeded => Some(message(
            TaskPresentationCode::StatusTextSucceeded,
            BTreeMap::new(),
        )),
        BackgroundTaskStatus::Failed => {
            let mut params = BTreeMap::new();
            if let Some(error) = last_error.filter(|error| !error.is_empty()) {
                params.insert("error".to_string(), Value::String(error.to_string()));
            }
            Some(message(TaskPresentationCode::StatusTextFailed, params))
        }
        BackgroundTaskStatus::Canceled => Some(message(
            TaskPresentationCode::StatusTextQuiet,
            BTreeMap::new(),
        )),
        BackgroundTaskStatus::Pending
        | BackgroundTaskStatus::Processing
        | BackgroundTaskStatus::Retry => None,
    }
}

fn system_health_status_message(health: &RuntimeSystemHealthResult) -> TaskPresentationMessage {
    if health.status == RuntimeSystemHealthStatus::Healthy {
        return message(
            TaskPresentationCode::StatusTextSystemHealthy,
            BTreeMap::new(),
        );
    }

    let components = health
        .components
        .iter()
        .filter(|component| component.status != RuntimeSystemHealthStatus::Healthy)
        .collect::<Vec<_>>();

    message(
        TaskPresentationCode::RuntimeSystemHealthIssueDetail,
        params([
            ("status", json!(health.status)),
            ("components", json!(components)),
        ]),
    )
}

fn message(code: TaskPresentationCode, params: BTreeMap<String, Value>) -> TaskPresentationMessage {
    TaskPresentationMessage { code, params }
}

fn params<const N: usize>(entries: [(&str, Value); N]) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::task_service::SystemRuntimeTaskKind;
    use crate::services::task_service::types::{
        RuntimeSystemHealthComponent, RuntimeTaskName, RuntimeTaskPayload, RuntimeTaskResult,
    };

    #[test]
    fn system_runtime_task_gets_stable_title_code() {
        let presentation = build_task_presentation(
            &TaskPayload::SystemRuntime(RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::AuthSessionCleanup),
            }),
            None,
            BackgroundTaskStatus::Pending,
            None,
        )
        .expect("presentation should be built");

        assert_eq!(
            presentation.title.as_ref().unwrap().code,
            TaskPresentationCode::RuntimeTaskAuthSessionCleanup
        );
        assert!(presentation.status.is_none());
    }

    #[test]
    fn healthy_system_runtime_result_gets_status_code() {
        let result = TaskResult::SystemRuntime(RuntimeTaskResult {
            duration_ms: 10,
            summary: Some("ok".to_string()),
            system_health: Some(RuntimeSystemHealthResult {
                status: RuntimeSystemHealthStatus::Healthy,
                components: vec![],
            }),
        });

        let presentation = build_task_presentation(
            &TaskPayload::SystemRuntime(RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::SystemHealthCheck),
            }),
            Some(&result),
            BackgroundTaskStatus::Succeeded,
            None,
        )
        .expect("presentation should be built");

        assert_eq!(
            presentation.status.as_ref().unwrap().code,
            TaskPresentationCode::StatusTextSystemHealthy
        );
    }

    #[test]
    fn degraded_system_runtime_result_lists_unhealthy_components() {
        let result = TaskResult::SystemRuntime(RuntimeTaskResult {
            duration_ms: 10,
            summary: Some("degraded".to_string()),
            system_health: Some(RuntimeSystemHealthResult {
                status: RuntimeSystemHealthStatus::Degraded,
                components: vec![
                    RuntimeSystemHealthComponent {
                        name: "database".to_string(),
                        status: RuntimeSystemHealthStatus::Healthy,
                        message: "ok".to_string(),
                    },
                    RuntimeSystemHealthComponent {
                        name: "cache".to_string(),
                        status: RuntimeSystemHealthStatus::Unhealthy,
                        message: "down".to_string(),
                    },
                ],
            }),
        });

        let presentation = build_task_presentation(
            &TaskPayload::SystemRuntime(RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::SystemHealthCheck),
            }),
            Some(&result),
            BackgroundTaskStatus::Failed,
            Some("health check failed"),
        )
        .expect("presentation should be built");

        let status = presentation.status.as_ref().unwrap();
        assert_eq!(
            status.code,
            TaskPresentationCode::RuntimeSystemHealthIssueDetail
        );
        assert_eq!(status.params["status"], "degraded");
        assert_eq!(
            status.params["components"]
                .as_array()
                .expect("components should be an array")
                .len(),
            1
        );
    }

    #[test]
    fn failed_runtime_task_gets_stable_status_code_with_error() {
        let presentation = build_task_presentation(
            &TaskPayload::SystemRuntime(RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::TaskCleanup),
            }),
            None,
            BackgroundTaskStatus::Failed,
            Some("cleanup failed"),
        )
        .expect("presentation should be built");

        let status = presentation.status.as_ref().unwrap();
        assert_eq!(status.code, TaskPresentationCode::StatusTextFailed);
        assert_eq!(status.params["error"], "cleanup failed");
    }
}

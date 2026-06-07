//! Background task DTOs and stored payload shapes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

use crate::types::{BackgroundTaskKind, BackgroundTaskStatus};

use super::runtime::SystemRuntimeTaskKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TaskStepStatus {
    Pending,
    Active,
    Succeeded,
    Failed,
    Skipped,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct TaskStepInfo {
    pub key: String,
    pub title: String,
    pub status: TaskStepStatus,
    pub progress_current: i64,
    pub progress_total: i64,
    pub detail: Option<String>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = Option<String>))]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = Option<String>))]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TaskPresentationCode {
    RuntimeSystemHealthIssueDetail,
    RuntimeTaskAuditCleanup,
    RuntimeTaskAuthSessionCleanup,
    RuntimeTaskBackgroundTaskDispatch,
    RuntimeTaskExternalAuthFlowCleanup,
    RuntimeTaskSystemHealthCheck,
    RuntimeTaskTaskCleanup,
    StatusTextFailed,
    StatusTextQuiet,
    StatusTextSucceeded,
    StatusTextSystemHealthy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct TaskPresentationMessage {
    pub code: TaskPresentationCode,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct TaskPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<TaskPresentationMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskPresentationMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct TaskCreatorSummary {
    pub id: i64,
    pub username: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTaskName {
    Known(SystemRuntimeTaskKind),
    Legacy(String),
}

impl RuntimeTaskName {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Known(kind) => kind.as_str(),
            Self::Legacy(value) => value.as_str(),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Known(kind) => kind.display_name().to_string(),
            Self::Legacy(value) => value.replace('-', " "),
        }
    }

    pub fn known(&self) -> Option<SystemRuntimeTaskKind> {
        match self {
            Self::Known(kind) => Some(*kind),
            Self::Legacy(_) => None,
        }
    }
}

impl From<SystemRuntimeTaskKind> for RuntimeTaskName {
    fn from(value: SystemRuntimeTaskKind) -> Self {
        Self::Known(value)
    }
}

impl From<String> for RuntimeTaskName {
    fn from(value: String) -> Self {
        SystemRuntimeTaskKind::from_wire_value(&value)
            .map(Self::Known)
            .unwrap_or(Self::Legacy(value))
    }
}

impl From<&str> for RuntimeTaskName {
    fn from(value: &str) -> Self {
        Self::from(value.to_string())
    }
}

impl std::fmt::Display for RuntimeTaskName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for RuntimeTaskName {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RuntimeTaskName {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct RuntimeTaskPayload {
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub task_name: RuntimeTaskName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSystemHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct RuntimeSystemHealthComponent {
    pub name: String,
    pub status: RuntimeSystemHealthStatus,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct RuntimeSystemHealthResult {
    pub status: RuntimeSystemHealthStatus,
    pub components: Vec<RuntimeSystemHealthComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct RuntimeTaskResult {
    pub duration_ms: i64,
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_health: Option<RuntimeSystemHealthResult>,
}

impl RuntimeTaskResult {
    pub fn from_timestamps(
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: chrono::DateTime<chrono::Utc>,
        summary: Option<String>,
        system_health: Option<RuntimeSystemHealthResult>,
    ) -> Self {
        Self {
            duration_ms: (finished_at - started_at).num_milliseconds().max(0),
            summary,
            system_health,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskPayload {
    SystemRuntime(RuntimeTaskPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskResult {
    SystemRuntime(RuntimeTaskResult),
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct TaskInfo {
    pub id: i64,
    pub kind: BackgroundTaskKind,
    pub status: BackgroundTaskStatus,
    pub display_name: String,
    pub creator_user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<TaskCreatorSummary>,
    pub progress_current: i64,
    pub progress_total: i64,
    pub progress_percent: i32,
    pub status_text: Option<String>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub payload: TaskPayload,
    pub result: Option<TaskResult>,
    pub steps: Vec<TaskStepInfo>,
    pub can_retry: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation: Option<TaskPresentation>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = Option<String>))]
    pub lease_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = Option<String>))]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = Option<String>))]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub expires_at: chrono::DateTime<chrono::Utc>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::{
        RuntimeSystemHealthComponent, RuntimeSystemHealthResult, RuntimeSystemHealthStatus,
        RuntimeTaskName, RuntimeTaskPayload, RuntimeTaskResult, TaskPayload, TaskPresentationCode,
        TaskPresentationMessage, TaskResult,
    };
    use crate::services::task_service::runtime::SystemRuntimeTaskKind;
    use chrono::{Duration, Utc};
    use std::collections::BTreeMap;

    #[test]
    fn runtime_task_name_round_trips_known_and_legacy_wire_values() {
        let known = RuntimeTaskName::from("system-health-check");
        assert_eq!(
            known.known(),
            Some(SystemRuntimeTaskKind::SystemHealthCheck)
        );
        assert_eq!(known.as_str(), "system-health-check");
        assert_eq!(known.display_name(), "System health check");
        assert_eq!(
            serde_json::to_string(&known).unwrap(),
            r#""system-health-check""#
        );

        let legacy = RuntimeTaskName::from("legacy-cleanup-job");
        assert_eq!(legacy.known(), None);
        assert_eq!(legacy.as_str(), "legacy-cleanup-job");
        assert_eq!(legacy.display_name(), "legacy cleanup job");
        assert_eq!(legacy.to_string(), "legacy-cleanup-job");

        let decoded: RuntimeTaskName = serde_json::from_str(r#""task-cleanup""#).unwrap();
        assert_eq!(decoded.known(), Some(SystemRuntimeTaskKind::TaskCleanup));
    }

    #[test]
    fn runtime_task_result_duration_never_goes_negative() {
        let started_at = Utc::now();
        let finished_at = started_at + Duration::milliseconds(42);
        let result = RuntimeTaskResult::from_timestamps(
            started_at,
            finished_at,
            Some("done".to_string()),
            None,
        );
        assert_eq!(result.duration_ms, 42);
        assert_eq!(result.summary.as_deref(), Some("done"));

        let backwards = RuntimeTaskResult::from_timestamps(finished_at, started_at, None, None);
        assert_eq!(backwards.duration_ms, 0);
    }

    #[test]
    fn task_payload_and_result_use_tagged_wire_shape() {
        let payload = TaskPayload::SystemRuntime(RuntimeTaskPayload {
            task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::AuditCleanup),
        });
        assert_eq!(
            serde_json::to_value(&payload).unwrap(),
            serde_json::json!({
                "kind": "system_runtime",
                "task_name": "audit-cleanup",
            })
        );

        let result = TaskResult::SystemRuntime(RuntimeTaskResult {
            duration_ms: 12,
            summary: Some("ok".to_string()),
            system_health: Some(RuntimeSystemHealthResult {
                status: RuntimeSystemHealthStatus::Degraded,
                components: vec![RuntimeSystemHealthComponent {
                    name: "database".to_string(),
                    status: RuntimeSystemHealthStatus::Healthy,
                    message: "ok".to_string(),
                }],
            }),
        });
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(encoded["kind"], "system_runtime");
        assert_eq!(encoded["duration_ms"], 12);
        assert_eq!(encoded["system_health"]["status"], "degraded");
    }

    #[test]
    fn task_presentation_message_omits_empty_params() {
        let empty = TaskPresentationMessage {
            code: TaskPresentationCode::StatusTextSucceeded,
            params: BTreeMap::new(),
        };
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            serde_json::json!({ "code": "status_text_succeeded" })
        );

        let mut params = BTreeMap::new();
        params.insert("task".to_string(), serde_json::json!("cleanup"));
        let message = TaskPresentationMessage {
            code: TaskPresentationCode::RuntimeTaskTaskCleanup,
            params,
        };
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::json!({
                "code": "runtime_task_task_cleanup",
                "params": { "task": "cleanup" },
            })
        );
    }
}

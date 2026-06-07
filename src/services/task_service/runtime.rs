//! System runtime task records stored in the background task table.
#![allow(dead_code)]

use chrono::{DateTime, Utc};

use crate::db::repository::background_task_repo;
use crate::entities::background_task;
use crate::errors::Result;
use crate::runtime::SharedRuntimeState;
use crate::types::{BackgroundTaskStatus, StoredTaskPayload};

use super::spec::{self, SystemRuntimeTask};
use super::types::{
    RuntimeSystemHealthResult, RuntimeTaskPayload, RuntimeTaskResult, TaskPresentationCode,
};
use super::{
    TypedTaskCreate, insert_typed_task_record, task_expiration_from, truncate_error,
    truncate_status_text,
};

pub(crate) fn system_runtime_payload_json(
    task_name: SystemRuntimeTaskKind,
) -> Result<StoredTaskPayload> {
    spec::serialize_payload::<SystemRuntimeTask>(&RuntimeTaskPayload {
        task_name: task_name.into(),
    })
}

pub(crate) async fn find_latest_system_runtime_by_task_name(
    state: &impl SharedRuntimeState,
    task_name: SystemRuntimeTaskKind,
) -> Result<Option<background_task::Model>> {
    let payload_json = system_runtime_payload_json(task_name)?;
    background_task_repo::find_latest_system_runtime_by_payload(state.reader_db(), &payload_json)
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemRuntimeTaskKind {
    BackgroundTaskDispatch,
    SystemHealthCheck,
    AuthSessionCleanup,
    ExternalAuthFlowCleanup,
    AuditCleanup,
    TaskCleanup,
}

impl SystemRuntimeTaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BackgroundTaskDispatch => "background-task-dispatch",
            Self::SystemHealthCheck => "system-health-check",
            Self::AuthSessionCleanup => "auth-session-cleanup",
            Self::ExternalAuthFlowCleanup => "external-auth-flow-cleanup",
            Self::AuditCleanup => "audit-cleanup",
            Self::TaskCleanup => "task-cleanup",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::BackgroundTaskDispatch => "Background task dispatch",
            Self::SystemHealthCheck => "System health check",
            Self::AuthSessionCleanup => "Auth session cleanup",
            Self::ExternalAuthFlowCleanup => "External auth flow cleanup",
            Self::AuditCleanup => "Audit log cleanup",
            Self::TaskCleanup => "Task artifact cleanup",
        }
    }

    pub const fn presentation_code(self) -> TaskPresentationCode {
        match self {
            Self::BackgroundTaskDispatch => TaskPresentationCode::RuntimeTaskBackgroundTaskDispatch,
            Self::SystemHealthCheck => TaskPresentationCode::RuntimeTaskSystemHealthCheck,
            Self::AuthSessionCleanup => TaskPresentationCode::RuntimeTaskAuthSessionCleanup,
            Self::ExternalAuthFlowCleanup => {
                TaskPresentationCode::RuntimeTaskExternalAuthFlowCleanup
            }
            Self::AuditCleanup => TaskPresentationCode::RuntimeTaskAuditCleanup,
            Self::TaskCleanup => TaskPresentationCode::RuntimeTaskTaskCleanup,
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "background-task-dispatch" => Some(Self::BackgroundTaskDispatch),
            "system-health-check" => Some(Self::SystemHealthCheck),
            "auth-session-cleanup" => Some(Self::AuthSessionCleanup),
            "external-auth-flow-cleanup" => Some(Self::ExternalAuthFlowCleanup),
            "audit-cleanup" => Some(Self::AuditCleanup),
            "task-cleanup" => Some(Self::TaskCleanup),
            _ => None,
        }
    }
}

impl std::fmt::Display for SystemRuntimeTaskKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTaskRunOutcome {
    Quiet,
    Succeeded {
        summary: Option<String>,
        system_health: Option<RuntimeSystemHealthResult>,
    },
    Failed {
        summary: Option<String>,
        error: String,
        system_health: Option<RuntimeSystemHealthResult>,
    },
}

impl RuntimeTaskRunOutcome {
    pub fn quiet() -> Self {
        Self::Quiet
    }

    pub fn succeeded(summary: Option<String>) -> Self {
        Self::Succeeded {
            summary,
            system_health: None,
        }
    }

    pub fn succeeded_with_system_health(
        summary: Option<String>,
        system_health: RuntimeSystemHealthResult,
    ) -> Self {
        Self::Succeeded {
            summary,
            system_health: Some(system_health),
        }
    }

    pub fn failed(summary: Option<String>, error: impl Into<String>) -> Self {
        Self::Failed {
            summary,
            error: error.into(),
            system_health: None,
        }
    }

    pub fn failed_with_system_health(
        summary: Option<String>,
        error: impl Into<String>,
        system_health: RuntimeSystemHealthResult,
    ) -> Self {
        Self::Failed {
            summary,
            error: error.into(),
            system_health: Some(system_health),
        }
    }

    fn should_record(&self) -> bool {
        !matches!(self, Self::Quiet)
    }

    fn status(&self) -> BackgroundTaskStatus {
        match self {
            Self::Quiet | Self::Succeeded { .. } => BackgroundTaskStatus::Succeeded,
            Self::Failed { .. } => BackgroundTaskStatus::Failed,
        }
    }

    fn summary(&self) -> Option<&str> {
        match self {
            Self::Quiet => None,
            Self::Succeeded { summary, .. } | Self::Failed { summary, .. } => summary.as_deref(),
        }
    }

    fn error(&self) -> Option<&str> {
        match self {
            Self::Failed { error, .. } => Some(error.as_str()),
            Self::Quiet | Self::Succeeded { .. } => None,
        }
    }

    fn system_health(&self) -> Option<RuntimeSystemHealthResult> {
        match self {
            Self::Succeeded { system_health, .. } | Self::Failed { system_health, .. } => {
                system_health.clone()
            }
            Self::Quiet => None,
        }
    }
}

pub async fn record_runtime_task_run(
    state: &impl SharedRuntimeState,
    task_name: SystemRuntimeTaskKind,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    outcome: &RuntimeTaskRunOutcome,
) -> Result<Option<background_task::Model>> {
    if !outcome.should_record() {
        return Ok(None);
    }

    let payload = RuntimeTaskPayload {
        task_name: task_name.into(),
    };
    let payload_json = spec::serialize_payload::<SystemRuntimeTask>(&payload)?;
    let summary = outcome.summary().map(truncate_status_text);
    let last_error = outcome.error().map(truncate_error);
    let result = RuntimeTaskResult::from_timestamps(
        started_at,
        finished_at,
        summary.clone(),
        outcome.system_health(),
    );
    let result_json = spec::serialize_result::<SystemRuntimeTask>(&result)?;

    if should_refresh_latest_success(task_name, outcome)
        && let Some(existing) = background_task_repo::find_latest_system_runtime_by_payload(
            state.writer_db(),
            &payload_json,
        )
        .await?
        && existing.status == BackgroundTaskStatus::Succeeded
        && background_task_repo::refresh_system_runtime_success(
            state.writer_db(),
            background_task_repo::SystemRuntimeSuccessRefresh {
                id: existing.id,
                result_json: result_json.as_ref(),
                status_text: summary.as_deref(),
                next_run_at: finished_at,
                started_at,
                finished_at,
                expires_at: task_expiration_from(state, finished_at),
            },
        )
        .await?
    {
        return background_task_repo::find_by_id(state.writer_db(), existing.id)
            .await
            .map(Some);
    }

    let progress_current = if matches!(outcome, RuntimeTaskRunOutcome::Failed { .. }) {
        0
    } else {
        1
    };
    let mut create = TypedTaskCreate::<SystemRuntimeTask>::new(task_name.display_name(), payload)
        .status(outcome.status())
        .without_steps()
        .progress(progress_current, 1)
        .started_at(started_at)
        .finished_at(finished_at)
        .last_error(last_error)
        .failure_can_retry(if matches!(outcome, RuntimeTaskRunOutcome::Failed { .. }) {
            Some(false)
        } else {
            None
        })
        .result(&result)?;
    if let Some(summary) = summary {
        create = create.status_text(summary);
    }

    let task = insert_typed_task_record(state, state.writer_db(), create).await?;
    Ok(Some(task))
}

fn should_refresh_latest_success(
    task_name: SystemRuntimeTaskKind,
    outcome: &RuntimeTaskRunOutcome,
) -> bool {
    task_name == SystemRuntimeTaskKind::SystemHealthCheck
        && matches!(
            outcome,
            RuntimeTaskRunOutcome::Succeeded {
                system_health: Some(RuntimeSystemHealthResult {
                    status: super::types::RuntimeSystemHealthStatus::Healthy,
                    ..
                }),
                ..
            }
        )
}

#[cfg(test)]
mod tests {
    use super::{
        RuntimeTaskRunOutcome, SystemRuntimeTaskKind, find_latest_system_runtime_by_task_name,
        record_runtime_task_run, system_runtime_payload_json,
    };
    use crate::services::task_service::types::{
        RuntimeSystemHealthComponent, RuntimeSystemHealthResult, RuntimeSystemHealthStatus,
    };
    use crate::types::BackgroundTaskStatus;
    use chrono::{Duration, Utc};

    async fn test_state() -> crate::runtime::AppState {
        let db_cfg = crate::config::DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            pool_size: 1,
            retry_count: 0,
        };
        let db = crate::db::connect_with_metrics(&db_cfg, crate::metrics_core::NoopMetrics::arc())
            .await
            .expect("runtime task test database should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("runtime task test migrations should run");
        crate::services::system_config_service::ensure_defaults(&db)
            .await
            .expect("runtime task test defaults should seed");

        let runtime_config = std::sync::Arc::new(crate::config::RuntimeConfig::new());
        runtime_config
            .reload(&db)
            .await
            .expect("runtime task config should reload");
        let config = std::sync::Arc::new(crate::config::Config {
            database: db_cfg,
            cache: crate::config::CacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        });
        let cache = crate::cache::create_cache(&config.cache).await;

        crate::runtime::AppState {
            db_handles: crate::db::DbHandles::single(db),
            config,
            runtime_config,
            cache,
            metrics: crate::metrics_core::NoopMetrics::arc(),
            background_task_dispatch_wakeup:
                crate::runtime::AppState::new_background_task_dispatch_wakeup(),
        }
    }

    fn health(status: RuntimeSystemHealthStatus) -> RuntimeSystemHealthResult {
        RuntimeSystemHealthResult {
            status,
            components: vec![RuntimeSystemHealthComponent {
                name: "database".to_string(),
                status: RuntimeSystemHealthStatus::Healthy,
                message: "ok".to_string(),
            }],
        }
    }

    #[test]
    fn system_runtime_kind_wire_values_display_names_and_codes_are_stable() {
        assert_eq!(
            SystemRuntimeTaskKind::BackgroundTaskDispatch.as_str(),
            "background-task-dispatch"
        );
        assert_eq!(
            SystemRuntimeTaskKind::SystemHealthCheck.display_name(),
            "System health check"
        );
        assert_eq!(
            SystemRuntimeTaskKind::from_wire_value("auth-session-cleanup"),
            Some(SystemRuntimeTaskKind::AuthSessionCleanup)
        );
        assert_eq!(SystemRuntimeTaskKind::from_wire_value("unknown"), None);
        assert_eq!(
            SystemRuntimeTaskKind::TaskCleanup.to_string(),
            "task-cleanup"
        );
        assert_eq!(
            SystemRuntimeTaskKind::AuditCleanup.presentation_code(),
            crate::services::task_service::types::TaskPresentationCode::RuntimeTaskAuditCleanup
        );
    }

    #[test]
    fn runtime_task_run_outcome_constructors_preserve_variants() {
        assert_eq!(RuntimeTaskRunOutcome::quiet(), RuntimeTaskRunOutcome::Quiet);
        assert_eq!(
            RuntimeTaskRunOutcome::succeeded(Some("ok".to_string())),
            RuntimeTaskRunOutcome::Succeeded {
                summary: Some("ok".to_string()),
                system_health: None,
            }
        );
        assert_eq!(
            RuntimeTaskRunOutcome::failed(Some("bad".to_string()), "boom"),
            RuntimeTaskRunOutcome::Failed {
                summary: Some("bad".to_string()),
                error: "boom".to_string(),
                system_health: None,
            }
        );
    }

    #[tokio::test]
    async fn quiet_runtime_task_outcome_does_not_create_record() {
        let state = test_state().await;
        let now = Utc::now();

        let task = record_runtime_task_run(
            &state,
            SystemRuntimeTaskKind::TaskCleanup,
            now - Duration::seconds(1),
            now,
            &RuntimeTaskRunOutcome::quiet(),
        )
        .await
        .unwrap();

        assert!(task.is_none());
        assert!(
            find_latest_system_runtime_by_task_name(&state, SystemRuntimeTaskKind::TaskCleanup)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn succeeded_and_failed_runtime_task_runs_create_terminal_records() {
        let state = test_state().await;
        let started_at = Utc::now() - Duration::seconds(3);
        let finished_at = Utc::now();

        let succeeded = record_runtime_task_run(
            &state,
            SystemRuntimeTaskKind::AuditCleanup,
            started_at,
            finished_at,
            &RuntimeTaskRunOutcome::succeeded(Some("audit cleanup done".to_string())),
        )
        .await
        .unwrap()
        .expect("succeeded runtime task should be recorded");
        assert_eq!(succeeded.status, BackgroundTaskStatus::Succeeded);
        assert_eq!(succeeded.progress_current, 1);
        assert_eq!(succeeded.progress_total, 1);
        assert_eq!(succeeded.status_text.as_deref(), Some("audit cleanup done"));
        assert!(succeeded.failure_can_retry.is_none());
        assert!(succeeded.result_json.is_some());

        let failed = record_runtime_task_run(
            &state,
            SystemRuntimeTaskKind::TaskCleanup,
            started_at,
            finished_at,
            &RuntimeTaskRunOutcome::failed(Some("task cleanup failed".to_string()), "boom"),
        )
        .await
        .unwrap()
        .expect("failed runtime task should be recorded");
        assert_eq!(failed.status, BackgroundTaskStatus::Failed);
        assert_eq!(failed.progress_current, 0);
        assert_eq!(failed.progress_total, 1);
        assert_eq!(failed.last_error.as_deref(), Some("boom"));
        assert_eq!(failed.failure_can_retry, Some(false));
    }

    #[tokio::test]
    async fn healthy_system_health_success_refreshes_latest_success_record() {
        let state = test_state().await;
        let first_start = Utc::now() - Duration::seconds(10);
        let first_finish = Utc::now() - Duration::seconds(9);

        let first = record_runtime_task_run(
            &state,
            SystemRuntimeTaskKind::SystemHealthCheck,
            first_start,
            first_finish,
            &RuntimeTaskRunOutcome::succeeded_with_system_health(
                Some("healthy one".to_string()),
                health(RuntimeSystemHealthStatus::Healthy),
            ),
        )
        .await
        .unwrap()
        .expect("first health check should record");

        let second_start = Utc::now() - Duration::seconds(2);
        let second_finish = Utc::now();
        let second = record_runtime_task_run(
            &state,
            SystemRuntimeTaskKind::SystemHealthCheck,
            second_start,
            second_finish,
            &RuntimeTaskRunOutcome::succeeded_with_system_health(
                Some("healthy two".to_string()),
                health(RuntimeSystemHealthStatus::Healthy),
            ),
        )
        .await
        .unwrap()
        .expect("second health check should refresh existing record");

        assert_eq!(second.id, first.id);
        assert_eq!(second.status_text.as_deref(), Some("healthy two"));
        assert_eq!(second.started_at, Some(second_start));
        assert_eq!(second.finished_at, Some(second_finish));

        let latest = find_latest_system_runtime_by_task_name(
            &state,
            SystemRuntimeTaskKind::SystemHealthCheck,
        )
        .await
        .unwrap()
        .expect("latest health check should exist");
        assert_eq!(latest.id, first.id);
    }

    #[tokio::test]
    async fn unhealthy_system_health_result_creates_new_failed_record() {
        let state = test_state().await;
        let started_at = Utc::now() - Duration::seconds(5);
        let finished_at = Utc::now();

        let failed = record_runtime_task_run(
            &state,
            SystemRuntimeTaskKind::SystemHealthCheck,
            started_at,
            finished_at,
            &RuntimeTaskRunOutcome::failed_with_system_health(
                Some("unhealthy".to_string()),
                "database down",
                health(RuntimeSystemHealthStatus::Unhealthy),
            ),
        )
        .await
        .unwrap()
        .expect("unhealthy health check should record failure");

        assert_eq!(failed.status, BackgroundTaskStatus::Failed);
        assert_eq!(failed.status_text.as_deref(), Some("unhealthy"));
        assert_eq!(failed.last_error.as_deref(), Some("database down"));
    }

    #[test]
    fn system_runtime_payload_json_uses_typed_task_shape() {
        let payload =
            system_runtime_payload_json(SystemRuntimeTaskKind::ExternalAuthFlowCleanup).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(payload.as_ref()).unwrap(),
            serde_json::json!({ "task_name": "external-auth-flow-cleanup" })
        );
    }
}

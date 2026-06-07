//! Persisted background task subsystem.
#![allow(dead_code)]

pub(crate) mod dispatch;
mod presentation;
mod registry;
mod retry;
pub(crate) mod runtime;
mod spec;
mod steps;
pub mod types;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use parking_lot::Mutex;
use sea_orm::{ConnectionTrait, DatabaseConnection, Set};
use tokio_util::sync::CancellationToken;

use crate::api::pagination::{AdminTaskSortBy, OffsetPage, SortOrder};
use crate::config::operations;
use crate::db::repository::{background_task_repo, user_repo};
use crate::entities::{background_task, user};
use crate::errors::{AsterError, Result};
use crate::runtime::{AppState, SharedRuntimeState};
use crate::services::audit_service;
use crate::types::{BackgroundTaskKind, BackgroundTaskStatus, StoredTaskResult, StoredTaskSteps};
use crate::utils::numbers::{i64_to_i32, i64_to_u64};

pub use dispatch::{DispatchStats, cleanup_expired, dispatch_due, drain};
use presentation::build_task_presentation;
use registry::{decode_task_payload, decode_task_result};
pub use runtime::{RuntimeTaskRunOutcome, SystemRuntimeTaskKind, record_runtime_task_run};
use spec::BackgroundTaskSpec;
use steps::{parse_task_steps_json, serialize_task_steps};
use types::{TaskCreatorSummary, TaskInfo, TaskResult, TaskStepInfo};

pub(super) const DEFAULT_TASK_RETENTION_HOURS: i64 = 24;
pub(super) const TASK_HEARTBEAT_INTERVAL_SECS: u64 = 10;
pub(super) const TASK_PROCESSING_STALE_SECS: i64 = 60;
pub(super) const TASK_DISPLAY_NAME_MAX_LEN: usize = 512;
pub(super) const TASK_LAST_ERROR_MAX_LEN: usize = 1024;
pub(super) const TASK_STATUS_TEXT_MAX_LEN: usize = 255;
pub(super) const TASK_DRAIN_MAX_ROUNDS: usize = 32;
const TASK_LEASE_LOST_MESSAGE_PREFIX: &str = "background task lease lost";
const TASK_LEASE_RENEWAL_TIMEOUT_MESSAGE_PREFIX: &str = "background task lease renewal timed out";
const TASK_WORKER_SHUTDOWN_REQUESTED_MESSAGE_PREFIX: &str =
    "background task worker shutdown requested";

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AdminTaskListFilters {
    pub(crate) kind: Option<BackgroundTaskKind>,
    pub(crate) status: Option<BackgroundTaskStatus>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AdminTaskCleanupFilters {
    pub(crate) finished_before: chrono::DateTime<chrono::Utc>,
    pub(crate) kind: Option<BackgroundTaskKind>,
    pub(crate) status: Option<BackgroundTaskStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskLease {
    pub(super) task_id: i64,
    pub(super) processing_token: i64,
}

impl TaskLease {
    pub(super) fn new(task_id: i64, processing_token: i64) -> Self {
        Self {
            task_id,
            processing_token,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TaskLeaseGuard {
    lease: TaskLease,
    renewal_timeout: StdDuration,
    shutdown_token: Option<CancellationToken>,
    state: Arc<Mutex<TaskLeaseGuardState>>,
}

#[derive(Debug)]
struct TaskLeaseGuardState {
    last_renewed_at: Instant,
    termination: Option<TaskLeaseTermination>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskLeaseTermination {
    Lost,
    RenewalTimedOut,
    ShutdownRequested,
}

impl TaskLeaseGuard {
    pub(super) fn new(lease: TaskLease) -> Self {
        Self::with_renewal_timeout(lease, task_lease_renewal_timeout())
    }

    pub(super) fn with_renewal_timeout(lease: TaskLease, renewal_timeout: StdDuration) -> Self {
        Self {
            lease,
            renewal_timeout,
            shutdown_token: None,
            state: Arc::new(Mutex::new(TaskLeaseGuardState {
                last_renewed_at: Instant::now(),
                termination: None,
            })),
        }
    }

    fn with_shutdown_token(lease: TaskLease, shutdown_token: CancellationToken) -> Self {
        Self {
            shutdown_token: Some(shutdown_token),
            ..Self::new(lease)
        }
    }

    pub(super) fn lease(&self) -> TaskLease {
        self.lease
    }

    pub(super) fn record_renewed(&self) {
        let mut state = self.state.lock();
        if state.termination.is_none() {
            state.last_renewed_at = Instant::now();
        }
    }

    pub(super) fn mark_lost(&self) -> AsterError {
        let mut state = self.state.lock();
        state.termination = Some(TaskLeaseTermination::Lost);
        task_lease_lost(self.lease)
    }

    fn mark_shutdown_requested(&self) -> AsterError {
        let mut state = self.state.lock();
        state.termination = Some(TaskLeaseTermination::ShutdownRequested);
        task_worker_shutdown_requested(self.lease)
    }

    pub(super) fn ensure_active(&self) -> Result<()> {
        let mut state = self.state.lock();
        match state.termination {
            Some(TaskLeaseTermination::Lost) => return Err(task_lease_lost(self.lease)),
            Some(TaskLeaseTermination::RenewalTimedOut) => {
                return Err(task_lease_renewal_timed_out(self.lease));
            }
            Some(TaskLeaseTermination::ShutdownRequested) => {
                return Err(task_worker_shutdown_requested(self.lease));
            }
            None => {}
        }
        if self
            .shutdown_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            state.termination = Some(TaskLeaseTermination::ShutdownRequested);
            return Err(task_worker_shutdown_requested(self.lease));
        }
        if state.last_renewed_at.elapsed() >= self.renewal_timeout {
            state.termination = Some(TaskLeaseTermination::RenewalTimedOut);
            return Err(task_lease_renewal_timed_out(self.lease));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct TaskExecutionContext {
    lease_guard: TaskLeaseGuard,
    shutdown_token: CancellationToken,
}

impl TaskExecutionContext {
    pub(super) fn new(lease: TaskLease, shutdown_token: CancellationToken) -> Self {
        Self {
            lease_guard: TaskLeaseGuard::with_shutdown_token(lease, shutdown_token.clone()),
            shutdown_token,
        }
    }

    pub(super) fn lease_guard(&self) -> &TaskLeaseGuard {
        &self.lease_guard
    }

    pub(super) fn ensure_active(&self) -> Result<()> {
        self.lease_guard.ensure_active()
    }

    pub(super) async fn sleep_or_shutdown(&self, duration: StdDuration) -> Result<()> {
        self.lease_guard.ensure_active()?;

        tokio::select! {
            biased;
            _ = self.shutdown_token.cancelled() => Err(self.lease_guard.mark_shutdown_requested()),
            _ = tokio::time::sleep(duration) => Ok(()),
        }
    }

    pub(super) async fn shutdown_requested(&self) -> Result<()> {
        self.shutdown_token.cancelled().await;
        Err(self.lease_guard.mark_shutdown_requested())
    }
}

pub(crate) async fn list_tasks_paginated_for_admin(
    state: &impl SharedRuntimeState,
    limit: u64,
    offset: u64,
    filters: AdminTaskListFilters,
    sort_by: AdminTaskSortBy,
    sort_order: SortOrder,
) -> Result<OffsetPage<TaskInfo>> {
    let limit = limit.clamp(1, operations::task_list_max_limit(state.runtime_config()));
    let (tasks, total) = background_task_repo::find_paginated_all_filtered(
        state.writer_db(),
        limit,
        offset,
        &background_task_repo::AdminTaskFilters {
            kind: filters.kind,
            status: filters.status,
        },
        sort_by,
        sort_order,
    )
    .await?;

    let items = tasks
        .into_iter()
        .map(|task| build_task_info(task, None))
        .collect::<Result<Vec<_>>>()?;
    let items = hydrate_task_creators(state.reader_db(), items).await?;
    Ok(OffsetPage::new(items, total, limit, offset))
}

pub(crate) async fn cleanup_tasks_for_admin(
    state: &impl SharedRuntimeState,
    filters: AdminTaskCleanupFilters,
) -> Result<u64> {
    validate_admin_task_cleanup_status(filters.status)?;
    background_task_repo::delete_terminal_by_filters(
        state.writer_db(),
        &background_task_repo::TerminalTaskCleanupFilters {
            finished_before: filters.finished_before,
            kind: filters.kind,
            status: filters.status,
        },
    )
    .await
}

pub(crate) async fn retry_task_for_admin(state: &AppState, task_id: i64) -> Result<TaskInfo> {
    let task = background_task_repo::find_by_id(state.writer_db(), task_id).await?;
    retry_task_record(state, &task).await?;
    let task = background_task_repo::find_by_id(state.writer_db(), task_id).await?;
    build_task_info_with_creator(state.reader_db(), task).await
}

pub(crate) async fn retry_task_for_admin_with_audit(
    state: &AppState,
    task_id: i64,
    audit_ctx: &audit_service::AuditContext,
) -> Result<TaskInfo> {
    let previous = background_task_repo::find_by_id(state.writer_db(), task_id).await?;
    retry_task_record(state, &previous).await?;
    let task = background_task_repo::find_by_id(state.writer_db(), task_id).await?;
    let task_info = build_task_info_with_creator(state.reader_db(), task).await?;
    audit_service::log_with_details(
        state,
        audit_ctx,
        audit_service::AuditAction::TaskRetry,
        audit_service::AuditEntityType::Task,
        Some(task_info.id),
        Some(&task_info.display_name),
        || {
            audit_service::details(audit_service::TaskRetryAuditDetails {
                kind: previous.kind.to_string(),
                previous_attempt_count: previous.attempt_count,
            })
        },
    )
    .await;
    Ok(task_info)
}

async fn retry_task_record(state: &AppState, task: &background_task::Model) -> Result<()> {
    if task.status != BackgroundTaskStatus::Failed {
        return Err(AsterError::validation_error(
            "only failed tasks can be retried",
        ));
    }
    if !task_can_retry(task) {
        return Err(AsterError::validation_error(
            "this task failure cannot be retried",
        ));
    }

    cleanup_task_temp_dir_for_task(state, task.id).await?;
    let steps_json = serialize_task_steps(&registry::initial_task_steps(task.kind))?;
    let max_attempts = registry::max_attempts(state.runtime_config().as_ref(), task.kind);
    let now = Utc::now();

    if !background_task_repo::reset_for_manual_retry(
        state.writer_db(),
        task.id,
        now,
        max_attempts,
        Some(steps_json.as_ref()),
    )
    .await?
    {
        return Err(AsterError::internal_error(format!(
            "failed to reset task #{} for retry",
            task.id
        )));
    }
    state.wake_background_task_dispatcher();
    Ok(())
}

async fn build_task_info_with_creator(
    db: &DatabaseConnection,
    task: background_task::Model,
) -> Result<TaskInfo> {
    let creator = match task.creator_user_id {
        Some(user_id) => Some(TaskCreatorSummary::from(
            user_repo::find_by_id(db, user_id).await?,
        )),
        None => None,
    };
    build_task_info(task, creator)
}

async fn hydrate_task_creators(
    db: &DatabaseConnection,
    tasks: Vec<TaskInfo>,
) -> Result<Vec<TaskInfo>> {
    let creator_ids = tasks
        .iter()
        .filter_map(|task| task.creator_user_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if creator_ids.is_empty() {
        return Ok(tasks);
    }

    let creators = user_repo::find_by_ids(db, &creator_ids)
        .await?
        .into_iter()
        .map(|user| (user.id, TaskCreatorSummary::from(user)))
        .collect::<HashMap<_, _>>();

    Ok(tasks
        .into_iter()
        .map(|mut task| {
            task.creator = task
                .creator_user_id
                .and_then(|user_id| creators.get(&user_id).cloned());
            task
        })
        .collect())
}

fn build_task_info(
    task: background_task::Model,
    creator: Option<TaskCreatorSummary>,
) -> Result<TaskInfo> {
    let progress_percent = if task.progress_total <= 0 {
        if task.status == BackgroundTaskStatus::Succeeded {
            100
        } else {
            0
        }
    } else {
        i64_to_i32(
            ((task.progress_current.saturating_mul(100)) / task.progress_total).clamp(0, 100),
            "task progress percent",
        )?
    };
    let payload = decode_task_payload(&task)?;
    let result = decode_task_result_or_none(&task);
    let steps = parse_task_steps_json(task.steps_json.as_ref().map(|raw| raw.as_ref()))?;
    let can_retry = task_can_retry(&task);
    let presentation = build_task_presentation(
        &payload,
        result.as_ref(),
        task.status,
        task.last_error.as_deref(),
    );

    Ok(TaskInfo {
        id: task.id,
        kind: task.kind,
        status: task.status,
        display_name: task.display_name,
        creator_user_id: task.creator_user_id,
        creator,
        progress_current: task.progress_current,
        progress_total: task.progress_total,
        progress_percent,
        status_text: task.status_text,
        attempt_count: task.attempt_count,
        max_attempts: task.max_attempts,
        last_error: task.last_error,
        payload,
        result,
        steps,
        can_retry,
        presentation,
        lease_expires_at: task.lease_expires_at,
        started_at: task.started_at,
        finished_at: task.finished_at,
        expires_at: task.expires_at,
        created_at: task.created_at,
        updated_at: task.updated_at,
    })
}

impl From<user::Model> for TaskCreatorSummary {
    fn from(model: user::Model) -> Self {
        Self {
            id: model.id,
            username: model.username,
            email: model.email,
        }
    }
}

fn decode_task_result_or_none(task: &background_task::Model) -> Option<TaskResult> {
    match decode_task_result(task) {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(
                task_id = task.id,
                error = %error,
                "failed to decode background task result; continuing without result"
            );
            None
        }
    }
}

fn task_can_retry(task: &background_task::Model) -> bool {
    task.status == BackgroundTaskStatus::Failed && task.failure_can_retry.unwrap_or(true)
}

pub(in crate::services::task_service) struct TypedTaskCreate<S: BackgroundTaskSpec> {
    display_name: String,
    payload: S::Payload,
    creator_user_id: Option<i64>,
    status: BackgroundTaskStatus,
    result_json: Option<StoredTaskResult>,
    include_steps: bool,
    progress_current: i64,
    progress_total: i64,
    status_text: Option<String>,
    next_run_at: chrono::DateTime<Utc>,
    started_at: Option<chrono::DateTime<Utc>>,
    finished_at: Option<chrono::DateTime<Utc>>,
    last_error: Option<String>,
    failure_can_retry: Option<bool>,
    expires_at_anchor: chrono::DateTime<Utc>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

impl<S: BackgroundTaskSpec> TypedTaskCreate<S> {
    pub(in crate::services::task_service) fn new(
        display_name: impl Into<String>,
        payload: S::Payload,
    ) -> Self {
        let now = Utc::now();
        Self {
            display_name: display_name.into(),
            payload,
            creator_user_id: None,
            status: BackgroundTaskStatus::Pending,
            result_json: None,
            include_steps: true,
            progress_current: 0,
            progress_total: 0,
            status_text: None,
            next_run_at: now,
            started_at: None,
            finished_at: None,
            last_error: None,
            failure_can_retry: None,
            expires_at_anchor: now,
            created_at: now,
            updated_at: now,
        }
    }

    pub(in crate::services::task_service) fn creator_user_id(
        mut self,
        creator_user_id: Option<i64>,
    ) -> Self {
        self.creator_user_id = creator_user_id;
        self
    }

    pub(in crate::services::task_service) fn next_run_at(
        mut self,
        next_run_at: chrono::DateTime<Utc>,
    ) -> Self {
        self.next_run_at = next_run_at;
        self.expires_at_anchor = next_run_at;
        self
    }

    pub(in crate::services::task_service) fn progress(mut self, current: i64, total: i64) -> Self {
        self.progress_current = current;
        self.progress_total = total;
        self
    }

    pub(in crate::services::task_service) fn status_text(mut self, status_text: String) -> Self {
        self.status_text = Some(status_text);
        self
    }

    pub(in crate::services::task_service) fn status(
        mut self,
        status: BackgroundTaskStatus,
    ) -> Self {
        self.status = status;
        self
    }

    pub(in crate::services::task_service) fn result(mut self, result: &S::Result) -> Result<Self> {
        self.result_json = Some(spec::serialize_result::<S>(result)?);
        Ok(self)
    }

    pub(in crate::services::task_service) fn without_steps(mut self) -> Self {
        self.include_steps = false;
        self
    }

    pub(in crate::services::task_service) fn started_at(
        mut self,
        started_at: chrono::DateTime<Utc>,
    ) -> Self {
        self.started_at = Some(started_at);
        self.created_at = started_at;
        self
    }

    pub(in crate::services::task_service) fn finished_at(
        mut self,
        finished_at: chrono::DateTime<Utc>,
    ) -> Self {
        self.finished_at = Some(finished_at);
        self.next_run_at = finished_at;
        self.expires_at_anchor = finished_at;
        self.updated_at = finished_at;
        self
    }

    pub(in crate::services::task_service) fn last_error(
        mut self,
        last_error: Option<String>,
    ) -> Self {
        self.last_error = last_error;
        self
    }

    pub(in crate::services::task_service) fn failure_can_retry(
        mut self,
        failure_can_retry: Option<bool>,
    ) -> Self {
        self.failure_can_retry = failure_can_retry;
        self
    }

    fn steps_json(&self) -> Result<Option<StoredTaskSteps>> {
        if self.include_steps {
            serialize_task_steps(&registry::initial_task_steps(S::KIND)).map(Some)
        } else {
            Ok(None)
        }
    }

    fn into_active_model(
        self,
        state: &impl SharedRuntimeState,
    ) -> Result<background_task::ActiveModel> {
        let payload_json = spec::serialize_payload::<S>(&self.payload)?;
        let steps_json = self.steps_json()?;

        Ok(background_task::ActiveModel {
            kind: Set(S::KIND),
            status: Set(self.status),
            creator_user_id: Set(self.creator_user_id),
            display_name: Set(truncate_display_name(&self.display_name)),
            payload_json: Set(payload_json),
            result_json: Set(self.result_json),
            runtime_json: Set(None),
            steps_json: Set(steps_json),
            progress_current: Set(self.progress_current),
            progress_total: Set(self.progress_total),
            status_text: Set(self.status_text.as_deref().map(truncate_status_text)),
            attempt_count: Set(0),
            max_attempts: Set(registry::max_attempts(
                state.runtime_config().as_ref(),
                S::KIND,
            )),
            next_run_at: Set(self.next_run_at),
            processing_token: Set(0),
            processing_started_at: Set(None),
            last_heartbeat_at: Set(None),
            lease_expires_at: Set(None),
            started_at: Set(self.started_at),
            finished_at: Set(self.finished_at),
            last_error: Set(self.last_error.as_deref().map(truncate_error)),
            failure_can_retry: Set(self.failure_can_retry),
            expires_at: Set(task_expiration_from(state, self.expires_at_anchor)),
            created_at: Set(self.created_at),
            updated_at: Set(self.updated_at),
            ..Default::default()
        })
    }
}

pub(in crate::services::task_service) async fn insert_typed_task_record<
    C: ConnectionTrait,
    S: BackgroundTaskSpec,
>(
    state: &impl SharedRuntimeState,
    db: &C,
    request: TypedTaskCreate<S>,
) -> Result<background_task::Model> {
    background_task_repo::create(db, request.into_active_model(state)?).await
}

pub(in crate::services::task_service) async fn create_typed_task_record<S: BackgroundTaskSpec>(
    state: &AppState,
    display_name: &str,
    payload: &S::Payload,
    creator_user_id: Option<i64>,
) -> Result<background_task::Model> {
    let task = insert_typed_task_record(
        state,
        state.writer_db(),
        TypedTaskCreate::<S>::new(display_name, payload.clone()).creator_user_id(creator_user_id),
    )
    .await?;
    state.wake_background_task_dispatcher();
    Ok(task)
}

pub(super) async fn mark_task_progress(
    state: &impl SharedRuntimeState,
    lease_guard: &TaskLeaseGuard,
    current: i64,
    total: i64,
    status_text: Option<&str>,
    steps: &[TaskStepInfo],
) -> Result<()> {
    update_task_progress_db(
        state.writer_db(),
        lease_guard,
        current,
        total,
        status_text,
        steps,
    )
    .await
}

pub(super) async fn update_task_progress_db(
    db: &DatabaseConnection,
    lease_guard: &TaskLeaseGuard,
    current: i64,
    total: i64,
    status_text: Option<&str>,
    steps: &[TaskStepInfo],
) -> Result<()> {
    let status_text = status_text.map(truncate_status_text);
    let steps_json = serialize_task_steps(steps)?;
    let lease = lease_guard.lease();
    let now = Utc::now();
    if background_task_repo::mark_progress(
        db,
        background_task_repo::TaskProgressUpdate {
            id: lease.task_id,
            processing_token: lease.processing_token,
            now,
            lease_expires_at: task_lease_expires_at(now),
            current,
            total,
            status_text: status_text.as_deref(),
            steps_json: Some(steps_json.as_ref()),
        },
    )
    .await?
    {
        lease_guard.record_renewed();
        Ok(())
    } else {
        Err(lease_guard.mark_lost())
    }
}

pub(super) async fn set_task_runtime_json(
    state: &impl SharedRuntimeState,
    lease_guard: &TaskLeaseGuard,
    runtime_json: Option<&str>,
) -> Result<()> {
    let lease = lease_guard.lease();
    let now = Utc::now();
    if background_task_repo::set_runtime_json(
        state.writer_db(),
        lease.task_id,
        lease.processing_token,
        runtime_json,
        now,
    )
    .await?
    {
        lease_guard.record_renewed();
        Ok(())
    } else {
        Err(lease_guard.mark_lost())
    }
}

pub(super) async fn set_task_display_name(
    state: &impl SharedRuntimeState,
    lease_guard: &TaskLeaseGuard,
    display_name: &str,
) -> Result<()> {
    let lease = lease_guard.lease();
    let now = Utc::now();
    let display_name = truncate_display_name(display_name);
    if background_task_repo::set_display_name(
        state.writer_db(),
        lease.task_id,
        lease.processing_token,
        &display_name,
        now,
    )
    .await?
    {
        lease_guard.record_renewed();
        Ok(())
    } else {
        Err(lease_guard.mark_lost())
    }
}

pub(super) async fn mark_task_succeeded(
    state: &impl SharedRuntimeState,
    lease_guard: &TaskLeaseGuard,
    result_json: Option<&StoredTaskResult>,
    current: i64,
    total: i64,
    status_text: Option<&str>,
    steps: &[TaskStepInfo],
) -> Result<()> {
    let now = Utc::now();
    let status_text = status_text.map(truncate_status_text);
    let steps_json = serialize_task_steps(steps)?;
    let lease = lease_guard.lease();
    if background_task_repo::mark_succeeded(
        state.writer_db(),
        background_task_repo::TaskSuccessUpdate {
            id: lease.task_id,
            processing_token: lease.processing_token,
            result_json: result_json.map(AsRef::as_ref),
            steps_json: Some(steps_json.as_ref()),
            current,
            total,
            status_text: status_text.as_deref(),
            finished_at: now,
            expires_at: task_expiration_from(state, now),
        },
    )
    .await?
    {
        lease_guard.record_renewed();
        Ok(())
    } else {
        Err(lease_guard.mark_lost())
    }
}

pub(super) async fn prepare_task_temp_dir(
    state: &impl SharedRuntimeState,
    lease: TaskLease,
) -> Result<String> {
    prepare_task_temp_dir_in_root(&state.config().server.temp_dir, lease).await
}

pub(super) async fn prepare_task_temp_dir_in_root(
    temp_root: &str,
    lease: TaskLease,
) -> Result<String> {
    cleanup_task_temp_dir_for_lease_in_root(temp_root, lease).await?;
    let task_temp_dir =
        crate::utils::paths::task_token_temp_dir(temp_root, lease.task_id, lease.processing_token);
    tokio::fs::create_dir_all(&task_temp_dir)
        .await
        .map_err(|error| AsterError::internal_error(format!("create task temp dir: {error}")))?;
    Ok(task_temp_dir)
}

pub(super) async fn cleanup_task_temp_dir_for_lease_in_root(
    temp_root: &str,
    lease: TaskLease,
) -> Result<()> {
    crate::utils::cleanup_temp_dir(&crate::utils::paths::task_token_temp_dir(
        temp_root,
        lease.task_id,
        lease.processing_token,
    ))
    .await;
    Ok(())
}

pub(super) async fn cleanup_task_temp_dir_for_task(
    state: &impl SharedRuntimeState,
    task_id: i64,
) -> Result<()> {
    cleanup_task_temp_dir_for_task_in_root(&state.config().server.temp_dir, task_id).await
}

pub(super) async fn cleanup_task_temp_dir_for_task_in_root(
    temp_root: &str,
    task_id: i64,
) -> Result<()> {
    crate::utils::cleanup_temp_dir(&crate::utils::paths::task_temp_dir(temp_root, task_id)).await;
    Ok(())
}

pub(super) fn task_expiration_from(
    state: &impl SharedRuntimeState,
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    now + Duration::hours(load_task_retention_hours(state))
}

pub(super) fn task_lease_expires_at(
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    now + Duration::seconds(TASK_PROCESSING_STALE_SECS.max(1))
}

fn validate_admin_task_cleanup_status(status: Option<BackgroundTaskStatus>) -> Result<()> {
    if status.is_some_and(|value| !value.is_terminal()) {
        return Err(AsterError::validation_error(
            "only completed task statuses can be cleaned up",
        ));
    }
    Ok(())
}

fn load_task_retention_hours(state: &impl SharedRuntimeState) -> i64 {
    let Some(raw) = state
        .runtime_config()
        .get(operations::TASK_RETENTION_HOURS_KEY)
    else {
        return DEFAULT_TASK_RETENTION_HOURS;
    };
    match raw.parse::<i64>() {
        Ok(hours) if hours > 0 => hours,
        _ => {
            tracing::warn!(
                "invalid task_retention_hours value '{}', using default",
                raw
            );
            DEFAULT_TASK_RETENTION_HOURS
        }
    }
}

pub(super) fn task_lease_lost(lease: TaskLease) -> AsterError {
    AsterError::internal_error(format!(
        "{TASK_LEASE_LOST_MESSAGE_PREFIX} for task #{} with token {}",
        lease.task_id, lease.processing_token
    ))
}

pub(super) fn task_lease_renewal_timed_out(lease: TaskLease) -> AsterError {
    AsterError::internal_error(format!(
        "{TASK_LEASE_RENEWAL_TIMEOUT_MESSAGE_PREFIX} for task #{} with token {}",
        lease.task_id, lease.processing_token
    ))
}

pub(super) fn task_worker_shutdown_requested(lease: TaskLease) -> AsterError {
    AsterError::internal_error(format!(
        "{TASK_WORKER_SHUTDOWN_REQUESTED_MESSAGE_PREFIX} for task #{} with token {}",
        lease.task_id, lease.processing_token
    ))
}

pub(super) fn is_task_lease_lost(error: &AsterError) -> bool {
    error.message().starts_with(TASK_LEASE_LOST_MESSAGE_PREFIX)
}

pub(super) fn is_task_lease_renewal_timed_out(error: &AsterError) -> bool {
    error
        .message()
        .starts_with(TASK_LEASE_RENEWAL_TIMEOUT_MESSAGE_PREFIX)
}

pub(super) fn is_task_worker_shutdown_requested(error: &AsterError) -> bool {
    error
        .message()
        .starts_with(TASK_WORKER_SHUTDOWN_REQUESTED_MESSAGE_PREFIX)
}

fn task_lease_renewal_timeout() -> StdDuration {
    let stale_secs = i64_to_u64(
        TASK_PROCESSING_STALE_SECS.max(1),
        "task processing stale seconds",
    )
    .unwrap_or(u64::MAX);
    let heartbeat_secs = TASK_HEARTBEAT_INTERVAL_SECS.max(1);
    StdDuration::from_secs(stale_secs.saturating_sub(heartbeat_secs).max(1))
}

pub(super) fn truncate_display_name(value: &str) -> String {
    crate::utils::truncate_utf8_to_max_bytes(value, TASK_DISPLAY_NAME_MAX_LEN)
}

pub(super) fn truncate_status_text(value: &str) -> String {
    value.chars().take(TASK_STATUS_TEXT_MAX_LEN).collect()
}

pub(super) fn truncate_error(error: &str) -> String {
    error.chars().take(TASK_LAST_ERROR_MAX_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::task_service::runtime::SystemRuntimeTaskKind;
    use crate::services::task_service::spec::SystemRuntimeTask;
    use crate::services::task_service::types::{
        RuntimeTaskName, RuntimeTaskPayload, RuntimeTaskResult, TaskStepStatus,
    };
    use crate::types::{StoredTaskPayload, SystemConfigSource, SystemConfigVisibility};
    use sea_orm::{ActiveModelTrait, Set};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    async fn test_state() -> AppState {
        let db_cfg = crate::config::DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            pool_size: 1,
            retry_count: 0,
        };
        let db = crate::db::connect_with_metrics(&db_cfg, crate::metrics_core::NoopMetrics::arc())
            .await
            .expect("task service test database should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("task service test migrations should run");
        crate::services::system_config_service::ensure_defaults(&db)
            .await
            .expect("task service test defaults should seed");

        let runtime_config = Arc::new(crate::config::RuntimeConfig::new());
        runtime_config
            .reload(&db)
            .await
            .expect("task service runtime config should reload");

        let test_dir = format!("/tmp/asterforge-task-service-{}", uuid::Uuid::new_v4());
        let temp_dir = format!("{test_dir}/temp");
        std::fs::create_dir_all(&temp_dir).expect("task service temp dir should exist");

        let config = Arc::new(crate::config::Config {
            server: crate::config::ServerConfig {
                temp_dir,
                ..Default::default()
            },
            database: db_cfg,
            cache: crate::config::CacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        });
        let cache = crate::cache::create_cache(&config.cache).await;

        AppState {
            db_handles: crate::db::DbHandles::single(db),
            config,
            runtime_config,
            cache,
            metrics: crate::metrics_core::NoopMetrics::arc(),
            background_task_dispatch_wakeup: AppState::new_background_task_dispatch_wakeup(),
        }
    }

    fn task_model(
        status: BackgroundTaskStatus,
        payload_json: StoredTaskPayload,
    ) -> background_task::Model {
        let now = Utc::now();
        background_task::Model {
            id: 42,
            kind: BackgroundTaskKind::SystemRuntime,
            status,
            creator_user_id: None,
            display_name: "Task".to_string(),
            payload_json,
            result_json: None,
            runtime_json: None,
            steps_json: None,
            progress_current: 0,
            progress_total: 1,
            status_text: None,
            attempt_count: 0,
            max_attempts: 1,
            next_run_at: now,
            processing_token: 0,
            processing_started_at: None,
            last_heartbeat_at: None,
            lease_expires_at: None,
            started_at: None,
            finished_at: None,
            last_error: None,
            failure_can_retry: None,
            expires_at: now + Duration::hours(24),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn admin_cleanup_rejects_non_terminal_status() {
        assert!(
            validate_admin_task_cleanup_status(Some(BackgroundTaskStatus::Processing)).is_err()
        );
        assert!(validate_admin_task_cleanup_status(Some(BackgroundTaskStatus::Succeeded)).is_ok());
    }

    #[test]
    fn lease_control_errors_are_internal_messages_only() {
        let lease = TaskLease::new(1, 2);
        let lost = task_lease_lost(lease);
        assert!(is_task_lease_lost(&lost));
        assert!(!is_task_lease_renewal_timed_out(&lost));
        assert!(!is_task_worker_shutdown_requested(&lost));
    }

    #[tokio::test]
    async fn typed_task_create_builds_active_model_with_truncation_and_runtime_defaults() {
        let state = test_state().await;
        state
            .runtime_config()
            .apply(crate::entities::system_config::Model {
                id: 999,
                key: operations::TASK_RETENTION_HOURS_KEY.to_string(),
                value: "48".to_string(),
                value_type: crate::types::SystemConfigValueType::Number,
                requires_restart: false,
                is_sensitive: false,
                source: SystemConfigSource::System,
                visibility: SystemConfigVisibility::Private,
                namespace: String::new(),
                category: String::new(),
                description: String::new(),
                updated_at: Utc::now(),
                updated_by: None,
            });

        let started_at = Utc::now() - Duration::minutes(2);
        let finished_at = Utc::now();
        let result = RuntimeTaskResult::from_timestamps(
            started_at,
            finished_at,
            Some("done".to_string()),
            None,
        );
        let active = TypedTaskCreate::<SystemRuntimeTask>::new(
            "x".repeat(TASK_DISPLAY_NAME_MAX_LEN + 16),
            RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::TaskCleanup),
            },
        )
        .creator_user_id(Some(7))
        .status(BackgroundTaskStatus::Succeeded)
        .progress(5, 10)
        .status_text("s".repeat(TASK_STATUS_TEXT_MAX_LEN + 16))
        .started_at(started_at)
        .finished_at(finished_at)
        .last_error(Some("e".repeat(TASK_LAST_ERROR_MAX_LEN + 16)))
        .failure_can_retry(Some(false))
        .result(&result)
        .unwrap()
        .into_active_model(&state)
        .unwrap();

        assert_eq!(active.kind.unwrap(), BackgroundTaskKind::SystemRuntime);
        assert_eq!(active.status.unwrap(), BackgroundTaskStatus::Succeeded);
        assert_eq!(active.creator_user_id.unwrap(), Some(7));
        assert_eq!(
            active.display_name.unwrap().len(),
            TASK_DISPLAY_NAME_MAX_LEN
        );
        assert_eq!(
            active.status_text.unwrap().unwrap().chars().count(),
            TASK_STATUS_TEXT_MAX_LEN
        );
        assert_eq!(
            active.last_error.unwrap().unwrap().chars().count(),
            TASK_LAST_ERROR_MAX_LEN
        );
        assert_eq!(active.progress_current.unwrap(), 5);
        assert_eq!(active.progress_total.unwrap(), 10);
        assert_eq!(active.max_attempts.unwrap(), 1);
        assert!(active.result_json.unwrap().is_some());
        assert!(active.steps_json.unwrap().is_some());
        assert_eq!(active.started_at.unwrap(), Some(started_at));
        assert_eq!(active.finished_at.unwrap(), Some(finished_at));
        assert_eq!(active.created_at.unwrap(), started_at);
        assert_eq!(active.updated_at.unwrap(), finished_at);
        assert_eq!(
            active.expires_at.unwrap(),
            finished_at + Duration::hours(48)
        );
    }

    #[tokio::test]
    async fn create_typed_task_record_persists_pending_task_without_creator() {
        let state = test_state().await;
        let task = create_typed_task_record::<SystemRuntimeTask>(
            &state,
            "Runtime task",
            &RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::AuditCleanup),
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(task.kind, BackgroundTaskKind::SystemRuntime);
        assert_eq!(task.status, BackgroundTaskStatus::Pending);
        assert_eq!(task.display_name, "Runtime task");
        assert_eq!(task.max_attempts, 1);
        assert_eq!(task.steps_json.as_ref().map(AsRef::as_ref), Some("[]"));
    }

    #[tokio::test]
    async fn task_info_decodes_payload_steps_result_and_retryability() {
        let state = test_state().await;
        let steps = vec![TaskStepInfo {
            key: "step".to_string(),
            title: "Step".to_string(),
            status: TaskStepStatus::Succeeded,
            progress_current: 1,
            progress_total: 1,
            detail: Some("done".to_string()),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
        }];
        let result = RuntimeTaskResult {
            duration_ms: 10,
            summary: Some("failed summary".to_string()),
            system_health: None,
        };
        let mut active: background_task::ActiveModel = task_model(
            BackgroundTaskStatus::Failed,
            spec::serialize_payload::<SystemRuntimeTask>(&RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::TaskCleanup),
            })
            .unwrap(),
        )
        .into();
        active.id = sea_orm::ActiveValue::NotSet;
        active.display_name = Set("Failed runtime task".to_string());
        active.result_json = Set(Some(
            spec::serialize_result::<SystemRuntimeTask>(&result).unwrap(),
        ));
        active.steps_json = Set(Some(serialize_task_steps(&steps).unwrap()));
        active.progress_current = Set(3);
        active.progress_total = Set(4);
        active.failure_can_retry = Set(Some(false));
        let task = active.insert(state.writer_db()).await.unwrap();

        let info = build_task_info(task, None).unwrap();
        assert_eq!(info.progress_percent, 75);
        assert!(!info.can_retry);
        assert_eq!(info.steps.len(), 1);
        assert!(matches!(
            info.payload,
            crate::services::task_service::types::TaskPayload::SystemRuntime(_)
        ));
        assert!(matches!(
            info.result,
            Some(crate::services::task_service::types::TaskResult::SystemRuntime(_))
        ));

        let succeeded = task_model(
            BackgroundTaskStatus::Succeeded,
            spec::serialize_payload::<SystemRuntimeTask>(&RuntimeTaskPayload {
                task_name: RuntimeTaskName::from(SystemRuntimeTaskKind::TaskCleanup),
            })
            .unwrap(),
        );
        let mut succeeded = succeeded;
        succeeded.progress_total = 0;
        assert_eq!(
            build_task_info(succeeded, None).unwrap().progress_percent,
            100
        );
    }

    #[tokio::test]
    async fn lease_guard_reports_lost_timeout_and_shutdown_states() {
        let lease = TaskLease::new(10, 20);
        let lost_guard = TaskLeaseGuard::new(lease);
        let lost = lost_guard.mark_lost();
        assert!(is_task_lease_lost(&lost));
        assert!(is_task_lease_lost(&lost_guard.ensure_active().unwrap_err()));

        let timeout_guard = TaskLeaseGuard::with_renewal_timeout(lease, StdDuration::ZERO);
        let timeout = timeout_guard.ensure_active().unwrap_err();
        assert!(is_task_lease_renewal_timed_out(&timeout));

        let shutdown = CancellationToken::new();
        let context = TaskExecutionContext::new(lease, shutdown.clone());
        shutdown.cancel();
        let error = context.ensure_active().unwrap_err();
        assert!(is_task_worker_shutdown_requested(&error));
    }

    #[tokio::test]
    async fn execution_context_sleep_and_shutdown_return_shutdown_errors() {
        let shutdown = CancellationToken::new();
        let context = TaskExecutionContext::new(TaskLease::new(11, 22), shutdown.clone());
        shutdown.cancel();

        let sleep_error = context
            .sleep_or_shutdown(StdDuration::from_secs(60))
            .await
            .unwrap_err();
        assert!(is_task_worker_shutdown_requested(&sleep_error));

        let context = TaskExecutionContext::new(TaskLease::new(12, 23), CancellationToken::new());
        context
            .sleep_or_shutdown(StdDuration::from_millis(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn temp_dir_helpers_prepare_and_cleanup_token_and_task_directories() {
        let temp_root = format!("/tmp/asterforge-task-temp-{}", uuid::Uuid::new_v4());
        let lease = TaskLease::new(123, 456);

        let token_dir = prepare_task_temp_dir_in_root(&temp_root, lease)
            .await
            .unwrap();
        assert!(tokio::fs::try_exists(&token_dir).await.unwrap());

        cleanup_task_temp_dir_for_lease_in_root(&temp_root, lease)
            .await
            .unwrap();
        assert!(!tokio::fs::try_exists(&token_dir).await.unwrap());

        let task_dir = crate::utils::paths::task_temp_dir(&temp_root, lease.task_id);
        let other_token_dir =
            crate::utils::paths::task_token_temp_dir(&temp_root, lease.task_id, 999);
        tokio::fs::create_dir_all(&other_token_dir).await.unwrap();
        assert!(tokio::fs::try_exists(&task_dir).await.unwrap());

        cleanup_task_temp_dir_for_task_in_root(&temp_root, lease.task_id)
            .await
            .unwrap();
        assert!(!tokio::fs::try_exists(&task_dir).await.unwrap());
    }

    #[test]
    fn truncation_helpers_preserve_utf8_boundaries_and_limits() {
        let long_display = format!("{}{}", "a".repeat(TASK_DISPLAY_NAME_MAX_LEN), "雪");
        assert_eq!(
            truncate_display_name(&long_display).len(),
            TASK_DISPLAY_NAME_MAX_LEN
        );
        assert_eq!(
            truncate_status_text(&"雪".repeat(TASK_STATUS_TEXT_MAX_LEN + 1))
                .chars()
                .count(),
            TASK_STATUS_TEXT_MAX_LEN
        );
        assert_eq!(
            truncate_error(&"e".repeat(TASK_LAST_ERROR_MAX_LEN + 1)).len(),
            TASK_LAST_ERROR_MAX_LEN
        );
    }
}

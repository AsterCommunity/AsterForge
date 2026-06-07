use std::future::Future;
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use futures::stream::{self, StreamExt};
use sea_orm::ActiveEnum;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::{
    DispatchStats, TASK_HEARTBEAT_INTERVAL_SECS, TaskDispatchOutcome, TaskLease, TaskLeaseGuard,
    is_task_lease_lost, is_task_lease_renewal_timed_out, task_expiration_from,
    task_lease_expires_at, truncate_error,
};
use crate::db::repository::background_task_repo;
use crate::entities::background_task;
use crate::errors::{AsterError, Result};
use crate::runtime::{AppState, SharedRuntimeState};
use crate::services::task_service::{
    TaskExecutionContext, registry,
    retry::TaskRetryClass,
    steps::{mark_active_step_failed, parse_task_steps_json, serialize_task_steps},
};
use crate::types::{BackgroundTaskKind, BackgroundTaskStatus};

pub(super) async fn run_claimed_tasks(
    state: &AppState,
    mut claimed_tasks: Vec<(background_task::Model, TaskLease)>,
    shutdown_token: CancellationToken,
) -> Result<DispatchStats> {
    let concurrency = claimed_tasks.len().max(1);
    claimed_tasks.sort_by_key(|(task, _)| (task.created_at, task.id));

    // 先把认领结果固定下来，再启动 worker。每个 lane 的容量已经在 claim 阶段扣过，
    // 这里直接把本批已认领任务全部放出去；fast_continue lane 会在本批结束后继续补位。
    let results = run_with_concurrency_limit(claimed_tasks, concurrency, |(task, lease)| {
        let state = state.clone();
        let shutdown_token = shutdown_token.clone();
        async move { process_claimed_task(&state, task, lease, shutdown_token).await }
    })
    .await;
    let mut stats = DispatchStats::default();
    let mut first_error = None;

    for result in results {
        match result {
            Ok(outcome) => stats.add_outcome(outcome),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(stats)
}
async fn process_claimed_task(
    state: &AppState,
    task: background_task::Model,
    lease: TaskLease,
    shutdown_token: CancellationToken,
) -> Result<TaskDispatchOutcome> {
    let context = TaskExecutionContext::new(lease, shutdown_token);
    let lease_guard = context.lease_guard().clone();
    let heartbeat_stop = CancellationToken::new();
    // Heartbeat must run in its own task. With SQLite the writer pool has one
    // connection; keeping heartbeat in a select! with the business future can
    // pause a future that already acquired that connection, then wait forever
    // for a second writer connection.
    let heartbeat_handle = spawn_task_heartbeat(
        state.clone(),
        task.id,
        lease.processing_token,
        lease_guard.clone(),
        heartbeat_stop.clone(),
    );
    let heartbeat_cancel_guard = heartbeat_stop.clone().drop_guard();

    let task_result = match context.ensure_active() {
        Ok(()) => registry::process_task(state, &task, context).await,
        Err(error) => Err(error),
    };
    drop(heartbeat_cancel_guard);
    stop_task_heartbeat(heartbeat_stop, heartbeat_handle).await;

    match task_result {
        Ok(()) => {
            record_task_metric(state, task.kind, "succeeded");
            Ok(TaskDispatchOutcome {
                succeeded: 1,
                ..Default::default()
            })
        }
        Err(error) => {
            // lease 丢失 / 续约超时代表“这条执行流已经过期”，不是业务失败。
            // 这时不要再把任务改成 Failed/Retry，否则旧 worker 可能覆盖新 lease 的结果。
            if is_task_lease_lost(&error)
                || is_task_lease_renewal_timed_out(&error)
                || super::super::is_task_worker_shutdown_requested(&error)
            {
                if super::super::is_task_worker_shutdown_requested(&error) {
                    release_task_for_shutdown(state, task.id, lease.processing_token).await?;
                }
                tracing::info!(
                    task_id = task.id,
                    processing_token = lease.processing_token,
                    "background task worker stopped before completion; skipping stale completion"
                );
                return Ok(TaskDispatchOutcome::default());
            }
            let attempt_count = task.attempt_count + 1;
            let error_message = truncate_error(error.message());
            let display_error_message = error_message.clone();
            let failed_steps_json =
                build_failed_task_steps_json(state, task.id, task.kind, &display_error_message)
                    .await;
            let retry_class = task_retry_class(task.kind, &error);
            let should_auto_retry =
                retry_class.should_auto_retry() && attempt_count < task.max_attempts;
            if !should_auto_retry {
                let finished_at = Utc::now();
                let failed = background_task_repo::mark_failed(
                    state.writer_db(),
                    background_task_repo::TaskFailureUpdate {
                        id: task.id,
                        processing_token: lease.processing_token,
                        attempt_count,
                        last_error: &error_message,
                        finished_at,
                        expires_at: task_expiration_from(state, finished_at),
                        steps_json: failed_steps_json.as_deref(),
                        failure_can_retry: retry_class.can_manual_retry(),
                    },
                )
                .await?;
                if !failed {
                    tracing::info!(
                        task_id = task.id,
                        processing_token = lease.processing_token,
                        "background task lease moved before failure state update; ignoring stale worker"
                    );
                    return Ok(TaskDispatchOutcome::default());
                }
                tracing::warn!(
                    task_id = task.id,
                    kind = %task.kind.to_value(),
                    attempt_count,
                    error = %display_error_message,
                    "background task permanently failed"
                );
                if failed {
                    record_task_metric(state, task.kind, "failed");
                }
                Ok(TaskDispatchOutcome {
                    failed: usize::from(failed),
                    ..Default::default()
                })
            } else {
                let retry_at = Utc::now() + Duration::seconds(retry_delay_secs(attempt_count));
                let retried = background_task_repo::mark_retry(
                    state.writer_db(),
                    task.id,
                    lease.processing_token,
                    attempt_count,
                    retry_at,
                    &error_message,
                    failed_steps_json.as_deref(),
                )
                .await?;
                if !retried {
                    tracing::info!(
                        task_id = task.id,
                        processing_token = lease.processing_token,
                        "background task lease moved before retry state update; ignoring stale worker"
                    );
                    return Ok(TaskDispatchOutcome::default());
                }
                tracing::warn!(
                    task_id = task.id,
                    kind = %task.kind.to_value(),
                    attempt_count,
                    retry_at = %retry_at,
                    error = %display_error_message,
                    "background task failed; scheduled retry"
                );
                state.wake_background_task_dispatcher();
                if retried {
                    record_task_metric(state, task.kind, "retry");
                }
                Ok(TaskDispatchOutcome {
                    retried: usize::from(retried),
                    ..Default::default()
                })
            }
        }
    }
}

fn spawn_task_heartbeat(
    state: AppState,
    task_id: i64,
    processing_token: i64,
    lease_guard: TaskLeaseGuard,
    stop_token: CancellationToken,
) -> JoinHandle<()> {
    spawn_task_heartbeat_with_interval(
        state,
        task_id,
        processing_token,
        lease_guard,
        stop_token,
        StdDuration::from_secs(TASK_HEARTBEAT_INTERVAL_SECS),
    )
}

pub(super) fn spawn_task_heartbeat_with_interval(
    state: AppState,
    task_id: i64,
    processing_token: i64,
    lease_guard: TaskLeaseGuard,
    stop_token: CancellationToken,
    interval: StdDuration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        run_task_heartbeat_loop(
            state,
            task_id,
            processing_token,
            lease_guard,
            stop_token,
            interval,
        )
        .await;
    })
}

async fn run_task_heartbeat_loop(
    state: AppState,
    task_id: i64,
    processing_token: i64,
    lease_guard: TaskLeaseGuard,
    stop_token: CancellationToken,
    interval: StdDuration,
) {
    let mut heartbeat = tokio::time::interval(interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            _ = stop_token.cancelled() => return,
            _ = heartbeat.tick() => {
                let now = Utc::now();
                // Let task completion cancel an in-flight pool acquire quickly.
                // This keeps shutdown/finish paths from waiting on a heartbeat
                // write that is no longer useful.
                let result = tokio::select! {
                    _ = stop_token.cancelled() => return,
                    result = background_task_repo::touch_heartbeat(
                        state.writer_db(),
                        task_id,
                        processing_token,
                        now,
                        task_lease_expires_at(now),
                    ) => result,
                };

                if evaluate_heartbeat_result(&lease_guard, result).is_err() {
                    return;
                }
            }
        }
    }
}

async fn stop_task_heartbeat(stop_token: CancellationToken, heartbeat_handle: JoinHandle<()>) {
    stop_token.cancel();
    if let Err(error) = heartbeat_handle.await {
        tracing::warn!(error = %error, "background task heartbeat worker stopped unexpectedly");
    }
}

async fn release_task_for_shutdown(
    state: &AppState,
    task_id: i64,
    processing_token: i64,
) -> Result<()> {
    // Graceful shutdown is neither task success nor task failure. Release the
    // current processing lease back into Retry so the next dispatcher round can
    // resume it with a fresh processing token.
    let released = background_task_repo::release_processing(
        state.writer_db(),
        task_id,
        processing_token,
        Utc::now(),
        BackgroundTaskStatus::Retry,
    )
    .await?;
    if released {
        state.wake_background_task_dispatcher();
    }
    Ok(())
}

fn record_task_metric(
    state: &impl SharedRuntimeState,
    kind: BackgroundTaskKind,
    status: &'static str,
) {
    state
        .metrics()
        .record_background_task_transition(kind.as_str(), status);
}

pub(super) fn evaluate_heartbeat_result(
    lease_guard: &TaskLeaseGuard,
    result: Result<bool>,
) -> Result<()> {
    let lease = lease_guard.lease();
    match result {
        Ok(true) => {
            lease_guard.record_renewed();
            Ok(())
        }
        Ok(false) => {
            // false 不是数据库故障，而是条件更新没命中：
            // 一般表示 status/token 已经不是当前 worker 的了，任务已被别的 lease 接管。
            tracing::info!(
                task_id = lease.task_id,
                processing_token = lease.processing_token,
                "background task lease lost; stopping outdated worker"
            );
            Err(lease_guard.mark_lost())
        }
        Err(error) => {
            // 这里只记录并等待下一轮 heartbeat 重试；真正要停 worker 的信号只能是
            // token 不再匹配，或者连续太久没有任何成功续约。
            //
            // 也就是说，瞬时 DB 抖动不会立刻触发二次认领；只有抖动长到超过
            // renewal_timeout，lease guard 才会把当前 worker 判定为不再安全继续执行。
            tracing::warn!(
                task_id = lease.task_id,
                processing_token = lease.processing_token,
                error = %error,
                "background task heartbeat update failed; continuing and retrying next heartbeat"
            );
            lease_guard.ensure_active()
        }
    }
}

async fn build_failed_task_steps_json(
    state: &AppState,
    task_id: i64,
    _kind: BackgroundTaskKind,
    error_message: &str,
) -> Option<String> {
    let latest = background_task_repo::find_by_id(state.writer_db(), task_id)
        .await
        .ok()?;
    let mut steps =
        parse_task_steps_json(latest.steps_json.as_ref().map(|raw| raw.as_ref())).ok()?;
    if steps.is_empty() {
        return None;
    }
    mark_active_step_failed(&mut steps, Some(error_message));
    serialize_task_steps(&steps).ok().map(Into::into)
}
fn retry_delay_secs(attempt_count: i32) -> i64 {
    match attempt_count {
        1 => 5,
        2 => 15,
        3 => 60,
        _ => 300,
    }
}

pub(super) fn task_retry_class(kind: BackgroundTaskKind, error: &AsterError) -> TaskRetryClass {
    super::super::registry::task_retry_class(kind, error)
}

pub(super) async fn run_with_concurrency_limit<T, O, F, Fut>(
    items: Vec<T>,
    limit: usize,
    handler: F,
) -> Vec<O>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = O>,
{
    stream::iter(items.into_iter().map(handler))
        .buffer_unordered(limit.max(1))
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{Duration, Utc};
    use migration::Migrator;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    use super::release_task_for_shutdown;
    use crate::entities::background_task;
    use crate::runtime::{AppState, SharedRuntimeState};
    use crate::types::{BackgroundTaskKind, BackgroundTaskStatus, StoredTaskPayload};

    async fn test_state() -> AppState {
        let db = crate::db::connect_with_metrics(
            &crate::config::DatabaseConfig {
                url: "sqlite::memory:".to_string(),
                pool_size: 1,
                retry_count: 0,
            },
            crate::metrics_core::NoopMetrics::arc(),
        )
        .await
        .expect("test database should connect");
        Migrator::up(&db, None)
            .await
            .expect("test database migration should run");
        crate::services::system_config_service::ensure_defaults(&db)
            .await
            .expect("system config defaults should be installed");
        let runtime_config = Arc::new(crate::config::RuntimeConfig::new());
        runtime_config
            .reload(&db)
            .await
            .expect("runtime config should load");
        let cache = crate::cache::create_cache(&crate::config::CacheConfig {
            enabled: false,
            ..Default::default()
        })
        .await;

        AppState {
            db_handles: crate::db::DbHandles::single(db),
            config: Arc::new(crate::config::Config::default()),
            runtime_config,
            cache,
            mail_sender: crate::services::mail_service::memory_sender(),
            metrics: crate::metrics_core::NoopMetrics::arc(),
            background_task_dispatch_wakeup: AppState::new_background_task_dispatch_wakeup(),
        }
    }

    #[tokio::test]
    async fn shutdown_release_returns_processing_task_to_retry_without_failure_update() {
        let state = test_state().await;
        let now = Utc::now();
        let task = background_task::ActiveModel {
            kind: Set(BackgroundTaskKind::SystemRuntime),
            status: Set(BackgroundTaskStatus::Processing),
            creator_user_id: Set(None),
            display_name: Set("Shutdown release task".to_string()),
            payload_json: Set(StoredTaskPayload(
                serde_json::json!({ "task_name": "task-cleanup" }).to_string(),
            )),
            result_json: Set(None),
            runtime_json: Set(None),
            steps_json: Set(None),
            progress_current: Set(0),
            progress_total: Set(1),
            status_text: Set(Some("in progress".to_string())),
            attempt_count: Set(2),
            max_attempts: Set(3),
            next_run_at: Set(now - Duration::seconds(30)),
            processing_token: Set(7),
            processing_started_at: Set(Some(now - Duration::seconds(20))),
            last_heartbeat_at: Set(Some(now - Duration::seconds(5))),
            lease_expires_at: Set(Some(now + Duration::seconds(30))),
            started_at: Set(Some(now - Duration::seconds(20))),
            finished_at: Set(None),
            last_error: Set(Some("previous failure".to_string())),
            failure_can_retry: Set(Some(true)),
            expires_at: Set(now + Duration::hours(24)),
            created_at: Set(now - Duration::hours(1)),
            updated_at: Set(now - Duration::seconds(5)),
            ..Default::default()
        }
        .insert(state.writer_db())
        .await
        .expect("processing task should insert");

        release_task_for_shutdown(&state, task.id, 7)
            .await
            .expect("shutdown release should succeed");

        let released = background_task::Entity::find_by_id(task.id)
            .one(state.reader_db())
            .await
            .expect("released task should query")
            .expect("released task should exist");
        assert_eq!(released.status, BackgroundTaskStatus::Retry);
        assert_eq!(released.processing_token, 7);
        assert_eq!(released.attempt_count, 2);
        assert_eq!(released.last_error.as_deref(), Some("previous failure"));
        assert_eq!(released.failure_can_retry, Some(true));
        assert_eq!(released.status_text, None);
        assert_eq!(released.processing_started_at, None);
        assert_eq!(released.last_heartbeat_at, None);
        assert_eq!(released.lease_expires_at, None);
        assert_eq!(released.finished_at, None);
    }
}

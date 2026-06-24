//! Claimed task execution lifecycle helpers.
//!
//! Product crates still own task rows, repositories, task bodies, error presentation, retention
//! policy, metrics labels, and wakeup signals. This module owns the stable execution lifecycle
//! around those product hooks: start a lease-bound context, run a heartbeat worker, ignore stale
//! workers after lease-control failures, decide whether an ordinary task error should retry or
//! permanently fail, and aggregate the resulting dispatch counters. Keeping this flow in Forge
//! makes product task systems smaller while preserving the product-owned persistence boundary.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use crate::{
    TaskDispatchOutcome, TaskExecutionContext, TaskHeartbeatStore, TaskLease, TaskRecord,
    TaskRetryClass, run_claimed_task_batch, spawn_task_heartbeat_with_interval,
    stop_task_heartbeat,
};

/// Minimal read-only view required to execute a claimed task.
pub trait ExecutableTaskRecord<Kind>: TaskRecord<Kind> {
    /// Number of attempts already persisted before the current execution.
    fn attempt_count(&self) -> i32;

    /// Maximum number of attempts allowed for automatic retry.
    fn max_attempts(&self) -> i32;
}

/// Failure update passed to product storage after a task exhausts automatic retry.
pub struct TaskPermanentFailure<'a> {
    /// Attempt count to persist for the just-finished execution.
    pub attempt_count: i32,
    /// Truncated/storable error string.
    pub storage_error: &'a str,
    /// Human-facing error string used for logs and step details.
    pub display_error: &'a str,
    /// Serialized task steps after marking the active step failed, if available.
    pub failed_steps_json: Option<&'a str>,
    /// Whether a later manual retry should be allowed.
    pub failure_can_retry: bool,
    /// Timestamp when the failure is recorded.
    pub finished_at: DateTime<Utc>,
}

/// Retry update passed to product storage after an automatically retryable failure.
pub struct TaskRetryUpdate<'a> {
    /// Attempt count to persist for the just-finished execution.
    pub attempt_count: i32,
    /// Timestamp when the next automatic retry should become due.
    pub retry_at: DateTime<Utc>,
    /// Truncated/storable error string.
    pub storage_error: &'a str,
    /// Human-facing error string used for logs and step details.
    pub display_error: &'a str,
    /// Serialized task steps after marking the active step failed, if available.
    pub failed_steps_json: Option<&'a str>,
}

/// Product hooks required by the shared claimed-task execution lifecycle.
#[async_trait::async_trait]
pub trait ClaimedTaskExecutionStore<Task, Kind>: TaskHeartbeatStore + Clone + Send + Sync {
    /// Runs the product task body.
    async fn process_task(
        &self,
        task: &Task,
        context: TaskExecutionContext,
    ) -> std::result::Result<(), Self::Error>;

    /// Returns whether an error means the current worker has lost its persisted lease.
    fn is_lease_lost_error(&self, error: &Self::Error) -> bool;

    /// Returns whether an error means heartbeat renewal exceeded the in-memory safety deadline.
    fn is_lease_renewal_timed_out_error(&self, error: &Self::Error) -> bool;

    /// Returns whether an error means cooperative shutdown should release the current lease.
    fn is_worker_shutdown_requested_error(&self, error: &Self::Error) -> bool;

    /// Classifies an ordinary task failure for retry behavior.
    fn retry_class(&self, task: &Task, error: &Self::Error) -> TaskRetryClass;

    /// Converts a task error into the string stored in the task row.
    fn storage_error(&self, error: &Self::Error) -> String;

    /// Converts the stored error string into a human-facing display string.
    fn display_error(&self, storage_error: &str) -> String;

    /// Builds serialized task steps for a failed task, if the product has step state.
    async fn failed_steps_json(&self, task: &Task, display_error: &str) -> Option<String>;

    /// Marks the claimed task permanently failed.
    async fn mark_task_failed(
        &self,
        task: &Task,
        lease: TaskLease,
        failure: TaskPermanentFailure<'_>,
    ) -> std::result::Result<bool, Self::Error>;

    /// Marks the claimed task retryable at a future timestamp.
    async fn mark_task_retry(
        &self,
        task: &Task,
        lease: TaskLease,
        retry: TaskRetryUpdate<'_>,
    ) -> std::result::Result<bool, Self::Error>;

    /// Releases a processing lease during cooperative shutdown.
    async fn release_task_for_shutdown(
        &self,
        task: &Task,
        lease: TaskLease,
    ) -> std::result::Result<bool, Self::Error>;

    /// Records a product metric or audit hook after a task transition.
    fn record_task_transition(&self, task: &Task, status: &'static str);

    /// Wakes the product dispatcher after retry or shutdown release creates runnable work.
    fn wake_dispatcher(&self);
}

/// Configuration for the shared claimed-task execution lifecycle.
#[derive(Debug, Clone, Copy)]
pub struct ClaimedTaskExecutionConfig<LeaseExpiresFn, RetryDelayFn> {
    /// In-memory timeout used by task code to stop unsafe stale workers.
    pub renewal_timeout: Duration,
    /// Interval used by the heartbeat worker.
    pub heartbeat_interval: Duration,
    /// Computes the persisted lease expiry for heartbeat writes.
    pub lease_expires_at: LeaseExpiresFn,
    /// Computes automatic retry delay in seconds from the new attempt count.
    pub retry_delay_secs: RetryDelayFn,
}

/// Runs a claimed task batch using the shared lifecycle and aggregates counters.
pub async fn run_claimed_task_batch_with_store<
    Store,
    Task,
    Kind,
    SortKey,
    SortFn,
    LeaseExpiresFn,
    RetryDelayFn,
>(
    store: Store,
    claimed_tasks: Vec<(Task, TaskLease)>,
    sort_key: SortFn,
    shutdown_token: CancellationToken,
    config: ClaimedTaskExecutionConfig<LeaseExpiresFn, RetryDelayFn>,
) -> std::result::Result<crate::DispatchStats, Store::Error>
where
    Store: ClaimedTaskExecutionStore<Task, Kind> + 'static,
    Task: ExecutableTaskRecord<Kind> + Clone + Send + Sync + 'static,
    Kind: Copy + std::fmt::Display + Send + Sync + 'static,
    SortKey: Ord,
    SortFn: FnMut(&(Task, TaskLease)) -> SortKey,
    LeaseExpiresFn: Fn(DateTime<Utc>) -> DateTime<Utc> + Copy + Send + Sync + 'static,
    RetryDelayFn: Fn(i32) -> i64 + Copy + Send + Sync + 'static,
{
    run_claimed_task_batch(claimed_tasks, sort_key, |(task, lease)| {
        let store = store.clone();
        let shutdown_token = shutdown_token.clone();
        async move { process_claimed_task(store, task, lease, shutdown_token, config).await }
    })
    .await
}

/// Runs one claimed task through heartbeat, processing, retry, and failure handling.
pub async fn process_claimed_task<Store, Task, Kind, LeaseExpiresFn, RetryDelayFn>(
    store: Store,
    task: Task,
    lease: TaskLease,
    shutdown_token: CancellationToken,
    config: ClaimedTaskExecutionConfig<LeaseExpiresFn, RetryDelayFn>,
) -> std::result::Result<TaskDispatchOutcome, Store::Error>
where
    Store: ClaimedTaskExecutionStore<Task, Kind> + 'static,
    Task: ExecutableTaskRecord<Kind> + Send + Sync + 'static,
    Kind: Copy + std::fmt::Display + Send + Sync + 'static,
    LeaseExpiresFn: Fn(DateTime<Utc>) -> DateTime<Utc> + Copy + Send + Sync + 'static,
    RetryDelayFn: Fn(i32) -> i64 + Copy + Send + Sync + 'static,
{
    let context = TaskExecutionContext::new(lease, config.renewal_timeout, shutdown_token);
    let lease_guard = context.lease_guard().clone();
    let heartbeat_stop = CancellationToken::new();
    let heartbeat_handle = spawn_task_heartbeat_with_interval(
        store.clone(),
        lease_guard.clone(),
        heartbeat_stop.clone(),
        config.heartbeat_interval,
        config.lease_expires_at,
    );
    let heartbeat_cancel_guard = heartbeat_stop.clone().drop_guard();

    let task_result = match context.ensure_active() {
        Ok(()) => store.process_task(&task, context).await,
        Err(error) => Err(Store::Error::from(error)),
    };
    drop(heartbeat_cancel_guard);
    stop_task_heartbeat(heartbeat_stop, heartbeat_handle).await;

    match task_result {
        Ok(()) => {
            store.record_task_transition(&task, "succeeded");
            Ok(TaskDispatchOutcome::succeeded())
        }
        Err(error)
            if store.is_lease_lost_error(&error)
                || store.is_lease_renewal_timed_out_error(&error)
                || store.is_worker_shutdown_requested_error(&error) =>
        {
            if store.is_worker_shutdown_requested_error(&error)
                && store.release_task_for_shutdown(&task, lease).await?
            {
                store.wake_dispatcher();
            }
            tracing::info!(
                task_id = task.id(),
                processing_token = lease.processing_token,
                "background task worker stopped before completion; skipping stale completion"
            );
            Ok(TaskDispatchOutcome::default())
        }
        Err(error) => {
            let attempt_count = task.attempt_count().saturating_add(1);
            let storage_error = store.storage_error(&error);
            let display_error = store.display_error(&storage_error);
            let failed_steps_json = store.failed_steps_json(&task, &display_error).await;
            let retry_class = store.retry_class(&task, &error);
            let should_auto_retry =
                retry_class.should_auto_retry() && attempt_count < task.max_attempts();

            if should_auto_retry {
                let retry_at = Utc::now()
                    + chrono::Duration::seconds((config.retry_delay_secs)(attempt_count));
                let retried = store
                    .mark_task_retry(
                        &task,
                        lease,
                        TaskRetryUpdate {
                            attempt_count,
                            retry_at,
                            storage_error: &storage_error,
                            display_error: &display_error,
                            failed_steps_json: failed_steps_json.as_deref(),
                        },
                    )
                    .await?;
                if !retried {
                    tracing::info!(
                        task_id = task.id(),
                        processing_token = lease.processing_token,
                        "background task lease moved before retry state update; ignoring stale worker"
                    );
                    return Ok(TaskDispatchOutcome::default());
                }

                tracing::warn!(
                    task_id = task.id(),
                    kind = %task.kind(),
                    attempt_count,
                    retry_at = %retry_at,
                    error = %display_error,
                    "background task failed; scheduled retry"
                );
                store.wake_dispatcher();
                store.record_task_transition(&task, "retry");
                Ok(TaskDispatchOutcome::retried())
            } else {
                let finished_at = Utc::now();
                let failed = store
                    .mark_task_failed(
                        &task,
                        lease,
                        TaskPermanentFailure {
                            attempt_count,
                            storage_error: &storage_error,
                            display_error: &display_error,
                            failed_steps_json: failed_steps_json.as_deref(),
                            failure_can_retry: retry_class.can_manual_retry(),
                            finished_at,
                        },
                    )
                    .await?;
                if !failed {
                    tracing::info!(
                        task_id = task.id(),
                        processing_token = lease.processing_token,
                        "background task lease moved before failure state update; ignoring stale worker"
                    );
                    return Ok(TaskDispatchOutcome::default());
                }

                tracing::warn!(
                    task_id = task.id(),
                    kind = %task.kind(),
                    attempt_count,
                    error = %display_error,
                    "background task permanently failed"
                );
                store.record_task_transition(&task, "failed");
                Ok(TaskDispatchOutcome::failed())
            }
        }
    }
}

/// Converts an operation into a boxed future.
pub fn boxed_task_future<'a, T, Error, Fut>(future: Fut) -> crate::TaskProcessFuture<'a, Error>
where
    Fut: Future<Output = std::result::Result<T, Error>> + Send + 'a,
    Error: Send + 'a,
{
    Box::pin(async move {
        future.await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use tokio_util::sync::CancellationToken;

    use super::{
        ClaimedTaskExecutionConfig, ClaimedTaskExecutionStore, ExecutableTaskRecord,
        TaskPermanentFailure, TaskRetryUpdate, process_claimed_task,
        run_claimed_task_batch_with_store,
    };
    use crate::{
        DispatchStats, TaskCoreError, TaskExecutionContext, TaskHeartbeatStore, TaskLease,
        TaskRecord, TaskRetryClass, default_task_retry_delay_secs, task_lease_expires_at,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestKind {
        Example,
    }

    impl std::fmt::Display for TestKind {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("example")
        }
    }

    #[derive(Debug, Clone)]
    struct TestTask {
        id: i64,
        attempt_count: i32,
        max_attempts: i32,
        order: i32,
    }

    impl TaskRecord<TestKind> for TestTask {
        fn id(&self) -> i64 {
            self.id
        }

        fn kind(&self) -> TestKind {
            TestKind::Example
        }

        fn payload_json(&self) -> &str {
            "{}"
        }

        fn result_json(&self) -> Option<&str> {
            None
        }
    }

    impl ExecutableTaskRecord<TestKind> for TestTask {
        fn attempt_count(&self) -> i32 {
            self.attempt_count
        }

        fn max_attempts(&self) -> i32 {
            self.max_attempts
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
    enum TestError {
        #[error("{0}")]
        Core(#[from] TaskCoreError),
        #[error("{0}")]
        Business(String),
    }

    #[derive(Debug, Default)]
    struct StoreState {
        process_results: VecDeque<std::result::Result<(), TestError>>,
        failed: usize,
        retried: usize,
        released: usize,
        wakes: usize,
        transitions: Vec<&'static str>,
        processed_order: Vec<i64>,
        last_failed_steps: Option<String>,
    }

    #[derive(Clone)]
    struct TestStore {
        state: Arc<Mutex<StoreState>>,
    }

    impl TestStore {
        fn with_results(results: Vec<std::result::Result<(), TestError>>) -> Self {
            Self {
                state: Arc::new(Mutex::new(StoreState {
                    process_results: results.into(),
                    ..StoreState::default()
                })),
            }
        }

        fn state(&self) -> std::sync::MutexGuard<'_, StoreState> {
            self.state.lock().expect("store state should lock")
        }
    }

    #[async_trait::async_trait]
    impl TaskHeartbeatStore for TestStore {
        type Error = TestError;

        async fn touch_task_heartbeat(
            &self,
            _lease: TaskLease,
            _now: DateTime<Utc>,
            _lease_expires_at: DateTime<Utc>,
        ) -> std::result::Result<bool, Self::Error> {
            Ok(true)
        }
    }

    #[async_trait::async_trait]
    impl ClaimedTaskExecutionStore<TestTask, TestKind> for TestStore {
        async fn process_task(
            &self,
            task: &TestTask,
            _context: TaskExecutionContext,
        ) -> std::result::Result<(), Self::Error> {
            let mut state = self.state();
            state.processed_order.push(task.id);
            state.process_results.pop_front().unwrap_or(Ok(()))
        }

        fn is_lease_lost_error(&self, error: &Self::Error) -> bool {
            matches!(error, TestError::Core(error) if error.is_task_lease_lost())
        }

        fn is_lease_renewal_timed_out_error(&self, error: &Self::Error) -> bool {
            matches!(error, TestError::Core(error) if error.is_task_lease_renewal_timed_out())
        }

        fn is_worker_shutdown_requested_error(&self, error: &Self::Error) -> bool {
            matches!(error, TestError::Core(error) if error.is_task_worker_shutdown_requested())
        }

        fn retry_class(&self, _task: &TestTask, error: &Self::Error) -> TaskRetryClass {
            match error {
                TestError::Business(message) if message == "never" => TaskRetryClass::Never,
                _ => TaskRetryClass::Auto,
            }
        }

        fn storage_error(&self, error: &Self::Error) -> String {
            error.to_string()
        }

        fn display_error(&self, storage_error: &str) -> String {
            storage_error.to_string()
        }

        async fn failed_steps_json(&self, _task: &TestTask, display_error: &str) -> Option<String> {
            Some(format!("failed:{display_error}"))
        }

        async fn mark_task_failed(
            &self,
            _task: &TestTask,
            _lease: TaskLease,
            failure: TaskPermanentFailure<'_>,
        ) -> std::result::Result<bool, Self::Error> {
            let mut state = self.state();
            state.failed += 1;
            state.last_failed_steps = failure.failed_steps_json.map(str::to_string);
            Ok(true)
        }

        async fn mark_task_retry(
            &self,
            _task: &TestTask,
            _lease: TaskLease,
            _retry: TaskRetryUpdate<'_>,
        ) -> std::result::Result<bool, Self::Error> {
            self.state().retried += 1;
            Ok(true)
        }

        async fn release_task_for_shutdown(
            &self,
            _task: &TestTask,
            _lease: TaskLease,
        ) -> std::result::Result<bool, Self::Error> {
            self.state().released += 1;
            Ok(true)
        }

        fn record_task_transition(&self, _task: &TestTask, status: &'static str) {
            self.state().transitions.push(status);
        }

        fn wake_dispatcher(&self) {
            self.state().wakes += 1;
        }
    }

    type TestExecutionConfig =
        ClaimedTaskExecutionConfig<fn(DateTime<Utc>) -> DateTime<Utc>, fn(i32) -> i64>;

    fn config() -> TestExecutionConfig {
        ClaimedTaskExecutionConfig {
            renewal_timeout: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(60),
            lease_expires_at: |now| task_lease_expires_at(now, 60),
            retry_delay_secs: default_task_retry_delay_secs,
        }
    }

    fn task(id: i64, attempt_count: i32, max_attempts: i32) -> TestTask {
        TestTask {
            id,
            attempt_count,
            max_attempts,
            order: i32::try_from(id).expect("test id should fit in i32"),
        }
    }

    #[tokio::test]
    async fn process_claimed_task_records_success() {
        let store = TestStore::with_results(vec![Ok(())]);

        let outcome = process_claimed_task(
            store.clone(),
            task(7, 0, 3),
            TaskLease::new(7, 2),
            CancellationToken::new(),
            config(),
        )
        .await
        .expect("task should succeed");

        assert_eq!(outcome.succeeded, 1);
        assert_eq!(store.state().transitions, vec!["succeeded"]);
    }

    #[tokio::test]
    async fn process_claimed_task_retries_auto_failure_with_budget() {
        let store = TestStore::with_results(vec![Err(TestError::Business("retry".to_string()))]);

        let outcome = process_claimed_task(
            store.clone(),
            task(7, 0, 3),
            TaskLease::new(7, 2),
            CancellationToken::new(),
            config(),
        )
        .await
        .expect("task should retry");

        assert_eq!(outcome.retried, 1);
        let state = store.state();
        assert_eq!(state.retried, 1);
        assert_eq!(state.wakes, 1);
        assert_eq!(state.transitions, vec!["retry"]);
    }

    #[tokio::test]
    async fn process_claimed_task_fails_when_retry_budget_is_exhausted() {
        let store = TestStore::with_results(vec![Err(TestError::Business("retry".to_string()))]);

        let outcome = process_claimed_task(
            store.clone(),
            task(7, 2, 3),
            TaskLease::new(7, 2),
            CancellationToken::new(),
            config(),
        )
        .await
        .expect("task should fail permanently");

        assert_eq!(outcome.failed, 1);
        let state = store.state();
        assert_eq!(state.failed, 1);
        assert_eq!(state.last_failed_steps.as_deref(), Some("failed:retry"));
        assert_eq!(state.transitions, vec!["failed"]);
    }

    #[tokio::test]
    async fn process_claimed_task_releases_shutdown_without_failure() {
        let lease = TaskLease::new(7, 2);
        let store = TestStore::with_results(vec![Err(TestError::Core(
            TaskCoreError::WorkerShutdownRequested {
                task_id: lease.task_id,
                processing_token: lease.processing_token,
            },
        ))]);

        let outcome = process_claimed_task(
            store.clone(),
            task(7, 0, 3),
            lease,
            CancellationToken::new(),
            config(),
        )
        .await
        .expect("shutdown should release");

        assert_eq!(outcome, crate::TaskDispatchOutcome::default());
        let state = store.state();
        assert_eq!(state.released, 1);
        assert_eq!(state.wakes, 1);
        assert_eq!(state.failed, 0);
        assert_eq!(state.retried, 0);
        assert!(state.transitions.is_empty());
    }

    #[tokio::test]
    async fn batch_runner_sorts_and_aggregates_claimed_tasks() {
        let store = TestStore::with_results(vec![Ok(()), Ok(())]);
        let claimed = vec![
            (task(2, 0, 3), TaskLease::new(2, 1)),
            (task(1, 0, 3), TaskLease::new(1, 1)),
        ];

        let stats = run_claimed_task_batch_with_store(
            store.clone(),
            claimed,
            |(task, _)| task.order,
            CancellationToken::new(),
            config(),
        )
        .await
        .expect("batch should succeed");

        assert_eq!(
            stats,
            DispatchStats {
                claimed: 0,
                succeeded: 2,
                retried: 0,
                failed: 0,
            }
        );
        assert_eq!(store.state().processed_order, vec![1, 2]);
    }
}

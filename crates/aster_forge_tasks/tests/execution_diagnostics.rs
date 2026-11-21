//! Isolated subscriber: unrelated parallel unit tests must not share diagnostic callsites.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use aster_forge_tasks::{
    ClaimedTaskExecutionConfig, ClaimedTaskExecutionStore, ExecutableTaskRecord, Result,
    TaskCoreError, TaskDispatchOutcome, TaskExecutionContext, TaskHeartbeatOutcome,
    TaskHeartbeatStore, TaskLease, TaskPermanentFailure, TaskRecord, TaskRetryClass,
    TaskRetryUpdate, default_task_retry_delay_secs, process_claimed_task, task_lease_expires_at,
};
use aster_forge_test::logs::CapturedLogs;
use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Record {
    attempt: i32,
    canceled: bool,
}
impl TaskRecord<&'static str> for Record {
    fn payload_json(&self) -> &'static str {
        r#"{"secret":"SECRET-payload-canary"}"#
    }
    fn result_json(&self) -> Option<&str> {
        None
    }
    fn id(&self) -> i64 {
        7
    }
    fn kind(&self) -> &'static str {
        "example"
    }
}
impl ExecutableTaskRecord<&'static str> for Record {
    fn cancellation_requested(&self) -> bool {
        self.canceled
    }
    fn attempt_count(&self) -> i32 {
        self.attempt
    }
    fn max_attempts(&self) -> i32 {
        3
    }
}

#[derive(Clone)]
struct Store {
    result: Result<()>,
    pending: bool,
    processed: Arc<AtomicBool>,
    acknowledge_cancel: Arc<AtomicBool>,
    transitions: Arc<Mutex<Vec<&'static str>>>,
}
impl Store {
    fn new(result: Result<()>) -> Self {
        Self {
            result,
            pending: false,
            processed: Arc::default(),
            acknowledge_cancel: Arc::default(),
            transitions: Arc::default(),
        }
    }
}
#[async_trait::async_trait]
impl TaskHeartbeatStore for Store {
    type Error = TaskCoreError;
    async fn touch_task_heartbeat(
        &self,
        _: TaskLease,
        _: DateTime<Utc>,
        _: DateTime<Utc>,
    ) -> Result<TaskHeartbeatOutcome> {
        Ok(TaskHeartbeatOutcome::Renewed)
    }
}
#[async_trait::async_trait]
impl ClaimedTaskExecutionStore<Record, &'static str> for Store {
    async fn process_task(&self, _: &Record, _: TaskExecutionContext) -> Result<()> {
        self.processed.store(true, Ordering::Release);
        if self.pending {
            std::future::pending::<()>().await;
        }
        self.result.clone()
    }
    async fn confirm_task_succeeded(&self, _: &Record, _: TaskLease) -> Result<bool> {
        Ok(true)
    }
    fn is_lease_lost_error(&self, error: &TaskCoreError) -> bool {
        error.is_task_lease_lost()
    }
    fn is_lease_renewal_timed_out_error(&self, error: &TaskCoreError) -> bool {
        error.is_task_lease_renewal_timed_out()
    }
    fn is_worker_shutdown_requested_error(&self, error: &TaskCoreError) -> bool {
        error.is_task_worker_shutdown_requested()
    }
    fn is_cancellation_requested_error(&self, error: &TaskCoreError) -> bool {
        error.is_task_cancellation_requested()
    }
    async fn cancel_task_if_requested(&self, _: &Record, _: TaskLease) -> Result<bool> {
        Ok(self.acknowledge_cancel.swap(false, Ordering::AcqRel))
    }
    fn retry_class(&self, _: &Record, _: &TaskCoreError) -> TaskRetryClass {
        TaskRetryClass::Auto
    }
    fn storage_error(&self, error: &TaskCoreError) -> String {
        error.to_string()
    }
    fn display_error(&self, error: &str) -> String {
        error.to_owned()
    }
    async fn failed_steps_json(&self, _: &Record, _: &str) -> Option<String> {
        None
    }
    async fn mark_task_failed(
        &self,
        _: &Record,
        _: TaskLease,
        _: TaskPermanentFailure<'_>,
    ) -> Result<bool> {
        Ok(true)
    }
    async fn mark_task_retry(
        &self,
        _: &Record,
        _: TaskLease,
        _: TaskRetryUpdate<'_>,
    ) -> Result<bool> {
        Ok(true)
    }
    async fn release_task_for_shutdown(&self, _: &Record, _: TaskLease) -> Result<bool> {
        Ok(true)
    }
    fn record_task_transition(&self, _: &Record, status: &'static str) {
        self.transitions.lock().unwrap().push(status);
    }
    fn wake_dispatcher(&self) {}
}

async fn execute(store: Store, attempt: i32, canceled: bool) -> Result<TaskDispatchOutcome> {
    process_claimed_task(
        store,
        Record { attempt, canceled },
        TaskLease::new(7, 2),
        CancellationToken::new(),
        ClaimedTaskExecutionConfig {
            renewal_timeout: Duration::from_mins(1),
            heartbeat_interval: Duration::from_mins(1),
            lease_expires_at: |now| task_lease_expires_at(now, 60),
            retry_delay_secs: default_task_retry_delay_secs,
        },
    )
    .await
}

#[tokio::test]
async fn outcomes_control_stop_drop_and_sensitive_error_redaction() {
    let logs = CapturedLogs::default();
    logs.capture_future(async {
        for (result, attempt) in [(Ok(()), 0), (Err(TaskCoreError::io("SECRET-payload-canary")), 0), (Err(TaskCoreError::io("SECRET-payload-canary")), 2)] {
            let outcome = execute(Store::new(result), attempt, false).await.unwrap();
            assert_eq!(outcome.succeeded + outcome.retried + outcome.failed, 1);
        }
        for error in [TaskCoreError::LeaseLost { task_id: 7, processing_token: 2 }, TaskCoreError::LeaseRenewalTimedOut { task_id: 7, processing_token: 2 }, TaskCoreError::WorkerShutdownRequested { task_id: 7, processing_token: 2 }] {
            let outcome = execute(Store::new(Err(error)), 0, false).await.unwrap();
            assert_eq!(outcome, TaskDispatchOutcome::default());
        }
        let canceled = Store::new(Ok(()));
        canceled.acknowledge_cancel.store(true, Ordering::Release);
        assert_eq!(execute(canceled.clone(), 0, true).await.unwrap().canceled, 1);
        assert!(!canceled.processed.load(Ordering::Acquire));
        let mut pending = Store::new(Ok(()));
        pending.pending = true;
        let mut future = Box::pin(execute(pending.clone(), 0, false));
        tokio::select! {
            result = &mut future => panic!("unexpected completion {result:?}"),
            () = async { while !pending.processed.load(Ordering::Acquire) { tokio::task::yield_now().await; } } => {}
        }
        drop(future);
        assert!(pending.transitions.lock().unwrap().is_empty());
    }).await;
    let text = logs.text();
    for field in [
        "task_id=7",
        "processing_token=2",
        "outcome=\"succeeded\"",
        "outcome=\"retry\"",
        "outcome=\"failed\"",
        "outcome=\"canceled\"",
        "reason=\"lease_lost\"",
        "reason=\"renewal_timeout\"",
        "reason=\"shutdown\"",
        "durable outcome unknown",
    ] {
        assert!(text.contains(field), "missing {field}: {text}");
    }
    assert!(!text.contains("SECRET-payload-canary"), "{text}");
}

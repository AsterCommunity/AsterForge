#![cfg(feature = "runtime")]

use std::sync::Arc;
use std::time::Duration;

use aster_forge_tasks::{
    TaskConsumer, TaskConsumerConfig, TaskDispatchOutcome, TaskRuntimeStatus, TaskWakeup,
    run_task_consumer,
};
use aster_forge_test::logs::CapturedLogs;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct FailingConsumer {
    wakeup: TaskWakeup,
    claimed: Arc<Notify>,
}

#[async_trait::async_trait]
impl TaskConsumer for FailingConsumer {
    type Task = ();
    type Error = &'static str;
    fn name(&self) -> &'static str {
        "fallback"
    }
    fn concurrency(&self) -> usize {
        1
    }
    fn poll_interval(&self) -> Duration {
        Duration::from_mins(1)
    }
    fn wakeup(&self) -> &TaskWakeup {
        &self.wakeup
    }
    fn error_kind(&self, _: &Self::Error) -> &'static str {
        "database_unavailable"
    }
    async fn claim(&self, _: usize) -> Result<Vec<()>, Self::Error> {
        self.claimed.notify_one();
        Err("SECRET-database-DSN-canary")
    }
    async fn execute(
        &self,
        (): (),
        _: CancellationToken,
    ) -> Result<TaskDispatchOutcome, Self::Error> {
        unreachable!()
    }
    async fn release_unstarted(&self, (): ()) -> Result<(), Self::Error> {
        unreachable!()
    }
}

#[tokio::test]
async fn spawned_claims_keep_identity_trigger_timing_and_safe_error_classification() {
    let logs = CapturedLogs::default();
    let consumer = FailingConsumer {
        wakeup: TaskWakeup::default(),
        claimed: Arc::new(Notify::new()),
    };
    let stop = CancellationToken::new();
    let (report, ()) = logs
        .capture_future(async {
            tokio::join!(
                run_task_consumer(
                    consumer.clone(),
                    stop.clone(),
                    TaskConsumerConfig::default(),
                    TaskRuntimeStatus::default()
                ),
                async {
                    consumer.claimed.notified().await;
                    stop.cancel();
                }
            )
        })
        .await;
    assert_eq!(report.unwrap().claim_errors, 1);
    let text = logs.text();
    for field in [
        "consumer=\"fallback\"",
        "consumer_id=",
        "wakeup_id=",
        "task consumer claim started",
        "claim_sequence=1",
        "trigger=\"startup\"",
        "elapsed_seconds=",
        "error_kind=\"database_unavailable\"",
    ] {
        assert!(text.contains(field), "missing {field}: {text}");
    }
    assert!(!text.contains("SECRET-database-DSN-canary"), "{text}");
}

//! Run with `RUST_LOG=info,aster_forge_tasks=trace` and `--features runtime`.
//! The in-memory queue stands in for product persistence; it is not a durable store.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aster_forge_tasks::{
    TaskConsumer, TaskConsumerConfig, TaskDispatchOutcome, TaskRuntimeStatus, TaskWakeup,
    run_task_consumer,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Queue {
    name: &'static str,
    pending: Arc<Mutex<Vec<u64>>>,
    wakeup: TaskWakeup,
    stop: CancellationToken,
}

#[async_trait::async_trait]
impl TaskConsumer for Queue {
    type Task = u64;
    type Error = std::io::Error;
    fn name(&self) -> &'static str {
        self.name
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
    async fn claim(&self, available: usize) -> Result<Vec<u64>, Self::Error> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| std::io::Error::other("queue unavailable"))?;
        Ok(if available > 0 {
            pending.pop().into_iter().collect()
        } else {
            Vec::new()
        })
    }
    async fn execute(
        &self,
        task: u64,
        _: CancellationToken,
    ) -> Result<TaskDispatchOutcome, Self::Error> {
        tracing::info!(task_id = task, lane = self.name, "example task completed");
        self.stop.cancel();
        Ok(TaskDispatchOutcome::succeeded())
    }
    async fn release_unstarted(&self, task: u64) -> Result<(), Self::Error> {
        self.pending
            .lock()
            .map_err(|_| std::io::Error::other("queue unavailable"))?
            .push(task);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init()?;
    let wakeup = TaskWakeup::default();
    let stop = CancellationToken::new();
    let first = Queue {
        name: "archive",
        pending: Arc::default(),
        wakeup: wakeup.clone(),
        stop: stop.clone(),
    };
    let second = Queue {
        name: "fallback",
        pending: Arc::default(),
        wakeup: wakeup.clone(),
        stop: stop.clone(),
    };
    let statuses = [TaskRuntimeStatus::default(), TaskRuntimeStatus::default()];
    let producer_statuses = statuses.clone();
    let producer_queue = second.pending.clone();
    let producer = async move {
        while !producer_statuses.iter().all(TaskRuntimeStatus::ready) {
            tokio::task::yield_now().await;
        }
        producer_queue
            .lock()
            .map_err(|_| std::io::Error::other("queue unavailable"))?
            .push(1);
        wakeup.notify();
        Ok::<_, std::io::Error>(())
    };
    let (archive, fallback, enqueue) = tokio::join!(
        run_task_consumer(
            first,
            stop.clone(),
            TaskConsumerConfig::default(),
            statuses[0].clone()
        ),
        run_task_consumer(
            second,
            stop,
            TaskConsumerConfig::default(),
            statuses[1].clone()
        ),
        producer,
    );
    archive?;
    fallback?;
    enqueue?;
    Ok(())
}

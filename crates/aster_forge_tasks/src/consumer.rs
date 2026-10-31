//! Independently running, capacity-bounded consumers for durable task stores.
//!
//! Consumers never acquire a scheduler leader lease. The store atomically claims at most the
//! requested number of tasks, and task execution retains the existing processing-token fences.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{
    BackgroundTaskDispatchBackoff, BackgroundTaskDispatchIteration, BackgroundTaskDispatchTrigger,
    DispatchStats, TaskDispatchOutcome,
};

/// Product adapter for one independently consumable task lane.
#[async_trait::async_trait]
pub trait TaskConsumer: Clone + Send + Sync + 'static {
    /// A durably claimed task, including its processing-token lease.
    type Task: Send + 'static;
    /// Product error, mapped at the product persistence/execution boundary.
    type Error: std::fmt::Display + Send;

    /// Current maximum executions owned by this process. Zero pauses acquisition.
    fn concurrency(&self) -> usize;
    /// Maximum idle polling delay. Notifications are an optimization, never a prerequisite.
    fn poll_interval(&self) -> Duration;
    /// Maximum adaptive idle delay; defaults to fixed polling.
    fn idle_max_interval(&self) -> Duration {
        self.poll_interval()
    }
    /// Waits for an optional enqueue/config notification.
    async fn notified(&self);
    /// Atomically claims no more than `available` tasks, applying any cluster-wide quota.
    async fn claim(&self, available: usize) -> Result<Vec<Self::Task>, Self::Error>;
    /// Executes with heartbeat, fencing, retry and cooperative cancellation.
    async fn execute(
        &self,
        task: Self::Task,
        cancellation: CancellationToken,
    ) -> Result<TaskDispatchOutcome, Self::Error>;
    /// Releases a claimed task whose handler has never started, using its original token fence.
    async fn release_unstarted(&self, task: Self::Task) -> Result<(), Self::Error>;
}

/// Two-stage shutdown budget. Running work drains before cooperative cancellation is requested.
#[derive(Debug, Clone, Copy)]
pub struct TaskConsumerConfig {
    pub drain_timeout: Duration,
    pub cancellation_timeout: Duration,
}

impl TaskConsumerConfig {
    fn validate(self) -> crate::Result<()> {
        self.drain_timeout
            .checked_add(self.cancellation_timeout)
            .filter(|budget| *budget <= Duration::from_hours(24))
            .ok_or_else(|| {
                crate::TaskCoreError::invalid_value(
                    "consumer shutdown budget must not exceed 24 hours",
                )
            })?;
        Ok(())
    }
}

impl Default for TaskConsumerConfig {
    fn default() -> Self {
        Self {
            drain_timeout: Duration::from_secs(20),
            cancellation_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Default)]
struct ConsumerStatusInner {
    running: AtomicBool,
    initialized: AtomicBool,
    draining: AtomicBool,
    claim_failed: AtomicBool,
    active: AtomicUsize,
}

/// Per-runner liveness/readiness state. Do not share one instance between runners
/// or between a runner and a scheduler. An empty queue is healthy.
#[derive(Clone, Default)]
pub struct TaskRuntimeStatus(Arc<ConsumerStatusInner>);

impl TaskRuntimeStatus {
    /// Local worker liveness, independent from remote-store availability or leader ownership.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.0.running.load(Ordering::Acquire)
    }
    pub(crate) fn start(&self) -> RunningGuard {
        self.0.running.store(true, Ordering::Release);
        RunningGuard(self.clone())
    }
    pub(crate) fn observe_available(&self, available: bool) {
        self.0.initialized.store(true, Ordering::Release);
        self.0.claim_failed.store(!available, Ordering::Release);
    }

    #[must_use]
    pub fn ready(&self) -> bool {
        self.0.running.load(Ordering::Acquire)
            && self.0.initialized.load(Ordering::Acquire)
            && !self.0.draining.load(Ordering::Acquire)
            && !self.0.claim_failed.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn active(&self) -> usize {
        self.0.active.load(Ordering::Acquire)
    }
}

pub(crate) struct RunningGuard(TaskRuntimeStatus);
impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.0.running.store(false, Ordering::Release);
        self.0.0.active.store(0, Ordering::Release);
    }
}

/// Final statistics; abandoned handlers retain their durable lease until expiry and takeover.
#[derive(Debug, Default)]
pub struct TaskConsumerReport {
    pub stats: DispatchStats,
    pub execution_errors: usize,
    pub abandoned: usize,
}

type ClaimJoin<C> = tokio::task::JoinHandle<(
    usize,
    Result<Vec<<C as TaskConsumer>::Task>, <C as TaskConsumer>::Error>,
)>;

/// Runs one lane until shutdown, replenishing capacity as individual tasks finish.
///
/// `status` belongs only to this runner. Product code must aggregate role-level health
/// from separate status instances.
/// Shutdown never waits for tasks owned by another consumer or tries to empty the durable queue.
/// Claims are also spawned separately, so a slow database claim cannot block existing handlers or
/// the shutdown deadline. Claims completing after drain begins are released before they start.
/// Dropping an uncooperative execution stops its heartbeat; it must not release its lease early,
/// because product blocking operations may still be unwinding outside the async future.
///
/// # Errors
/// Returns an invalid-configuration error before storage access when the combined shutdown budget
/// overflows or exceeds 24 hours. Transient claim and execution errors are reported and isolated.
///
/// # Panics
/// Does not intentionally panic. A task-consumer worker panic is caught by its Tokio task boundary
/// and reported as an execution error; no internal claim option is unwrapped.
#[expect(
    clippy::too_many_lines,
    reason = "the consumer loop coordinates claim, execution, notification and two shutdown deadlines"
)]
pub async fn run_task_consumer<C: TaskConsumer>(
    consumer: C,
    stop_claiming: CancellationToken,
    config: TaskConsumerConfig,
    status: TaskRuntimeStatus,
) -> crate::Result<TaskConsumerReport> {
    config.validate()?;
    let mut report = TaskConsumerReport::default();
    if stop_claiming.is_cancelled() {
        return Ok(report);
    }
    let _running = status.start();
    let execution_cancel = CancellationToken::new();
    let _execution_guard = execution_cancel.clone().drop_guard();
    let mut executing = tokio::task::JoinSet::new();
    let mut claiming: Option<ClaimJoin<C>> = None;
    let mut next_poll = Instant::now();
    let mut backoff =
        BackgroundTaskDispatchBackoff::new(consumer.poll_interval(), consumer.idle_max_interval());
    let mut stopping = false;
    let mut cancelling = false;
    let mut deadline = None;

    loop {
        if stopping && executing.is_empty() && claiming.is_none() {
            break;
        }
        let wake_at = deadline.unwrap_or(next_poll);
        tokio::select! {
            biased;
            () = stop_claiming.cancelled(), if !stopping => {
                stopping = true;
                status.0.draining.store(true, Ordering::Release);
                deadline = Some(Instant::now() + config.drain_timeout);
            }
            result = executing.join_next(), if !executing.is_empty() => {
                status.0.active.fetch_sub(1, Ordering::AcqRel);
                match result {
                    Some(Ok(Ok(outcome))) => report.stats.add_outcome(outcome),
                    Some(Ok(Err(error))) => {
                        report.execution_errors += 1;
                        tracing::warn!(%error, "task consumer execution failed");
                    }
                    Some(Err(_)) => {
                        report.execution_errors += 1;
                        tracing::error!("task consumer handler panicked; its lease will expire");
                    }
                    None => {}
                }
                backoff.record_iteration(BackgroundTaskDispatchTrigger::Wakeup, BackgroundTaskDispatchIteration::active(), consumer.poll_interval(),consumer.idle_max_interval());
                next_poll = Instant::now();
            }
            result = await_claim::<C>(&mut claiming), if claiming.is_some() => {
                let Some(result) = result else { continue; };
                let _completed_claim = claiming.take();
                match result {
                    Ok((available, Ok(tasks))) => {
                        status.0.initialized.store(true, Ordering::Release);
                        status.0.claim_failed.store(false, Ordering::Release);
                        let mut iteration = BackgroundTaskDispatchIteration::idle();
                        for (index, task) in tasks.into_iter().enumerate() {
                            if stopping || stop_claiming.is_cancelled() || index >= available {
                                if let Err(error) = consumer.release_unstarted(task).await {
                                    tracing::warn!(%error, "failed to release unstarted task; awaiting lease expiry");
                                }
                                continue;
                            }
                            iteration = BackgroundTaskDispatchIteration::active();
                            let runner = consumer.clone();
                            let cancel = execution_cancel.clone();
                            executing.spawn(async move { runner.execute(task, cancel).await });
                            status.0.active.fetch_add(1, Ordering::AcqRel);
                            report.stats.claimed += 1;
                        }
                        backoff.record_iteration(BackgroundTaskDispatchTrigger::Timer,iteration,consumer.poll_interval(),consumer.idle_max_interval());
                    }
                    Ok((_available, Err(error))) => {
                        status.0.claim_failed.store(true, Ordering::Release);
                        tracing::warn!(%error, "task consumer claim failed; retrying on next poll");
                        backoff.record_iteration(BackgroundTaskDispatchTrigger::Timer,BackgroundTaskDispatchIteration::failed(),consumer.poll_interval(),consumer.idle_max_interval());
                    }
                    Err(error) => {
                        status.0.claim_failed.store(true, Ordering::Release);
                        tracing::warn!(%error, "task consumer claim task failed");
                        backoff.record_iteration(BackgroundTaskDispatchTrigger::Timer,BackgroundTaskDispatchIteration::failed(),consumer.poll_interval(),consumer.idle_max_interval());
                    }
                }
                next_poll = Instant::now();
            }
            () = tokio::time::sleep_until(wake_at) => {
                if stopping {
                    if cancelling {
                        report.abandoned = executing.len();
                        if let Some(handle) = claiming.take() {
                            handle.abort();
                        }
                        executing.abort_all();
                        break;
                    }
                    cancelling = true;
                    execution_cancel.cancel();
                    deadline = Some(Instant::now() + config.cancellation_timeout);
                    continue;
                }
                let iteration=BackgroundTaskDispatchIteration::idle();
                let available = consumer.concurrency().saturating_sub(executing.len());
                if available > 0 && claiming.is_none() {
                    let runner = consumer.clone();
                    claiming = Some(tokio::spawn(async move {
                        let result = runner.claim(available).await;
                        (available, result)
                    }));
                } else if consumer.concurrency() == 0 {
                    status.0.initialized.store(true, Ordering::Release);
                }
                backoff.record_iteration(BackgroundTaskDispatchTrigger::Timer,iteration,consumer.poll_interval(),consumer.idle_max_interval());
                next_poll = Instant::now() + backoff.sleep_duration(consumer.poll_interval(),consumer.idle_max_interval());
            }
            () = consumer.notified(), if !stopping => {
                backoff.record_iteration(BackgroundTaskDispatchTrigger::Wakeup,BackgroundTaskDispatchIteration::idle(),consumer.poll_interval(),consumer.idle_max_interval());
                next_poll = Instant::now();
            }
        }
    }
    Ok(report)
}

async fn await_claim<C: TaskConsumer>(
    claiming: &mut Option<ClaimJoin<C>>,
) -> Option<std::result::Result<(usize, Result<Vec<C::Task>, C::Error>), tokio::task::JoinError>> {
    let handle = claiming.as_mut()?;
    Some(handle.await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};
    use tokio::sync::{Notify, Semaphore, mpsc};

    #[derive(Clone)]
    struct Store {
        queue: Arc<Mutex<VecDeque<usize>>>,
        concurrency: Arc<AtomicUsize>,
        failing: Arc<AtomicBool>,
        claim_gate: Arc<Semaphore>,
        block_claim: Arc<AtomicBool>,
        claiming: Arc<Notify>,
        wake: Arc<Notify>,
        entered: mpsc::UnboundedSender<usize>,
        gates: Arc<Vec<Semaphore>>,
        released: Arc<Mutex<Vec<usize>>>,
        claims: Arc<Mutex<Vec<usize>>>,
        ignore_cancel: bool,
        overclaim: bool,
    }

    impl Store {
        fn new(tasks: &[usize], concurrency: usize) -> (Self, mpsc::UnboundedReceiver<usize>) {
            let (entered, receiver) = mpsc::unbounded_channel();
            (
                Self {
                    queue: Arc::new(Mutex::new(tasks.iter().copied().collect())),
                    concurrency: Arc::new(AtomicUsize::new(concurrency)),
                    failing: Arc::default(),
                    claim_gate: Arc::new(Semaphore::new(0)),
                    block_claim: Arc::default(),
                    claiming: Arc::default(),
                    wake: Arc::default(),
                    entered,
                    gates: Arc::new((0..16).map(|_| Semaphore::new(0)).collect()),
                    released: Arc::default(),
                    claims: Arc::default(),
                    ignore_cancel: false,
                    overclaim: false,
                },
                receiver,
            )
        }
    }

    #[async_trait::async_trait]
    impl TaskConsumer for Store {
        type Task = usize;
        type Error = &'static str;
        fn concurrency(&self) -> usize {
            self.concurrency.load(Ordering::Acquire)
        }
        fn poll_interval(&self) -> Duration {
            Duration::from_millis(10)
        }
        async fn notified(&self) {
            self.wake.notified().await;
        }
        async fn claim(&self, available: usize) -> Result<Vec<usize>, Self::Error> {
            self.claims.lock().unwrap().push(available);
            self.claiming.notify_one();
            if self.block_claim.load(Ordering::Acquire) {
                self.claim_gate.acquire().await.unwrap().forget();
            }
            if self.failing.load(Ordering::Acquire) {
                return Err("database unavailable");
            }
            let mut queue = self.queue.lock().unwrap();
            let count = queue.len().min(available + usize::from(self.overclaim));
            Ok(queue.drain(..count).collect())
        }
        async fn execute(
            &self,
            task: usize,
            cancel: CancellationToken,
        ) -> Result<TaskDispatchOutcome, Self::Error> {
            self.entered.send(task).unwrap();
            assert_ne!(task, 15, "injected handler panic");
            if self.ignore_cancel {
                self.gates[task].acquire().await.unwrap().forget();
            } else {
                tokio::select! {
                    permit = self.gates[task].acquire() => { permit.unwrap().forget(); }
                    () = cancel.cancelled() => {
                        self.released.lock().unwrap().push(task);
                        return Ok(TaskDispatchOutcome::default());
                    }
                }
            }
            if task == 14 {
                return Err("injected execution error");
            }
            Ok(TaskDispatchOutcome::succeeded())
        }
        async fn release_unstarted(&self, task: usize) -> Result<(), Self::Error> {
            self.released.lock().unwrap().push(task);
            Ok(())
        }
    }

    fn config() -> TaskConsumerConfig {
        TaskConsumerConfig {
            drain_timeout: Duration::from_millis(30),
            cancellation_timeout: Duration::from_millis(30),
        }
    }

    async fn receive(receiver: &mut mpsc::UnboundedReceiver<usize>) -> usize {
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn until(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !predicate() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn independent_consumers_claim_different_tasks_without_a_leader() {
        let (store, mut entered) = Store::new(&[0, 1], 1);
        let stop_a = CancellationToken::new();
        let stop_b = CancellationToken::new();
        let a = TaskRuntimeStatus::default();
        let b = TaskRuntimeStatus::default();
        let first = tokio::spawn(run_task_consumer(
            store.clone(),
            stop_a.clone(),
            config(),
            a.clone(),
        ));
        let second = tokio::spawn(run_task_consumer(
            store.clone(),
            stop_b.clone(),
            config(),
            b.clone(),
        ));
        assert_ne!(receive(&mut entered).await, receive(&mut entered).await);
        assert_eq!(a.active(), 1);
        assert_eq!(b.active(), 1);
        stop_a.cancel();
        first.await.unwrap().unwrap();
        assert!(!a.ready());
        assert!(b.ready(), "one runner must not clear another runner status");
        stop_b.cancel();
        second.await.unwrap().unwrap();
        assert!(!a.ready());
        assert!(!b.ready());
    }

    #[tokio::test]
    async fn fills_a_free_slot_without_waiting_for_the_slowest_task() {
        let (store, mut entered) = Store::new(&[0, 1, 2], 2);
        let stop = CancellationToken::new();
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            status.clone(),
        ));
        assert_eq!(receive(&mut entered).await, 0);
        assert_eq!(receive(&mut entered).await, 1);
        store.gates[1].add_permits(1);
        assert_eq!(receive(&mut entered).await, 2);
        assert_eq!(status.active(), 2);
        assert!(
            store
                .claims
                .lock()
                .unwrap()
                .iter()
                .skip(1)
                .all(|slots| *slots <= 1)
        );
        stop.cancel();
        let report = runner.await.unwrap().unwrap();
        assert_eq!(report.stats.succeeded, 1);
        assert_eq!(report.abandoned, 0);
    }

    #[tokio::test]
    async fn pending_claim_does_not_block_other_handlers_or_shutdown_deadlines() {
        let (store, mut entered) = Store::new(&[0, 1, 2, 3], 3);
        let stop = CancellationToken::new();
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            status.clone(),
        ));
        assert_eq!(receive(&mut entered).await, 0);
        assert_eq!(receive(&mut entered).await, 1);
        assert_eq!(receive(&mut entered).await, 2);

        store.block_claim.store(true, Ordering::Release);
        store.gates[1].add_permits(1);
        until(|| store.claims.lock().unwrap().len() >= 2).await;

        store.gates[2].add_permits(1);
        until(|| status.active() == 1).await;
        stop.cancel();
        let report = tokio::time::timeout(Duration::from_secs(1), runner)
            .await
            .expect("shutdown deadline must not wait for a blocked claim")
            .unwrap()
            .unwrap();
        assert_eq!(report.stats.succeeded, 2);
        assert_eq!(report.abandoned, 0);
        assert_eq!(
            store
                .queue
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[tokio::test]
    async fn shutdown_drains_only_owned_work_and_does_not_claim_the_queue() {
        let (store, mut entered) = Store::new(&[0, 1], 1);
        let stop = CancellationToken::new();
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            status.clone(),
        ));
        assert_eq!(receive(&mut entered).await, 0);
        stop.cancel();
        until(|| !status.ready()).await;
        store.gates[0].add_permits(1);
        let report = runner.await.unwrap().unwrap();
        assert_eq!(report.stats.succeeded, 1);
        assert_eq!(
            store
                .queue
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(store.released.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_during_claim_releases_without_starting_a_handler() {
        let (store, mut entered) = Store::new(&[0], 1);
        store.block_claim.store(true, Ordering::Release);
        let stop = CancellationToken::new();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            TaskConsumerConfig {
                drain_timeout: Duration::from_secs(2),
                cancellation_timeout: Duration::from_secs(2),
            },
            TaskRuntimeStatus::default(),
        ));
        store.claiming.notified().await;
        stop.cancel();
        store.claim_gate.add_permits(1);
        runner.await.unwrap().unwrap();
        assert!(entered.try_recv().is_err());
        assert_eq!(*store.released.lock().unwrap(), vec![0]);
    }

    #[tokio::test]
    async fn uncooperative_work_is_abandoned_without_early_lease_release() {
        let (mut store, mut entered) = Store::new(&[0], 1);
        store.ignore_cancel = true;
        let stop = CancellationToken::new();
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            status.clone(),
        ));
        receive(&mut entered).await;
        stop.cancel();
        assert_eq!(runner.await.unwrap().unwrap().abandoned, 1);
        assert!(store.released.lock().unwrap().is_empty());
        assert_eq!(status.active(), 0);
        assert!(!status.ready());
    }

    #[tokio::test]
    async fn idle_health_recovers_after_database_errors_without_notification() {
        let (store, _) = Store::new(&[], 1);
        store.failing.store(true, Ordering::Release);
        let stop = CancellationToken::new();
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            status.clone(),
        ));
        store.claiming.notified().await;
        assert!(!status.ready());
        store.failing.store(false, Ordering::Release);
        until(|| status.ready()).await;
        assert_eq!(status.active(), 0);
        stop.cancel();
        runner.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn paused_consumer_resumes_with_dynamic_capacity() {
        let (store, mut entered) = Store::new(&[0], 0);
        let stop = CancellationToken::new();
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            status.clone(),
        ));
        until(|| status.ready()).await;
        assert!(store.claims.lock().unwrap().is_empty());
        store.concurrency.store(1, Ordering::Release);
        assert_eq!(receive(&mut entered).await, 0);
        stop.cancel();
        runner.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn excess_claims_are_released_instead_of_queued_behind_capacity() {
        let (mut store, mut entered) = Store::new(&[0, 1], 1);
        store.overclaim = true;
        let stop = CancellationToken::new();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            TaskRuntimeStatus::default(),
        ));
        assert_eq!(receive(&mut entered).await, 0);
        assert_eq!(*store.released.lock().unwrap(), vec![1]);
        stop.cancel();
        runner.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancelled_before_start_does_not_touch_storage() {
        let (store, _) = Store::new(&[0], 1);
        let stop = CancellationToken::new();
        stop.cancel();
        run_task_consumer(store.clone(), stop, config(), TaskRuntimeStatus::default())
            .await
            .unwrap();
        assert!(store.claims.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_shutdown_budgets_fail_without_claiming() {
        for budget in [Duration::MAX, Duration::from_secs(86_401)] {
            let (store, _) = Store::new(&[0], 1);
            let result = run_task_consumer(
                store.clone(),
                CancellationToken::new(),
                TaskConsumerConfig {
                    drain_timeout: budget,
                    cancellation_timeout: Duration::from_secs(1),
                },
                TaskRuntimeStatus::default(),
            )
            .await;
            assert!(result.is_err());
            assert!(store.claims.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn dropping_consumer_clears_liveness_without_releasing_active_leases() {
        let (store, mut entered) = Store::new(&[0], 1);
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            CancellationToken::new(),
            config(),
            status.clone(),
        ));
        receive(&mut entered).await;
        assert!(status.ready());
        runner.abort();
        assert!(runner.await.unwrap_err().is_cancelled());
        assert!(!status.ready());
        assert_eq!(status.active(), 0);
        assert!(store.released.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn panic_and_execution_error_do_not_stop_unrelated_tasks() {
        let (store, mut entered) = Store::new(&[15, 14, 0], 2);
        let stop = CancellationToken::new();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            TaskRuntimeStatus::default(),
        ));
        assert_eq!(receive(&mut entered).await, 15);
        assert_eq!(receive(&mut entered).await, 14);
        assert_eq!(receive(&mut entered).await, 0);
        store.gates[14].add_permits(1);
        store.gates[0].add_permits(1);
        until(|| store.queue.lock().unwrap().is_empty()).await;
        stop.cancel();
        let report = runner.await.unwrap().unwrap();
        assert_eq!(report.execution_errors, 2);
        assert_eq!(report.stats.succeeded, 1);
    }
}

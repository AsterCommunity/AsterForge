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

/// Final statistics. Unknown claim outcomes and abandoned handlers retain their durable leases.
#[derive(Debug, Default)]
pub struct TaskConsumerReport {
    pub stats: DispatchStats,
    pub execution_errors: usize,
    pub claim_errors: usize,
    pub release_errors: usize,
    pub abandoned: usize,
    pub abandoned_claims: usize,
    pub abandoned_releases: usize,
}

#[derive(Clone, Copy)]
enum ConsumerPhase {
    Running,
    Draining(Instant),
    Cancelling(Instant),
}

impl ConsumerPhase {
    fn deadline(self) -> Option<Instant> {
        match self {
            Self::Running => None,
            Self::Draining(deadline) | Self::Cancelling(deadline) => Some(deadline),
        }
    }
    fn stopping(self) -> bool {
        !matches!(self, Self::Running)
    }
}

struct ClaimBatch<C: TaskConsumer> {
    reserved: usize,
    result: Result<Vec<C::Task>, C::Error>,
}

struct ConsumerRuntime<C: TaskConsumer> {
    consumer: C,
    status: TaskRuntimeStatus,
    config: TaskConsumerConfig,
    phase: ConsumerPhase,
    report: TaskConsumerReport,
    executing: tokio::task::JoinSet<Result<TaskDispatchOutcome, C::Error>>,
    claiming: tokio::task::JoinSet<ClaimBatch<C>>,
    releasing: tokio::task::JoinSet<Result<(), C::Error>>,
    cancellation: CancellationToken,
    next_poll: Instant,
    backoff: BackgroundTaskDispatchBackoff,
}

impl<C: TaskConsumer> ConsumerRuntime<C> {
    fn new(consumer: C, status: TaskRuntimeStatus, config: TaskConsumerConfig) -> Self {
        let backoff = BackgroundTaskDispatchBackoff::new(
            consumer.poll_interval(),
            consumer.idle_max_interval(),
        );
        Self {
            consumer,
            status,
            config,
            backoff,
            phase: ConsumerPhase::Running,
            report: TaskConsumerReport::default(),
            executing: tokio::task::JoinSet::new(),
            claiming: tokio::task::JoinSet::new(),
            releasing: tokio::task::JoinSet::new(),
            cancellation: CancellationToken::new(),
            next_poll: Instant::now(),
        }
    }

    fn idle(&self) -> bool {
        self.executing.is_empty() && self.claiming.is_empty() && self.releasing.is_empty()
    }

    fn record_iteration(
        &mut self,
        trigger: BackgroundTaskDispatchTrigger,
        iteration: BackgroundTaskDispatchIteration,
    ) {
        self.backoff.record_iteration(
            trigger,
            iteration,
            self.consumer.poll_interval(),
            self.consumer.idle_max_interval(),
        );
        self.next_poll = Instant::now()
            + self.backoff.sleep_duration(
                self.consumer.poll_interval(),
                self.consumer.idle_max_interval(),
            );
    }

    fn completed_execution(
        &mut self,
        result: Result<Result<TaskDispatchOutcome, C::Error>, tokio::task::JoinError>,
    ) {
        self.status
            .0
            .active
            .store(self.executing.len(), Ordering::Release);
        match result {
            Ok(Ok(outcome)) => self.report.stats.add_outcome(outcome),
            Ok(Err(error)) => {
                self.report.execution_errors += 1;
                tracing::warn!(%error, "task consumer execution failed");
            }
            Err(error) => {
                self.report.execution_errors += 1;
                tracing::error!(%error, "task consumer handler failed; its lease will expire");
            }
        }
        self.record_iteration(
            BackgroundTaskDispatchTrigger::Wakeup,
            BackgroundTaskDispatchIteration::active(),
        );
        self.next_poll = Instant::now();
    }

    fn completed_claim(
        &mut self,
        result: Result<ClaimBatch<C>, tokio::task::JoinError>,
        stop: &CancellationToken,
    ) {
        let batch = match result {
            Ok(batch) => batch,
            Err(error) => {
                self.failed_claim(&error);
                return;
            }
        };
        let tasks = match batch.result {
            Ok(tasks) => tasks,
            Err(error) => {
                self.failed_claim(&error);
                return;
            }
        };
        let reserved = batch.reserved;
        self.status.observe_available(true);
        // Re-read dynamic limits after the I/O completes; a limit may have fallen meanwhile.
        let available = reserved.min(
            self.consumer
                .concurrency()
                .saturating_sub(self.executing.len()),
        );
        let has_work = !tasks.is_empty();
        for (index, task) in tasks.into_iter().enumerate() {
            let runner = self.consumer.clone();
            if self.phase.stopping() || stop.is_cancelled() || index >= available {
                self.releasing
                    .spawn(async move { runner.release_unstarted(task).await });
            } else {
                let cancellation = self.cancellation.clone();
                self.executing
                    .spawn(async move { runner.execute(task, cancellation).await });
                self.report.stats.claimed += 1;
            }
        }
        self.status
            .0
            .active
            .store(self.executing.len(), Ordering::Release);
        self.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            if has_work {
                BackgroundTaskDispatchIteration::active()
            } else {
                BackgroundTaskDispatchIteration::idle()
            },
        );
    }

    fn failed_claim(&mut self, error: &impl std::fmt::Display) {
        self.report.claim_errors += 1;
        self.status.0.claim_failed.store(true, Ordering::Release);
        tracing::warn!(%error, "task consumer claim failed");
        self.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::failed(),
        );
    }

    fn completed_release(&mut self, result: Result<Result<(), C::Error>, tokio::task::JoinError>) {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.report.release_errors += 1;
                tracing::warn!(%error, "unstarted task release failed; awaiting lease expiry");
            }
            Err(error) => {
                self.report.release_errors += 1;
                tracing::error!(%error, "unstarted task release panicked; awaiting lease expiry");
            }
        }
    }

    fn poll(&mut self) {
        let available = self
            .consumer
            .concurrency()
            .saturating_sub(self.executing.len());
        if available > 0 {
            let runner = self.consumer.clone();
            self.claiming.spawn(async move {
                ClaimBatch {
                    reserved: available,
                    result: runner.claim(available).await,
                }
            });
        } else {
            if self.consumer.concurrency() == 0 {
                self.status.observe_available(true);
            }
            self.record_iteration(
                BackgroundTaskDispatchTrigger::Timer,
                BackgroundTaskDispatchIteration::idle(),
            );
        }
    }

    async fn abandon(&mut self) {
        self.report.abandoned = self.executing.len();
        self.report.abandoned_claims = self.claiming.len();
        self.report.abandoned_releases = self.releasing.len();
        // Abort every group before joining any of them. In particular, execution/heartbeat
        // futures must drop before the product is allowed to close database resources.
        self.executing.abort_all();
        self.claiming.abort_all();
        self.releasing.abort_all();
        self.executing.shutdown().await;
        self.claiming.shutdown().await;
        self.releasing.shutdown().await;
    }
}

/// Runs one lane, replenishing free slots while claim/release I/O and handlers run independently.
///
/// Each runner and scheduler must own a separate `status`; products aggregate role health.
/// Shutdown stops acquisition, waits for owned I/O/work, then requests cooperative cancellation.
/// Work still pending at the final deadline is aborted and joined. Claims with unknown database
/// outcomes retain their leases until expiry; they are never prematurely released.
/// Dropping the runner aborts all owned tasks. Products must keep blocking operations fenced.
///
/// # Errors
/// Rejects a combined shutdown budget over 24 hours before accessing storage. Claim/execution/
/// release failures are reported separately and do not stop unrelated work.
pub async fn run_task_consumer<C: TaskConsumer>(
    consumer: C,
    stop_claiming: CancellationToken,
    config: TaskConsumerConfig,
    status: TaskRuntimeStatus,
) -> crate::Result<TaskConsumerReport> {
    config.validate()?;
    if stop_claiming.is_cancelled() {
        return Ok(TaskConsumerReport::default());
    }
    let _running = status.start();
    let mut runtime = ConsumerRuntime::new(consumer, status, config);
    let _cancel_on_drop = runtime.cancellation.clone().drop_guard();
    loop {
        if runtime.phase.stopping() && runtime.idle() {
            break;
        }
        let deadline = runtime.phase.deadline();
        tokio::select! {
            biased;
            () = stop_claiming.cancelled(), if !runtime.phase.stopping() => {
                runtime.status.0.draining.store(true, Ordering::Release);
                runtime.phase = ConsumerPhase::Draining(Instant::now() + runtime.config.drain_timeout);
            }
            () = async {
                if let Some(deadline) = deadline { tokio::time::sleep_until(deadline).await; }
            }, if deadline.is_some() => {
                if matches!(runtime.phase, ConsumerPhase::Draining(_)) {
                    runtime.cancellation.cancel();
                    runtime.phase = ConsumerPhase::Cancelling(Instant::now() + runtime.config.cancellation_timeout);
                } else {
                    runtime.abandon().await;
                    break;
                }
            }
            Some(result) = runtime.executing.join_next(), if !runtime.executing.is_empty() => {
                runtime.completed_execution(result);
            }
            Some(result) = runtime.claiming.join_next(), if !runtime.claiming.is_empty() => {
                runtime.completed_claim(result, &stop_claiming);
            }
            Some(result) = runtime.releasing.join_next(), if !runtime.releasing.is_empty() => {
                runtime.completed_release(result);
            }
            () = tokio::time::sleep_until(runtime.next_poll), if !runtime.phase.stopping() && runtime.claiming.is_empty() => {
                runtime.poll();
            }
            () = runtime.consumer.notified(), if !runtime.phase.stopping() => {
                runtime.record_iteration(BackgroundTaskDispatchTrigger::Wakeup, BackgroundTaskDispatchIteration::idle());
                runtime.next_poll = Instant::now();
            }
        }
    }
    Ok(runtime.report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};
    use tokio::sync::{Notify, Semaphore, mpsc};

    #[derive(Clone, Copy, Default)]
    enum ReleaseMode {
        #[default]
        Normal,
        Error,
        Panic,
        Pending,
    }

    struct InFlight(Arc<AtomicUsize>);
    impl InFlight {
        fn enter(count: Arc<AtomicUsize>) -> Self {
            count.fetch_add(1, Ordering::SeqCst);
            Self(count)
        }
    }
    impl Drop for InFlight {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

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
        claim_panics: Arc<AtomicUsize>,
        claims_live: Arc<AtomicUsize>,
        executions_live: Arc<AtomicUsize>,
        releases_live: Arc<AtomicUsize>,
        release_mode: Arc<Mutex<ReleaseMode>>,
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
                    claim_panics: Arc::default(),
                    claims_live: Arc::default(),
                    executions_live: Arc::default(),
                    releases_live: Arc::default(),
                    release_mode: Arc::default(),
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
            let _in_flight = InFlight::enter(self.claims_live.clone());
            self.claims.lock().unwrap().push(available);
            self.claiming.notify_one();
            assert!(
                self.claim_panics
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| left
                        .checked_sub(1))
                    .is_err(),
                "injected claim panic"
            );
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
            let _in_flight = InFlight::enter(self.executions_live.clone());
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
            let _in_flight = InFlight::enter(self.releases_live.clone());
            let mode = *self.release_mode.lock().unwrap();
            match mode {
                ReleaseMode::Normal => {}
                ReleaseMode::Error => return Err("injected release error"),
                ReleaseMode::Panic => panic!("injected release panic"),
                ReleaseMode::Pending => std::future::pending::<()>().await,
            }
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
    #[tokio::test(start_paused = true)]
    async fn empty_queue_and_failed_claims_preserve_polling_backoff() {
        for failing in [false, true] {
            let (store, _entered) = Store::new(&[], 1);
            store.failing.store(failing, Ordering::Release);
            let stop = CancellationToken::new();
            let runner = tokio::spawn(run_task_consumer(
                store.clone(),
                stop.clone(),
                config(),
                TaskRuntimeStatus::default(),
            ));
            until(|| {
                store.claims.lock().unwrap().len() == 1
                    && store.claims_live.load(Ordering::SeqCst) == 0
            })
            .await;
            tokio::task::yield_now().await;
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                store.claims.lock().unwrap().len(),
                1,
                "idle/error may not hot-spin"
            );
            tokio::time::advance(Duration::from_millis(9)).await;
            tokio::task::yield_now().await;
            assert_eq!(store.claims.lock().unwrap().len(), 1);
            tokio::time::advance(if failing {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(1)
            })
            .await;
            until(|| store.claims.lock().unwrap().len() == 2).await;
            stop.cancel();
            let report = runner.await.unwrap().unwrap();
            assert_eq!(report.claim_errors, if failing { 2 } else { 0 });
        }
    }

    #[tokio::test]
    async fn notification_wakes_idle_consumer_and_claim_panic_recovers() {
        let (store, mut entered) = Store::new(&[], 1);
        store.claim_panics.store(1, Ordering::SeqCst);
        let stop = CancellationToken::new();
        let status = TaskRuntimeStatus::default();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            status.clone(),
        ));
        store.claiming.notified().await;
        until(|| store.claims_live.load(Ordering::SeqCst) == 0).await;
        assert!(!status.ready());
        store.queue.lock().unwrap().push_back(0);
        store.wake.notify_one();
        receive(&mut entered).await;
        assert!(status.ready());
        stop.cancel();
        let report = runner.await.unwrap().unwrap();
        assert_eq!(report.claim_errors, 1);
        assert_eq!(report.execution_errors, 0);
    }

    #[tokio::test]
    async fn pending_release_does_not_block_handler_progress_or_shutdown() {
        let (mut store, mut entered) = Store::new(&[0, 1], 1);
        store.overclaim = true;
        *store.release_mode.lock().unwrap() = ReleaseMode::Pending;
        let stop = CancellationToken::new();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            TaskRuntimeStatus::default(),
        ));
        receive(&mut entered).await;
        until(|| store.releases_live.load(Ordering::SeqCst) == 1).await;
        store.gates[0].add_permits(1);
        until(|| store.executions_live.load(Ordering::SeqCst) == 0).await;
        stop.cancel();
        let report = tokio::time::timeout(Duration::from_secs(1), runner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(report.stats.succeeded, 1);
        assert_eq!(report.abandoned, 0);
        assert_eq!(report.abandoned_releases, 1);
        assert_eq!(
            store.releases_live.load(Ordering::SeqCst),
            0,
            "abort must be joined before returning"
        );
        assert!(store.released.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_or_panicked_releases_preserve_unrelated_execution() {
        for mode in [ReleaseMode::Error, ReleaseMode::Panic] {
            let (mut store, mut entered) = Store::new(&[0, 1], 1);
            store.overclaim = true;
            *store.release_mode.lock().unwrap() = mode;
            let stop = CancellationToken::new();
            let runner = tokio::spawn(run_task_consumer(
                store.clone(),
                stop.clone(),
                config(),
                TaskRuntimeStatus::default(),
            ));
            receive(&mut entered).await;
            store.gates[0].add_permits(1);
            until(|| store.executions_live.load(Ordering::SeqCst) == 0).await;
            stop.cancel();
            let report = runner.await.unwrap().unwrap();
            assert_eq!(report.stats.succeeded, 1);
            assert_eq!(report.release_errors, 1);
            assert_eq!(report.execution_errors, 0);
        }
    }

    #[tokio::test]
    async fn dropping_runner_aborts_pending_claim_and_release_io() {
        for release in [false, true] {
            let (mut store, mut entered) = Store::new(&[0, 1], 1);
            store.overclaim = release;
            store.block_claim.store(!release, Ordering::Release);
            if release {
                *store.release_mode.lock().unwrap() = ReleaseMode::Pending;
            }
            let runner = tokio::spawn(run_task_consumer(
                store.clone(),
                CancellationToken::new(),
                config(),
                TaskRuntimeStatus::default(),
            ));
            if release {
                receive(&mut entered).await;
                until(|| store.releases_live.load(Ordering::SeqCst) > 0).await;
            } else {
                store.claiming.notified().await;
            }
            runner.abort();
            assert!(runner.await.unwrap_err().is_cancelled());
            until(|| {
                store.claims_live.load(Ordering::SeqCst)
                    + store.releases_live.load(Ordering::SeqCst)
                    + store.executions_live.load(Ordering::SeqCst)
                    == 0
            })
            .await;
            store.claim_gate.add_permits(1);
            tokio::task::yield_now().await;
            assert_eq!(
                store.queue.lock().unwrap().len(),
                if release { 0 } else { 2 },
                "an aborted claim must not continue acquiring leases"
            );
        }
    }

    #[tokio::test]
    async fn capacity_reduction_during_claim_releases_excess_claims() {
        let (store, mut entered) = Store::new(&[0, 1], 2);
        store.block_claim.store(true, Ordering::Release);
        let stop = CancellationToken::new();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config(),
            TaskRuntimeStatus::default(),
        ));
        store.claiming.notified().await;
        store.concurrency.store(1, Ordering::Release);
        store.claim_gate.add_permits(1);
        assert_eq!(receive(&mut entered).await, 0);
        until(|| store.released.lock().unwrap().len() == 1).await;
        assert_eq!(*store.released.lock().unwrap(), vec![1]);
        stop.cancel();
        runner.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn defaults_and_liveness_track_the_local_runner_during_store_failure() {
        let config = TaskConsumerConfig::default();
        assert_eq!(config.drain_timeout, Duration::from_secs(20));
        assert_eq!(config.cancellation_timeout, Duration::from_secs(5));
        let (store, _entered) = Store::new(&[], 1);
        store.failing.store(true, Ordering::Release);
        let status = TaskRuntimeStatus::default();
        assert!(!status.is_running());
        let stop = CancellationToken::new();
        let runner = tokio::spawn(run_task_consumer(
            store.clone(),
            stop.clone(),
            config,
            status.clone(),
        ));
        store.claiming.notified().await;
        assert!(status.is_running());
        assert!(!status.ready());
        stop.cancel();
        runner.await.unwrap().unwrap();
        assert!(!status.is_running());
    }
}

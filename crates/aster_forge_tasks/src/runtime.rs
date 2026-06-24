//! Runtime worker loops for background task systems.
//!
//! This module contains the product-neutral runtime mechanics shared by services that run
//! background work: a shutdown-aware task container, periodic worker loops, panic recovery for one
//! recorded iteration, jittered sleep calculation, and the adaptive idle backoff used by database
//! dispatchers. Product crates keep ownership of their task names, runtime configuration, outcome
//! records, wakeup sources, and persistence layer by passing small closures into these helpers.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::FutureExt;
use rand::RngExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

/// Default grace period used when shutting down background workers.
pub const BACKGROUND_TASK_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
/// Minimum backoff used after a dispatch iteration returns an error.
pub const BACKGROUND_TASK_DISPATCH_ERROR_BACKOFF_CAP: Duration = Duration::from_secs(5);

/// Reason a dispatch loop is about to run an iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskDispatchTrigger {
    /// Initial run immediately after worker startup.
    Startup,
    /// Regular timer-based polling.
    Timer,
    /// Product wakeup signal, usually emitted after enqueueing a task.
    Wakeup,
}

/// Activity summary returned by one dispatch iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundTaskDispatchIteration {
    has_activity: bool,
    failed: bool,
}

impl BackgroundTaskDispatchIteration {
    /// Creates an idle dispatch iteration.
    pub const fn idle() -> Self {
        Self {
            has_activity: false,
            failed: false,
        }
    }

    /// Creates a dispatch iteration that claimed or completed work.
    pub const fn active() -> Self {
        Self {
            has_activity: true,
            failed: false,
        }
    }

    /// Creates a dispatch iteration that failed.
    pub const fn failed() -> Self {
        Self {
            has_activity: false,
            failed: true,
        }
    }

    /// Returns whether the iteration performed task work.
    pub const fn has_activity(self) -> bool {
        self.has_activity
    }

    /// Returns whether the iteration failed.
    pub const fn failed_to_dispatch(self) -> bool {
        self.failed
    }
}

/// Adaptive idle backoff for background task dispatch workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundTaskDispatchBackoff {
    idle_interval: Duration,
    last_error: bool,
}

impl BackgroundTaskDispatchBackoff {
    /// Creates a dispatch backoff state using the current runtime intervals.
    pub fn new(base_interval: Duration, max_interval: Duration) -> Self {
        Self {
            idle_interval: effective_dispatch_base_interval(base_interval, max_interval),
            last_error: false,
        }
    }

    /// Returns the sleep duration for the next dispatch loop wait.
    pub fn sleep_duration(&self, base_interval: Duration, max_interval: Duration) -> Duration {
        let base_interval = effective_dispatch_base_interval(base_interval, max_interval);
        let max_interval = effective_dispatch_max_interval(base_interval, max_interval);
        if self.last_error {
            return base_interval.max(BACKGROUND_TASK_DISPATCH_ERROR_BACKOFF_CAP);
        }
        self.idle_interval.max(base_interval).min(max_interval)
    }

    /// Records the last dispatch iteration and updates the idle/error backoff state.
    pub fn record_iteration(
        &mut self,
        trigger: BackgroundTaskDispatchTrigger,
        iteration: BackgroundTaskDispatchIteration,
        base_interval: Duration,
        max_interval: Duration,
    ) {
        let base_interval = effective_dispatch_base_interval(base_interval, max_interval);
        let max_interval = effective_dispatch_max_interval(base_interval, max_interval);
        if iteration.failed {
            self.idle_interval = base_interval;
            self.last_error = true;
            return;
        }
        if iteration.has_activity || matches!(trigger, BackgroundTaskDispatchTrigger::Wakeup) {
            self.idle_interval = base_interval;
            self.last_error = false;
            return;
        }
        self.idle_interval = self
            .idle_interval
            .max(base_interval)
            .saturating_mul(2)
            .min(max_interval);
        self.last_error = false;
    }
}

/// Shutdown-aware collection of spawned background workers.
pub struct BackgroundTasks {
    shutdown_token: CancellationToken,
    handles: JoinSet<()>,
    shutdown_grace: Duration,
}

impl BackgroundTasks {
    /// Creates a task collection with a fresh shutdown token and the default shutdown grace.
    pub fn new() -> Self {
        Self::with_shutdown_token(CancellationToken::new())
    }

    /// Creates a task collection using an externally owned shutdown token.
    pub fn with_shutdown_token(shutdown_token: CancellationToken) -> Self {
        Self::with_shutdown_token_and_grace(shutdown_token, BACKGROUND_TASK_SHUTDOWN_GRACE)
    }

    /// Creates a task collection using an externally owned token and custom shutdown grace.
    pub fn with_shutdown_token_and_grace(
        shutdown_token: CancellationToken,
        shutdown_grace: Duration,
    ) -> Self {
        Self {
            shutdown_token,
            handles: JoinSet::new(),
            shutdown_grace,
        }
    }

    /// Returns a clone of the shutdown token observed by all workers in this collection.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Spawns a worker into the collection.
    pub fn push<F>(&mut self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.handles.spawn(task);
    }

    /// Requests shutdown, waits for cooperative exit, and aborts remaining workers after grace.
    pub async fn shutdown(self) {
        let BackgroundTasks {
            shutdown_token,
            mut handles,
            shutdown_grace,
        } = self;
        shutdown_token.cancel();

        let graceful_shutdown = async { while handles.join_next().await.is_some() {} };
        if tokio::time::timeout(shutdown_grace, graceful_shutdown)
            .await
            .is_err()
        {
            let aborted = handles.len();
            handles.abort_all();
            tracing::warn!(
                aborted,
                grace_secs = shutdown_grace.as_secs(),
                "background tasks did not stop before the shutdown grace period; aborting remaining workers"
            );
            while handles.join_next().await.is_some() {}
        }
    }
}

impl Default for BackgroundTasks {
    fn default() -> Self {
        Self::new()
    }
}

/// Product callbacks used by panic-protected recorded task iterations.
pub struct RecordedTaskHooks<TaskFn, PanicFn, RecordFn> {
    /// Runs the product task body.
    pub task_fn: TaskFn,
    /// Converts a panic payload message into the product's runtime outcome type.
    pub panic_outcome: PanicFn,
    /// Persists or observes one runtime task outcome.
    pub record_outcome: RecordFn,
}

impl<TaskFn, PanicFn, RecordFn> RecordedTaskHooks<TaskFn, PanicFn, RecordFn> {
    /// Creates recorded task hooks from product callbacks.
    pub const fn new(task_fn: TaskFn, panic_outcome: PanicFn, record_outcome: RecordFn) -> Self {
        Self {
            task_fn,
            panic_outcome,
            record_outcome,
        }
    }
}

/// Configuration for one periodic runtime task worker.
pub struct PeriodicTask<Name, State, IntervalFn, TaskFn, PanicFn, RecordFn> {
    /// Product task identifier.
    pub name: Name,
    /// Stable task name used in tracing spans.
    pub task_name: &'static str,
    /// Reads the latest product-configured interval.
    pub interval_fn: IntervalFn,
    /// Optional upper bound for positive jitter.
    pub jitter_cap: Option<Duration>,
    /// Shared shutdown token.
    pub shutdown_token: CancellationToken,
    /// Product runtime state passed to callbacks.
    pub state: State,
    /// Product callbacks for execution, panic conversion, and recording.
    pub hooks: RecordedTaskHooks<TaskFn, PanicFn, RecordFn>,
}

/// Runs a periodic task until shutdown.
///
/// The first iteration runs immediately unless the token is already cancelled. Each later
/// iteration sleeps using the latest product-provided interval and optional jitter cap. Panics in
/// one iteration are converted into a product outcome through `panic_outcome` and then recorded by
/// `record_outcome`; they do not kill the worker loop.
pub async fn run_periodic_task<
    Name,
    State,
    IntervalFn,
    TaskFn,
    TaskFut,
    PanicFn,
    RecordFn,
    RecordFut,
    Outcome,
>(
    task: PeriodicTask<Name, State, IntervalFn, TaskFn, PanicFn, RecordFn>,
) where
    Name: Copy + Send + 'static,
    State: Clone + Send + Sync + 'static,
    IntervalFn: Fn(&State) -> Duration + Send + Sync + 'static,
    TaskFn: Fn(State) -> TaskFut + Send + Sync + 'static,
    TaskFut: Future<Output = Outcome> + Send,
    PanicFn: Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn:
        Fn(State, Name, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut + Send + Sync + 'static,
    RecordFut: Future<Output = ()> + Send,
    Outcome: Send + 'static,
{
    let PeriodicTask {
        name,
        task_name,
        interval_fn,
        jitter_cap,
        shutdown_token,
        state,
        hooks,
    } = task;
    let RecordedTaskHooks {
        task_fn,
        panic_outcome,
        record_outcome,
    } = hooks;

    if shutdown_token.is_cancelled() {
        return;
    }
    run_recorded_task_iteration(
        name,
        task_name,
        state.clone(),
        &task_fn,
        &panic_outcome,
        &record_outcome,
    )
    .instrument(tracing::info_span!("bg_task", task.name = task_name))
    .await;

    loop {
        let sleep_duration = periodic_sleep_duration(interval_fn(&state), jitter_cap);
        tokio::select! {
            biased;
            _ = shutdown_token.cancelled() => break,
            _ = tokio::time::sleep(sleep_duration) => {}
        }

        if shutdown_token.is_cancelled() {
            break;
        }

        run_recorded_task_iteration(
            name,
            task_name,
            state.clone(),
            &task_fn,
            &panic_outcome,
            &record_outcome,
        )
        .instrument(tracing::info_span!("bg_task", task.name = task_name))
        .await;
    }
}

/// Runs one panic-protected task iteration and records its outcome.
pub async fn run_recorded_task_iteration<
    Name,
    State,
    TaskFn,
    TaskFut,
    PanicFn,
    RecordFn,
    RecordFut,
    Outcome,
>(
    name: Name,
    task_name: &'static str,
    state: State,
    task_fn: &TaskFn,
    panic_outcome: &PanicFn,
    record_outcome: &RecordFn,
) where
    Name: Copy + Send + 'static,
    State: Clone + Send + Sync + 'static,
    TaskFn: Fn(State) -> TaskFut + Send + Sync + 'static,
    TaskFut: Future<Output = Outcome> + Send,
    PanicFn: Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn:
        Fn(State, Name, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut + Send + Sync + 'static,
    RecordFut: Future<Output = ()> + Send,
    Outcome: Send + 'static,
{
    let started_at = Utc::now();
    let outcome = match AssertUnwindSafe(task_fn(state.clone()))
        .catch_unwind()
        .await
    {
        Ok(outcome) => outcome,
        Err(panic) => {
            let panic_message = panic_payload_message(&panic);
            tracing::error!("background task '{task_name}' panicked: {panic_message}");
            panic_outcome(panic_message)
        }
    };
    let finished_at = Utc::now();

    record_outcome(state, name, started_at, finished_at, outcome).await;
}

/// Runs a wakeable dispatch loop with adaptive idle backoff.
///
/// Product crates provide the wakeup future and one dispatch iteration closure. The iteration
/// closure is responsible for claim/execute logic, panic recovery if desired, metrics, and
/// persistence of runtime task history.
pub async fn run_dispatch_worker<State, BaseFn, MaxFn, WakeFn, WakeFut, DispatchFn, DispatchFut>(
    task_name: &'static str,
    shutdown_token: CancellationToken,
    state: State,
    base_interval_fn: BaseFn,
    max_interval_fn: MaxFn,
    wakeup: WakeFn,
    dispatch_iteration: DispatchFn,
) where
    State: Clone + Send + Sync + 'static,
    BaseFn: Fn(&State) -> Duration + Send + Sync + 'static,
    MaxFn: Fn(&State) -> Duration + Send + Sync + 'static,
    WakeFn: Fn(State) -> WakeFut + Send + Sync + 'static,
    WakeFut: Future<Output = ()> + Send,
    DispatchFn: Fn(State, CancellationToken) -> DispatchFut + Send + Sync + 'static,
    DispatchFut: Future<Output = BackgroundTaskDispatchIteration> + Send,
{
    let mut backoff =
        BackgroundTaskDispatchBackoff::new(base_interval_fn(&state), max_interval_fn(&state));
    if shutdown_token.is_cancelled() {
        return;
    }
    let iteration = dispatch_iteration(state.clone(), shutdown_token.clone())
        .instrument(tracing::info_span!("bg_task", task.name = task_name))
        .await;
    backoff.record_iteration(
        BackgroundTaskDispatchTrigger::Startup,
        iteration,
        base_interval_fn(&state),
        max_interval_fn(&state),
    );

    loop {
        let sleep_duration =
            backoff.sleep_duration(base_interval_fn(&state), max_interval_fn(&state));
        let trigger = tokio::select! {
            biased;
            _ = shutdown_token.cancelled() => break,
            _ = wakeup(state.clone()) => BackgroundTaskDispatchTrigger::Wakeup,
            _ = tokio::time::sleep(sleep_duration) => BackgroundTaskDispatchTrigger::Timer,
        };

        if shutdown_token.is_cancelled() {
            break;
        }

        let iteration = dispatch_iteration(state.clone(), shutdown_token.clone())
            .instrument(tracing::info_span!("bg_task", task.name = task_name))
            .await;
        backoff.record_iteration(
            trigger,
            iteration,
            base_interval_fn(&state),
            max_interval_fn(&state),
        );
    }
}

/// Returns a periodic delay with bounded positive jitter.
pub fn periodic_sleep_duration(base_interval: Duration, jitter_cap: Option<Duration>) -> Duration {
    let Some(jitter_cap) = jitter_cap else {
        return base_interval;
    };

    let max_jitter_ms = effective_jitter_cap(base_interval, jitter_cap).as_millis();
    if max_jitter_ms == 0 {
        return base_interval;
    }

    let max_jitter_ms = u128_to_u64_saturating(max_jitter_ms.min(u128::from(u64::MAX)));
    let jitter_ms = rand::rng().random_range(0..=max_jitter_ms);
    base_interval.saturating_add(Duration::from_millis(jitter_ms))
}

/// Returns the effective jitter cap for one periodic interval.
pub fn effective_jitter_cap(base_interval: Duration, jitter_cap: Duration) -> Duration {
    let bounded_ms =
        u128_to_u64_saturating(base_interval.as_millis().min(u128::from(u64::MAX))) / 10;
    jitter_cap.min(Duration::from_millis(bounded_ms))
}

/// Returns the effective dispatch base interval, enforcing a one-second minimum.
pub fn effective_dispatch_base_interval(
    base_interval: Duration,
    _max_interval: Duration,
) -> Duration {
    if base_interval.is_zero() {
        return Duration::from_secs(1);
    }
    base_interval
}

/// Returns the effective maximum dispatch interval.
pub fn effective_dispatch_max_interval(
    base_interval: Duration,
    max_interval: Duration,
) -> Duration {
    max_interval.max(base_interval)
}

fn panic_payload_message(panic: &Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn u128_to_u64_saturating(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::{DateTime, Utc};
    use tokio::sync::{Notify, oneshot};
    use tokio_util::sync::CancellationToken;

    use super::{
        BACKGROUND_TASK_DISPATCH_ERROR_BACKOFF_CAP, BackgroundTaskDispatchBackoff,
        BackgroundTaskDispatchIteration, BackgroundTaskDispatchTrigger, BackgroundTasks,
        PeriodicTask, RecordedTaskHooks, effective_jitter_cap, periodic_sleep_duration,
        run_dispatch_worker, run_periodic_task, run_recorded_task_iteration,
    };
    use std::time::Duration;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestTaskName {
        Cleanup,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestOutcome {
        Succeeded,
        Failed(String),
    }

    fn test_interval(_: &()) -> Duration {
        Duration::from_secs(60)
    }

    #[test]
    fn periodic_sleep_duration_is_unchanged_without_jitter() {
        let base = Duration::from_secs(5);
        assert_eq!(periodic_sleep_duration(base, None), base);
    }

    #[test]
    fn periodic_sleep_duration_caps_jitter_to_ten_percent_of_interval() {
        let base = Duration::from_secs(5);
        let cap = Duration::from_secs(30);

        for _ in 0..64 {
            let delay = periodic_sleep_duration(base, Some(cap));
            assert!(delay >= base);
            assert!(delay <= base + Duration::from_millis(500));
        }
    }

    #[test]
    fn periodic_sleep_duration_uses_requested_cap_when_it_is_smaller() {
        let base = Duration::from_secs(3600);
        let cap = Duration::from_secs(30);

        for _ in 0..64 {
            let delay = periodic_sleep_duration(base, Some(cap));
            assert!(delay >= base);
            assert!(delay <= base + cap);
        }
    }

    #[test]
    fn effective_jitter_cap_handles_zero_interval() {
        assert_eq!(
            effective_jitter_cap(Duration::ZERO, Duration::from_secs(30)),
            Duration::ZERO
        );
    }

    #[tokio::test]
    async fn shutdown_only_awaits_each_handle_once() {
        let mut tasks = BackgroundTasks::new();
        tasks.push(async {});

        tasks.shutdown().await;
    }

    #[tokio::test]
    async fn external_shutdown_token_stops_background_worker_before_shutdown_join() {
        let shutdown_token = CancellationToken::new();
        let mut tasks = BackgroundTasks::with_shutdown_token(shutdown_token.clone());
        let (stopped_tx, stopped_rx) = oneshot::channel();

        tasks.push({
            let shutdown_token = shutdown_token.clone();
            async move {
                shutdown_token.cancelled().await;
                let _ = stopped_tx.send(());
            }
        });

        shutdown_token.cancel();
        tokio::time::timeout(Duration::from_millis(50), stopped_rx)
            .await
            .expect("background worker should observe external shutdown")
            .expect("background worker should report shutdown");

        tasks.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_aborts_workers_after_custom_grace() {
        let mut tasks = BackgroundTasks::with_shutdown_token_and_grace(
            CancellationToken::new(),
            Duration::from_millis(1),
        );
        let calls = Arc::new(AtomicUsize::new(0));

        tasks.push({
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                futures::future::pending::<()>().await;
            }
        });

        tasks.shutdown().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recorded_iteration_records_success() {
        let recorded = Arc::new(AtomicUsize::new(0));

        run_recorded_task_iteration(
            TestTaskName::Cleanup,
            "cleanup",
            (),
            &|()| async { TestOutcome::Succeeded },
            &TestOutcome::Failed,
            &{
                let recorded = recorded.clone();
                move |(), name, started_at: DateTime<Utc>, finished_at, outcome| {
                    let recorded = recorded.clone();
                    async move {
                        assert_eq!(name, TestTaskName::Cleanup);
                        assert!(finished_at >= started_at);
                        assert_eq!(outcome, TestOutcome::Succeeded);
                        recorded.fetch_add(1, Ordering::SeqCst);
                    }
                }
            },
        )
        .await;

        assert_eq!(recorded.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recorded_iteration_converts_panic_to_failure_outcome() {
        let recorded = Arc::new(AtomicUsize::new(0));

        run_recorded_task_iteration(
            TestTaskName::Cleanup,
            "cleanup",
            (),
            &|()| async {
                panic!("boom");
                #[allow(unreachable_code)]
                TestOutcome::Succeeded
            },
            &TestOutcome::Failed,
            &{
                let recorded = recorded.clone();
                move |(), _name, _started_at, _finished_at, outcome| {
                    let recorded = recorded.clone();
                    async move {
                        assert_eq!(outcome, TestOutcome::Failed("boom".to_string()));
                        recorded.fetch_add(1, Ordering::SeqCst);
                    }
                }
            },
        )
        .await;

        assert_eq!(recorded.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn pre_cancelled_shutdown_token_skips_periodic_startup_iteration() {
        let shutdown_token = CancellationToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        shutdown_token.cancel();
        let task_fn = {
            let calls = calls.clone();
            move |()| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    TestOutcome::Succeeded
                }
            }
        };

        run_periodic_task(PeriodicTask {
            name: TestTaskName::Cleanup,
            task_name: "cleanup",
            interval_fn: test_interval,
            jitter_cap: None,
            shutdown_token,
            state: (),
            hooks: RecordedTaskHooks::new(
                task_fn,
                TestOutcome::Failed,
                |(), _name, _started_at, _finished_at, _outcome| async {},
            ),
        })
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dispatch_worker_runs_startup_and_wakeup_iterations() {
        #[derive(Clone)]
        struct State {
            notify: Arc<Notify>,
            calls: Arc<AtomicUsize>,
        }

        let shutdown_token = CancellationToken::new();
        let state = State {
            notify: Arc::new(Notify::new()),
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let calls = state.calls.clone();

        let worker = tokio::spawn(run_dispatch_worker(
            "dispatch",
            shutdown_token.clone(),
            state.clone(),
            |_| Duration::from_secs(60),
            |_| Duration::from_secs(120),
            |state: State| async move {
                state.notify.notified().await;
            },
            |state: State, _shutdown| async move {
                state.calls.fetch_add(1, Ordering::SeqCst);
                BackgroundTaskDispatchIteration::idle()
            },
        ));

        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        state.notify.notify_one();
        while calls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }

        shutdown_token.cancel();
        worker.await.expect("dispatch worker should stop cleanly");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn background_task_dispatch_zero_base_interval_uses_minimum_delay() {
        let base = Duration::ZERO;
        let max = Duration::from_secs(30);
        let mut backoff = BackgroundTaskDispatchBackoff::new(base, max);

        assert_eq!(backoff.sleep_duration(base, max), Duration::from_secs(1));

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), Duration::from_secs(2));
    }

    #[test]
    fn background_task_dispatch_backoff_grows_on_idle_and_caps() {
        let base = Duration::from_secs(5);
        let max = Duration::from_secs(30);
        let mut backoff = BackgroundTaskDispatchBackoff::new(base, max);

        assert_eq!(backoff.sleep_duration(base, max), base);

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), Duration::from_secs(10));

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), Duration::from_secs(20));

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), max);

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), max);
    }

    #[test]
    fn background_task_dispatch_backoff_resets_on_wakeup_and_activity() {
        let base = Duration::from_secs(5);
        let max = Duration::from_secs(60);
        let mut backoff = BackgroundTaskDispatchBackoff::new(base, max);

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), Duration::from_secs(20));

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Wakeup,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), base);

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), Duration::from_secs(10));

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::active(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), base);
    }

    #[test]
    fn background_task_dispatch_backoff_never_polls_faster_than_normal_after_error() {
        let base = Duration::from_secs(30);
        let max = Duration::from_secs(120);
        let mut backoff = BackgroundTaskDispatchBackoff::new(base, max);

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::failed(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), base);

        let short_base = Duration::from_secs(1);
        let mut short_backoff = BackgroundTaskDispatchBackoff::new(short_base, max);
        short_backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::failed(),
            short_base,
            max,
        );
        assert_eq!(
            short_backoff.sleep_duration(short_base, max),
            BACKGROUND_TASK_DISPATCH_ERROR_BACKOFF_CAP
        );

        backoff.record_iteration(
            BackgroundTaskDispatchTrigger::Timer,
            BackgroundTaskDispatchIteration::idle(),
            base,
            max,
        );
        assert_eq!(backoff.sleep_duration(base, max), Duration::from_secs(60));
    }
}

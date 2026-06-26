//! Scheduled runtime task catalog and runner primitives.
//!
//! A scheduled task is a product-owned runtime job with a stable name and interval. Forge keeps
//! the reusable coordination contract here: products register catalog entries, a store atomically
//! claims due firings, and the runner records one panic-protected execution before advancing the
//! next due timestamp. Concrete persistence is supplied by another crate, typically
//! `aster_forge_db`.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::FutureExt;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::runtime::panic_payload_message;
use crate::{
    BackgroundTasks, RecordedTaskHooks, RegisteredRuntimeTaskKind, periodic_sleep_duration,
};

/// One scheduled runtime task entry registered by a product runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledTaskCatalogEntry<'a> {
    /// Product namespace.
    pub namespace: &'a str,
    /// Stable task wire name.
    pub task_name: &'a str,
    /// Operator-facing display name.
    pub display_name: &'a str,
    /// First due timestamp used when inserting a new catalog row.
    pub first_run_at: DateTime<Utc>,
}

/// Request to atomically claim one due scheduled task firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledTaskClaimRequest<'a> {
    /// Product namespace.
    pub namespace: &'a str,
    /// Stable task wire name.
    pub task_name: &'a str,
    /// Process-unique runtime owner id.
    pub owner_id: &'a str,
    /// Current timestamp.
    pub now: DateTime<Utc>,
    /// Claim TTL. Another runtime may reclaim after this duration.
    pub claim_ttl: Duration,
}

/// Claimed scheduled task firing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTaskClaim {
    /// Stable row identifier.
    pub task_id: String,
    /// Product namespace.
    pub namespace: String,
    /// Stable task wire name.
    pub task_name: String,
    /// Runtime owner id that owns this claim.
    pub owner_id: String,
    /// Due timestamp that was claimed.
    pub scheduled_at: DateTime<Utc>,
    /// Claim acquisition timestamp.
    pub claimed_at: DateTime<Utc>,
    /// Claim expiry timestamp.
    pub claim_expires_at: DateTime<Utc>,
}

/// Completion update for a claimed scheduled task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTaskCompletion {
    /// Claimed firing to complete.
    pub claim: ScheduledTaskClaim,
    /// Runtime completion timestamp.
    pub finished_at: DateTime<Utc>,
    /// Next due timestamp after this completion.
    pub next_run_at: DateTime<Utc>,
}

/// Persistence contract used by scheduled task runners.
#[async_trait::async_trait]
pub trait ScheduledTaskStore: Clone + Send + Sync + 'static {
    /// Store error type.
    type Error: std::fmt::Display + Send + Sync + 'static;

    /// Ensures one scheduled task is present in the catalog.
    async fn ensure_scheduled_task(
        &self,
        entry: ScheduledTaskCatalogEntry<'_>,
    ) -> std::result::Result<(), Self::Error>;

    /// Attempts to claim one due scheduled task firing.
    async fn claim_scheduled_task(
        &self,
        request: ScheduledTaskClaimRequest<'_>,
    ) -> std::result::Result<Option<ScheduledTaskClaim>, Self::Error>;

    /// Completes a claimed firing and advances the next due timestamp.
    async fn complete_scheduled_task(
        &self,
        completion: ScheduledTaskCompletion,
    ) -> std::result::Result<bool, Self::Error>;
}

/// Configuration for one scheduled periodic runtime task worker.
pub struct ScheduledPeriodicTask<Name, State, Store, IntervalFn, TaskFn, PanicFn, RecordFn> {
    /// Product task identifier.
    pub name: Name,
    /// Product namespace.
    pub namespace: &'static str,
    /// Stable task wire name.
    pub task_name: &'static str,
    /// Operator-facing display name.
    pub display_name: &'static str,
    /// Process-unique runtime owner id.
    pub owner_id: String,
    /// Claim TTL used to recover from crashed workers.
    pub claim_ttl: Duration,
    /// Reads the latest product-configured interval.
    pub interval_fn: IntervalFn,
    /// Optional upper bound for positive jitter.
    pub jitter_cap: Option<Duration>,
    /// Shared shutdown token.
    pub shutdown_token: CancellationToken,
    /// Product runtime state passed to callbacks.
    pub state: State,
    /// Scheduled task store.
    pub store: Store,
    /// Product callbacks for execution, panic conversion, and recording.
    pub hooks: RecordedTaskHooks<TaskFn, PanicFn, RecordFn>,
}

/// Registers multiple scheduled runtime tasks with shared runner context.
///
/// Products usually have a group of scheduled tasks that all share the same
/// namespace, owner id, claim TTL, shutdown token, state, store, panic mapping,
/// and outcome recorder. This registrar lets product runtime code register each
/// task with one line while Forge assembles the full [`ScheduledPeriodicTask`]
/// runner for every entry.
pub struct ScheduledTaskRegistrar<'a, Name, State, Store, PanicFn, RecordFn, Outcome> {
    tasks: &'a mut BackgroundTasks,
    namespace: &'static str,
    owner_id: String,
    claim_ttl: Duration,
    shutdown_token: CancellationToken,
    state: State,
    store: Store,
    panic_outcome: PanicFn,
    record_outcome: RecordFn,
    _name: std::marker::PhantomData<Name>,
    _outcome: std::marker::PhantomData<Outcome>,
}

impl<'a, Name, State, Store, PanicFn, RecordFn, Outcome>
    ScheduledTaskRegistrar<'a, Name, State, Store, PanicFn, RecordFn, Outcome>
{
    /// Creates a scheduled task registrar from shared runner context.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tasks: &'a mut BackgroundTasks,
        namespace: &'static str,
        owner_id: impl Into<String>,
        claim_ttl: Duration,
        shutdown_token: CancellationToken,
        state: State,
        store: Store,
        panic_outcome: PanicFn,
        record_outcome: RecordFn,
    ) -> Self {
        Self {
            tasks,
            namespace,
            owner_id: owner_id.into(),
            claim_ttl,
            shutdown_token,
            state,
            store,
            panic_outcome,
            record_outcome,
            _name: std::marker::PhantomData,
            _outcome: std::marker::PhantomData,
        }
    }
}

impl<'a, Name, State, Store, PanicFn, RecordFn, Outcome>
    ScheduledTaskRegistrar<'a, Name, State, Store, PanicFn, RecordFn, Outcome>
where
    Name: RegisteredRuntimeTaskKind + Send + Sync + 'static,
    State: Clone + Send + Sync + 'static,
    Store: ScheduledTaskStore,
    PanicFn: Clone + Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn: Clone + Send + Sync + 'static,
    Outcome: Send + 'static,
{
    /// Registers one scheduled runtime task.
    pub fn register<IntervalFn, TaskFn, TaskFut, RecordFut>(
        &mut self,
        name: Name,
        interval_fn: IntervalFn,
        jitter_cap: Option<Duration>,
        task_fn: TaskFn,
    ) where
        IntervalFn: Fn(&State) -> Duration + Send + Sync + 'static,
        TaskFn: Fn(State) -> TaskFut + Send + Sync + 'static,
        TaskFut: Future<Output = Outcome> + Send + 'static,
        RecordFn:
            Fn(State, Name, ScheduledTaskClaim, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut,
        RecordFut: Future<Output = ()> + Send + 'static,
    {
        self.tasks
            .push(run_scheduled_periodic_task(ScheduledPeriodicTask {
                name,
                namespace: self.namespace,
                task_name: name.as_str(),
                display_name: name.display_name(),
                owner_id: self.owner_id.clone(),
                claim_ttl: self.claim_ttl,
                interval_fn,
                jitter_cap,
                shutdown_token: self.shutdown_token.clone(),
                state: self.state.clone(),
                store: self.store.clone(),
                hooks: RecordedTaskHooks::new(
                    task_fn,
                    self.panic_outcome.clone(),
                    self.record_outcome.clone(),
                ),
            }));
    }
}

/// Runs a scheduled periodic task until shutdown.
///
/// Unlike [`crate::run_periodic_task`], this runner first claims a due catalog row. If the row is
/// not due, or another process owns a fresh claim, the worker skips that iteration. Successful and
/// failed task outcomes both complete the claim and advance `next_run_at`; crashes and process
/// exits before completion are recovered by claim expiry.
pub async fn run_scheduled_periodic_task<
    Name,
    State,
    Store,
    IntervalFn,
    TaskFn,
    TaskFut,
    PanicFn,
    RecordFn,
    RecordFut,
    Outcome,
>(
    task: ScheduledPeriodicTask<Name, State, Store, IntervalFn, TaskFn, PanicFn, RecordFn>,
) where
    Name: Copy + Send + 'static,
    State: Clone + Send + Sync + 'static,
    Store: ScheduledTaskStore,
    IntervalFn: Fn(&State) -> Duration + Send + Sync + 'static,
    TaskFn: Fn(State) -> TaskFut + Send + Sync + 'static,
    TaskFut: Future<Output = Outcome> + Send + 'static,
    PanicFn: Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn: Fn(State, Name, ScheduledTaskClaim, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut
        + Send
        + Sync
        + 'static,
    RecordFut: Future<Output = ()> + Send + 'static,
    Outcome: Send + 'static,
{
    if task.shutdown_token.is_cancelled() {
        return;
    }

    run_scheduled_periodic_iteration(&task)
        .instrument(tracing::info_span!("bg_task", task.name = task.task_name))
        .await;

    loop {
        let sleep_duration =
            periodic_sleep_duration((task.interval_fn)(&task.state), task.jitter_cap);
        tokio::select! {
            biased;
            _ = task.shutdown_token.cancelled() => break,
            _ = tokio::time::sleep(sleep_duration) => {}
        }

        if task.shutdown_token.is_cancelled() {
            break;
        }

        run_scheduled_periodic_iteration(&task)
            .instrument(tracing::info_span!("bg_task", task.name = task.task_name))
            .await;
    }
}

async fn run_scheduled_periodic_iteration<
    Name,
    State,
    Store,
    IntervalFn,
    TaskFn,
    TaskFut,
    PanicFn,
    RecordFn,
    RecordFut,
    Outcome,
>(
    task: &ScheduledPeriodicTask<Name, State, Store, IntervalFn, TaskFn, PanicFn, RecordFn>,
) where
    Name: Copy + Send + 'static,
    State: Clone + Send + Sync + 'static,
    Store: ScheduledTaskStore,
    IntervalFn: Fn(&State) -> Duration + Send + Sync + 'static,
    TaskFn: Fn(State) -> TaskFut + Send + Sync + 'static,
    TaskFut: Future<Output = Outcome> + Send + 'static,
    PanicFn: Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn: Fn(State, Name, ScheduledTaskClaim, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut
        + Send
        + Sync
        + 'static,
    RecordFut: Future<Output = ()> + Send + 'static,
    Outcome: Send + 'static,
{
    let now = Utc::now();
    if let Err(error) = task
        .store
        .ensure_scheduled_task(ScheduledTaskCatalogEntry {
            namespace: task.namespace,
            task_name: task.task_name,
            display_name: task.display_name,
            first_run_at: now,
        })
        .await
    {
        tracing::warn!(
            task.name = task.task_name,
            error = %error,
            "failed to ensure scheduled task catalog row"
        );
        return;
    }

    let claim = match task
        .store
        .claim_scheduled_task(ScheduledTaskClaimRequest {
            namespace: task.namespace,
            task_name: task.task_name,
            owner_id: &task.owner_id,
            now,
            claim_ttl: task.claim_ttl,
        })
        .await
    {
        Ok(Some(claim)) => claim,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                task.name = task.task_name,
                error = %error,
                "failed to claim scheduled task"
            );
            return;
        }
    };

    let started_at = Utc::now();
    let outcome = match std::panic::AssertUnwindSafe((task.hooks.task_fn)(task.state.clone()))
        .catch_unwind()
        .await
    {
        Ok(outcome) => outcome,
        Err(panic) => {
            let panic_message = panic_payload_message(&panic);
            tracing::error!(
                task.name = task.task_name,
                "scheduled task panicked: {panic_message}"
            );
            (task.hooks.panic_outcome)(panic_message)
        }
    };
    let finished_at = Utc::now();

    let record_result = std::panic::AssertUnwindSafe((task.hooks.record_outcome)(
        task.state.clone(),
        task.name,
        claim.clone(),
        started_at,
        finished_at,
        outcome,
    ))
    .catch_unwind()
    .await;
    if let Err(panic) = record_result {
        let panic_message = panic_payload_message(&panic);
        tracing::error!(
            task.name = task.task_name,
            "scheduled task outcome recorder panicked: {panic_message}"
        );
        return;
    }

    let Some(next_run_at) = next_scheduled_run_at(finished_at, (task.interval_fn)(&task.state))
    else {
        tracing::warn!(
            task.name = task.task_name,
            "scheduled task interval overflowed while computing next run"
        );
        return;
    };

    match task
        .store
        .complete_scheduled_task(ScheduledTaskCompletion {
            claim,
            finished_at,
            next_run_at,
        })
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                task.name = task.task_name,
                "scheduled task claim was not completed because ownership changed"
            );
        }
        Err(error) => {
            tracing::warn!(
                task.name = task.task_name,
                error = %error,
                "failed to complete scheduled task claim"
            );
        }
    }
}

/// Computes the next run timestamp after a completed scheduled task firing.
pub fn next_scheduled_run_at(
    finished_at: DateTime<Utc>,
    interval: Duration,
) -> Option<DateTime<Utc>> {
    let interval = chrono::Duration::from_std(interval).ok()?;
    finished_at.checked_add_signed(interval)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use tokio_util::sync::CancellationToken;

    use super::{
        ScheduledPeriodicTask, ScheduledTaskCatalogEntry, ScheduledTaskClaim,
        ScheduledTaskClaimRequest, ScheduledTaskCompletion, ScheduledTaskStore,
        next_scheduled_run_at, run_scheduled_periodic_task,
    };
    use crate::RecordedTaskHooks;

    #[derive(Clone)]
    struct MemoryScheduleStore {
        calls: Arc<AtomicUsize>,
        completions: Arc<AtomicUsize>,
    }

    fn test_interval(_: &()) -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }

    #[async_trait]
    impl ScheduledTaskStore for MemoryScheduleStore {
        type Error = String;

        async fn ensure_scheduled_task(
            &self,
            entry: ScheduledTaskCatalogEntry<'_>,
        ) -> Result<(), Self::Error> {
            assert_eq!(entry.namespace, "aster_test");
            assert_eq!(entry.task_name, "cleanup");
            Ok(())
        }

        async fn claim_scheduled_task(
            &self,
            request: ScheduledTaskClaimRequest<'_>,
        ) -> Result<Option<ScheduledTaskClaim>, Self::Error> {
            if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
                return Ok(None);
            }
            Ok(Some(ScheduledTaskClaim {
                task_id: "aster_test:cleanup".to_string(),
                namespace: request.namespace.to_string(),
                task_name: request.task_name.to_string(),
                owner_id: request.owner_id.to_string(),
                scheduled_at: request.now,
                claimed_at: request.now,
                claim_expires_at: request.now,
            }))
        }

        async fn complete_scheduled_task(
            &self,
            completion: ScheduledTaskCompletion,
        ) -> Result<bool, Self::Error> {
            assert_eq!(completion.claim.task_name, "cleanup");
            assert!(completion.next_run_at >= completion.finished_at);
            self.completions.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
    }

    fn memory_store() -> MemoryScheduleStore {
        MemoryScheduleStore {
            calls: Arc::new(AtomicUsize::new(0)),
            completions: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[tokio::test]
    async fn scheduled_periodic_task_claims_records_and_completes_one_due_run() {
        let shutdown = CancellationToken::new();
        let ran = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(AtomicUsize::new(0));
        let store = memory_store();
        let completions = store.completions.clone();
        let ran_for_task = ran.clone();
        let recorded_for_hook = recorded.clone();
        let shutdown_for_hook = shutdown.clone();

        run_scheduled_periodic_task(ScheduledPeriodicTask {
            name: "cleanup",
            namespace: "aster_test",
            task_name: "cleanup",
            display_name: "Cleanup",
            owner_id: "runtime-a".to_string(),
            claim_ttl: std::time::Duration::from_secs(30),
            interval_fn: test_interval,
            jitter_cap: None,
            shutdown_token: shutdown.clone(),
            state: (),
            store,
            hooks: RecordedTaskHooks::new(
                move |()| {
                    let ran = ran_for_task.clone();
                    async move {
                        ran.fetch_add(1, Ordering::SeqCst);
                        "ok"
                    }
                },
                |_| "panic",
                move |(), _name, claim: ScheduledTaskClaim, _started_at, _finished_at, outcome| {
                    let recorded = recorded_for_hook.clone();
                    let shutdown = shutdown_for_hook.clone();
                    async move {
                        assert_eq!(claim.task_name, "cleanup");
                        assert_eq!(outcome, "ok");
                        recorded.fetch_add(1, Ordering::SeqCst);
                        shutdown.cancel();
                    }
                },
            ),
        })
        .await;

        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(recorded.load(Ordering::SeqCst), 1);
        assert_eq!(completions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn scheduled_periodic_task_records_panic_outcome_and_completes_claim() {
        let shutdown = CancellationToken::new();
        let store = memory_store();
        let completions = store.completions.clone();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorded_for_hook = recorded.clone();
        let shutdown_for_hook = shutdown.clone();

        run_scheduled_periodic_task(ScheduledPeriodicTask {
            name: "cleanup",
            namespace: "aster_test",
            task_name: "cleanup",
            display_name: "Cleanup",
            owner_id: "runtime-a".to_string(),
            claim_ttl: std::time::Duration::from_secs(30),
            interval_fn: test_interval,
            jitter_cap: None,
            shutdown_token: shutdown.clone(),
            state: (),
            store,
            hooks: RecordedTaskHooks::new(
                move |()| async move {
                    panic!("scheduled body failed");
                    #[allow(unreachable_code)]
                    "ok".to_string()
                },
                |message| format!("panic:{message}"),
                move |(), _name, _claim, _started_at, _finished_at, outcome| {
                    let recorded = recorded_for_hook.clone();
                    let shutdown = shutdown_for_hook.clone();
                    async move {
                        recorded
                            .lock()
                            .expect("recorded outcomes should lock")
                            .push(outcome);
                        shutdown.cancel();
                    }
                },
            ),
        })
        .await;

        assert_eq!(
            recorded
                .lock()
                .expect("recorded outcomes should lock")
                .as_slice(),
            ["panic:scheduled body failed"]
        );
        assert_eq!(completions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn scheduled_periodic_task_does_not_complete_claim_when_recorder_panics() {
        let shutdown = CancellationToken::new();
        let store = memory_store();
        let completions = store.completions.clone();

        run_scheduled_periodic_task(ScheduledPeriodicTask {
            name: "cleanup",
            namespace: "aster_test",
            task_name: "cleanup",
            display_name: "Cleanup",
            owner_id: "runtime-a".to_string(),
            claim_ttl: std::time::Duration::from_secs(30),
            interval_fn: test_interval,
            jitter_cap: None,
            shutdown_token: shutdown.clone(),
            state: (),
            store,
            hooks: RecordedTaskHooks::new(
                move |()| {
                    let shutdown = shutdown.clone();
                    async move {
                        shutdown.cancel();
                        "ok"
                    }
                },
                |_| "panic",
                move |(), _name, _claim, _started_at, _finished_at, _outcome| async move {
                    panic!("record failed");
                },
            ),
        })
        .await;

        assert_eq!(completions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn next_scheduled_run_at_adds_interval() {
        let finished_at = Utc.with_ymd_and_hms(2026, 6, 26, 1, 2, 3).unwrap();
        assert_eq!(
            next_scheduled_run_at(finished_at, std::time::Duration::from_secs(60)),
            Some(Utc.with_ymd_and_hms(2026, 6, 26, 1, 3, 3).unwrap())
        );
    }
}

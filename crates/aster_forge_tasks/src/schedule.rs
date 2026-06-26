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
use crate::{RecordedTaskHooks, periodic_sleep_duration};

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
    TaskFut: Future<Output = Outcome> + Send,
    PanicFn: Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn: Fn(State, Name, ScheduledTaskClaim, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut
        + Send
        + Sync
        + 'static,
    RecordFut: Future<Output = ()> + Send,
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
    TaskFut: Future<Output = Outcome> + Send,
    PanicFn: Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn: Fn(State, Name, ScheduledTaskClaim, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut
        + Send
        + Sync
        + 'static,
    RecordFut: Future<Output = ()> + Send,
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

    (task.hooks.record_outcome)(
        task.state.clone(),
        task.name,
        claim.clone(),
        started_at,
        finished_at,
        outcome,
    )
    .await;

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
            Ok(true)
        }
    }

    #[tokio::test]
    async fn scheduled_periodic_task_claims_records_and_completes_one_due_run() {
        let shutdown = CancellationToken::new();
        let ran = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(AtomicUsize::new(0));
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
            store: MemoryScheduleStore {
                calls: Arc::new(AtomicUsize::new(0)),
            },
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

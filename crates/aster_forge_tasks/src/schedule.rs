//! Scheduled runtime task catalog and runner primitives.
//!
//! A scheduled task is a product-owned runtime job with a stable name and interval. Forge keeps
//! the reusable coordination contract here: products register catalog entries, a store atomically
//! claims due firings, and the runner records one panic-protected execution before advancing the
//! next due timestamp. Concrete persistence is supplied by another crate, typically
//! `aster_forge_db`.

use std::future::Future;
use std::marker::PhantomData;
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

/// Renewal update for a claim whose task body is still running.
///
/// The store must apply the same ownership predicate as completion (task id,
/// owner id, and claim acquisition timestamp), so a renewal can never revive a
/// claim another runtime has already reclaimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledTaskClaimRenewal<'a> {
    /// Owned claim to renew.
    pub claim: &'a ScheduledTaskClaim,
    /// Renewal timestamp.
    pub now: DateTime<Utc>,
    /// Fresh claim TTL applied from `now`.
    pub claim_ttl: Duration,
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

    /// Renews an owned claim while the task body is still running.
    ///
    /// Returning `Ok(false)` means the ownership predicate did not match, so
    /// the worker must treat the claim as lost and stop renewing. Returning
    /// `Err(_)` is treated as transient and retried on the next renewal tick.
    async fn renew_scheduled_task_claim(
        &self,
        renewal: ScheduledTaskClaimRenewal<'_>,
    ) -> std::result::Result<bool, Self::Error>;

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

/// Configuration for a leased group of scheduled runtime tasks.
///
/// This is the high-level entrypoint for multi-instance Aster services. Forge
/// generates the process owner id, supervises the runtime lease, creates the
/// lease-scoped [`BackgroundTasks`] group, and wires every declared scheduled
/// task into the shared catalog store. Product code only declares singleton
/// workers and scheduled task bodies through [`ScheduledRuntimeTaskGroup`].
#[derive(Clone)]
pub struct LeasedScheduledRuntimeConfig<
    Name,
    Outcome,
    State,
    LeaseStore,
    ScheduleStore,
    PanicFn,
    RecordFn,
> {
    namespace: &'static str,
    lease_id: String,
    lease_store: LeaseStore,
    schedule_store: ScheduleStore,
    claim_ttl: Duration,
    lease_ttl: Duration,
    lease_renew_interval: Duration,
    lease_standby_retry_interval: Duration,
    state: State,
    panic_outcome: PanicFn,
    record_outcome: RecordFn,
    _name: PhantomData<fn() -> Name>,
    _outcome: PhantomData<fn() -> Outcome>,
}

impl<Name, Outcome, State, LeaseStore, ScheduleStore, PanicFn, RecordFn>
    LeasedScheduledRuntimeConfig<Name, Outcome, State, LeaseStore, ScheduleStore, PanicFn, RecordFn>
{
    /// Creates configuration for one leased scheduled runtime task group.
    pub fn new<RecordFut>(
        namespace: &'static str,
        lease_id: impl Into<String>,
        lease_store: LeaseStore,
        schedule_store: ScheduleStore,
        state: State,
        panic_outcome: PanicFn,
        record_outcome: RecordFn,
    ) -> Self
    where
        PanicFn: Fn(String) -> Outcome,
        RecordFn:
            Fn(State, Name, ScheduledTaskClaim, DateTime<Utc>, DateTime<Utc>, Outcome) -> RecordFut,
        RecordFut: Future<Output = ()> + Send + 'static,
    {
        Self {
            namespace,
            lease_id: lease_id.into(),
            lease_store,
            schedule_store,
            claim_ttl: Duration::from_secs(120),
            lease_ttl: aster_forge_runtime::DEFAULT_RUNTIME_LEASE_TTL,
            lease_renew_interval: Duration::from_secs(10),
            lease_standby_retry_interval: aster_forge_runtime::DEFAULT_RUNTIME_LEASE_RETRY_INTERVAL,
            state,
            panic_outcome,
            record_outcome,
            _name: PhantomData,
            _outcome: PhantomData,
        }
    }

    /// Sets the scheduled task claim TTL.
    pub const fn claim_ttl(mut self, claim_ttl: Duration) -> Self {
        self.claim_ttl = claim_ttl;
        self
    }

    /// Sets the runtime lease TTL.
    pub const fn lease_ttl(mut self, lease_ttl: Duration) -> Self {
        self.lease_ttl = lease_ttl;
        self
    }

    /// Sets the runtime lease renewal interval for the active owner.
    pub const fn lease_renew_interval(mut self, lease_renew_interval: Duration) -> Self {
        self.lease_renew_interval = lease_renew_interval;
        self
    }

    /// Sets the standby retry interval while another process owns the lease.
    pub const fn lease_standby_retry_interval(
        mut self,
        lease_standby_retry_interval: Duration,
    ) -> Self {
        self.lease_standby_retry_interval = lease_standby_retry_interval;
        self
    }

    /// Runs this configured leased scheduled runtime group until shutdown.
    ///
    /// Prefer this method at product entrypoints because it keeps the call
    /// shaped like a component declaration: configure shared resources once,
    /// then declare workers and scheduled tasks in the closure.
    pub async fn run<ConfigureFn>(self, shutdown_token: CancellationToken, configure: ConfigureFn)
    where
        Name: RegisteredRuntimeTaskKind + Send + Sync + 'static,
        State: Clone + Send + Sync + 'static,
        LeaseStore: aster_forge_runtime::RuntimeLeaseStore,
        ScheduleStore: ScheduledTaskStore,
        ConfigureFn: for<'a> FnMut(
                &mut ScheduledRuntimeTaskGroup<
                    'a,
                    Name,
                    State,
                    ScheduleStore,
                    PanicFn,
                    RecordFn,
                    Outcome,
                >,
            ) + Send
            + 'static,
        PanicFn: Clone + Fn(String) -> Outcome + Send + Sync + 'static,
        RecordFn: Clone + Send + Sync + 'static,
        Outcome: Send + 'static,
    {
        run_leased_scheduled_runtime_tasks(self, shutdown_token, configure).await;
    }

    fn into_parts(
        self,
    ) -> LeasedScheduledRuntimeParts<State, LeaseStore, ScheduleStore, PanicFn, RecordFn> {
        let owner_id = aster_forge_runtime::new_runtime_lease_owner_id();
        let lease_config =
            aster_forge_runtime::RuntimeLeaseConfig::new(self.lease_id, owner_id.clone())
                .ttl(self.lease_ttl)
                .renew_interval(self.lease_renew_interval)
                .standby_retry_interval(self.lease_standby_retry_interval);
        LeasedScheduledRuntimeParts {
            namespace: self.namespace,
            owner_id,
            lease_store: self.lease_store,
            schedule_store: self.schedule_store,
            claim_ttl: self.claim_ttl,
            state: self.state,
            panic_outcome: self.panic_outcome,
            record_outcome: self.record_outcome,
            lease_config,
        }
    }
}

struct LeasedScheduledRuntimeParts<State, LeaseStore, ScheduleStore, PanicFn, RecordFn> {
    namespace: &'static str,
    owner_id: String,
    lease_store: LeaseStore,
    schedule_store: ScheduleStore,
    claim_ttl: Duration,
    state: State,
    panic_outcome: PanicFn,
    record_outcome: RecordFn,
    lease_config: aster_forge_runtime::RuntimeLeaseConfig,
}

/// Lease-scoped task group used by product registration closures.
///
/// A value of this type exists only while Forge is building the worker group
/// for one lease acquisition. Use [`Self::worker`] for singleton workers that
/// should run only on the active owner, and [`Self::scheduled`] for tasks that
/// should additionally coordinate each firing through the scheduled task
/// catalog.
pub struct ScheduledRuntimeTaskGroup<'a, Name, State, Store, PanicFn, RecordFn, Outcome> {
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
    ScheduledRuntimeTaskGroup<'a, Name, State, Store, PanicFn, RecordFn, Outcome>
where
    State: Clone + Send + Sync + 'static,
{
    /// Spawns one lease-scoped singleton worker into this group.
    pub fn worker<WorkerFn, WorkerFut>(&mut self, worker: WorkerFn)
    where
        WorkerFn: FnOnce(CancellationToken, State) -> WorkerFut,
        WorkerFut: Future<Output = ()> + Send + 'static,
    {
        self.tasks
            .push(worker(self.shutdown_token.clone(), self.state.clone()));
    }

    /// Returns a clone of the lease-scoped shutdown token.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Returns a clone of the product runtime state.
    pub fn state(&self) -> State {
        self.state.clone()
    }
}

impl<'a, Name, State, Store, PanicFn, RecordFn, Outcome>
    ScheduledRuntimeTaskGroup<'a, Name, State, Store, PanicFn, RecordFn, Outcome>
where
    Name: RegisteredRuntimeTaskKind + Send + Sync + 'static,
    State: Clone + Send + Sync + 'static,
    Store: ScheduledTaskStore,
    PanicFn: Clone + Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn: Clone + Send + Sync + 'static,
    Outcome: Send + 'static,
{
    /// Registers one scheduled runtime task in the lease-scoped worker group.
    pub fn scheduled<IntervalFn, TaskFn, TaskFut, RecordFut>(
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

/// Runs a lease-supervised scheduled runtime task group until shutdown.
///
/// Forge owns the lifecycle glue: owner id generation, runtime lease
/// supervision, lease-scoped shutdown token creation, scheduled task catalog
/// registration, and graceful worker shutdown. Product code supplies the
/// runtime config and a closure that declares workers and scheduled tasks.
async fn run_leased_scheduled_runtime_tasks<
    Name,
    Outcome,
    State,
    LeaseStore,
    ScheduleStore,
    ConfigureFn,
    PanicFn,
    RecordFn,
>(
    config: LeasedScheduledRuntimeConfig<
        Name,
        Outcome,
        State,
        LeaseStore,
        ScheduleStore,
        PanicFn,
        RecordFn,
    >,
    shutdown_token: CancellationToken,
    mut configure: ConfigureFn,
) where
    Name: RegisteredRuntimeTaskKind + Send + Sync + 'static,
    State: Clone + Send + Sync + 'static,
    LeaseStore: aster_forge_runtime::RuntimeLeaseStore,
    ScheduleStore: ScheduledTaskStore,
    ConfigureFn: for<'a> FnMut(
            &mut ScheduledRuntimeTaskGroup<
                'a,
                Name,
                State,
                ScheduleStore,
                PanicFn,
                RecordFn,
                Outcome,
            >,
        ) + Send
        + 'static,
    PanicFn: Clone + Fn(String) -> Outcome + Send + Sync + 'static,
    RecordFn: Clone + Send + Sync + 'static,
    Outcome: Send + 'static,
{
    let parts = config.into_parts();
    let LeasedScheduledRuntimeParts {
        namespace,
        owner_id,
        lease_store,
        schedule_store,
        claim_ttl,
        state,
        panic_outcome,
        record_outcome,
        lease_config,
    } = parts;

    aster_forge_runtime::run_runtime_lease_supervisor(
        lease_store,
        lease_config,
        shutdown_token,
        move |leased_shutdown_token| {
            let mut tasks = BackgroundTasks::with_shutdown_token(leased_shutdown_token);
            let group_shutdown_token = tasks.shutdown_token();
            let mut group = ScheduledRuntimeTaskGroup {
                tasks: &mut tasks,
                namespace,
                owner_id: owner_id.clone(),
                claim_ttl,
                shutdown_token: group_shutdown_token,
                state: state.clone(),
                store: schedule_store.clone(),
                panic_outcome: panic_outcome.clone(),
                record_outcome: record_outcome.clone(),
                _name: std::marker::PhantomData,
                _outcome: std::marker::PhantomData,
            };
            configure(&mut group);
            tasks
        },
        |background_tasks| async move {
            background_tasks.shutdown().await;
        },
    )
    .await;
}

/// Runs a scheduled periodic task until shutdown.
///
/// Unlike [`crate::run_periodic_task`], this runner first claims a due catalog row. If the row is
/// not due, or another process owns a fresh claim, the worker skips that iteration. Successful and
/// failed task outcomes both complete the claim and advance `next_run_at`; crashes and process
/// exits before completion are recovered by claim expiry.
///
/// While the task body runs, a renewal loop extends the claim at
/// [`scheduled_claim_renew_interval`] ticks, so a task that outlives `claim_ttl` is not reclaimed
/// and executed twice by another runtime. Renewal failures never abort the task body: a lost
/// claim only stops the renewal loop, and completion still guards on ownership.
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

/// Derives the claim renewal tick from the claim TTL.
///
/// Renewing three times per TTL window means two consecutive missed ticks still
/// leave one renewal before expiry. The floor keeps `tokio::time::interval`
/// away from a zero period for pathological TTLs.
pub fn scheduled_claim_renew_interval(claim_ttl: Duration) -> Duration {
    (claim_ttl / 3).max(Duration::from_millis(10))
}

/// Renews one owned scheduled task claim until stopped or the claim is lost.
///
/// Mirrors the background task heartbeat loop: `Ok(false)` means the ownership
/// predicate no longer matches (another runtime reclaimed the firing), so the
/// loop stops; `Err(_)` is logged and retried on the next tick. The loop never
/// aborts the running task body — completion still guards on ownership.
pub async fn run_scheduled_claim_renewal_loop<Store>(
    store: Store,
    claim: ScheduledTaskClaim,
    claim_ttl: Duration,
    interval: Duration,
    stop_token: CancellationToken,
) where
    Store: ScheduledTaskStore,
{
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = stop_token.cancelled() => return,
            _ = ticker.tick() => {
                let renewal = ScheduledTaskClaimRenewal {
                    claim: &claim,
                    now: Utc::now(),
                    claim_ttl,
                };
                let result = tokio::select! {
                    _ = stop_token.cancelled() => return,
                    result = store.renew_scheduled_task_claim(renewal) => result,
                };

                match result {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(
                            task.name = claim.task_name,
                            "scheduled task claim lost; stopping claim renewal"
                        );
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(
                            task.name = claim.task_name,
                            error = %error,
                            "scheduled task claim renewal failed; retrying next tick"
                        );
                    }
                }
            }
        }
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

    let renewal_stop = task.shutdown_token.child_token();
    let renewal_handle = tokio::spawn(run_scheduled_claim_renewal_loop(
        task.store.clone(),
        claim.clone(),
        task.claim_ttl,
        scheduled_claim_renew_interval(task.claim_ttl),
        renewal_stop.clone(),
    ));

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

    renewal_stop.cancel();
    if let Err(error) = renewal_handle.await {
        tracing::warn!(
            task.name = task.task_name,
            error = %error,
            "scheduled task claim renewal worker stopped unexpectedly"
        );
    }

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
        LeasedScheduledRuntimeConfig, ScheduledPeriodicTask, ScheduledRuntimeTaskGroup,
        ScheduledTaskCatalogEntry, ScheduledTaskClaim, ScheduledTaskClaimRequest,
        ScheduledTaskCompletion, ScheduledTaskStore, next_scheduled_run_at,
        run_scheduled_claim_renewal_loop, run_scheduled_periodic_task,
        scheduled_claim_renew_interval,
    };
    use crate::{RecordedTaskHooks, RegisteredRuntimeTaskKind};

    #[derive(Clone)]
    struct MemoryScheduleStore {
        calls: Arc<AtomicUsize>,
        completions: Arc<AtomicUsize>,
        renewals: Arc<AtomicUsize>,
        renewal_script: Arc<Mutex<std::collections::VecDeque<Result<bool, String>>>>,
    }

    fn test_interval(_: &()) -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }

    #[derive(Clone)]
    struct AlwaysAcquireLeaseStore {
        acquired: Arc<AtomicUsize>,
        released: Arc<AtomicUsize>,
    }

    impl AlwaysAcquireLeaseStore {
        fn new() -> Self {
            Self {
                acquired: Arc::new(AtomicUsize::new(0)),
                released: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl aster_forge_runtime::RuntimeLeaseStore for AlwaysAcquireLeaseStore {
        type Error = String;

        async fn try_acquire(
            &self,
            _claim: aster_forge_runtime::RuntimeLeaseClaim<'_>,
        ) -> Result<aster_forge_runtime::RuntimeLeaseAcquire, Self::Error> {
            self.acquired.fetch_add(1, Ordering::SeqCst);
            Ok(aster_forge_runtime::RuntimeLeaseAcquire::Acquired)
        }

        async fn renew(
            &self,
            _lease_id: &str,
            _owner_id: &str,
            _now: chrono::DateTime<Utc>,
            _expires_at: chrono::DateTime<Utc>,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }

        async fn release(&self, _lease_id: &str, _owner_id: &str) -> Result<(), Self::Error> {
            self.released.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestRuntimeTask {
        Cleanup,
    }

    impl RegisteredRuntimeTaskKind for TestRuntimeTask {
        fn as_str(self) -> &'static str {
            "cleanup"
        }

        fn display_name(self) -> &'static str {
            "Cleanup"
        }

        fn from_wire_value(value: &str) -> Option<Self> {
            (value == "cleanup").then_some(Self::Cleanup)
        }
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

        async fn renew_scheduled_task_claim(
            &self,
            renewal: super::ScheduledTaskClaimRenewal<'_>,
        ) -> Result<bool, Self::Error> {
            assert_eq!(renewal.claim.task_name, "cleanup");
            assert!(!renewal.claim_ttl.is_zero());
            self.renewals.fetch_add(1, Ordering::SeqCst);
            let scripted = self
                .renewal_script
                .lock()
                .expect("renewal script should lock")
                .pop_front();
            scripted.unwrap_or(Ok(true))
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
            renewals: Arc::new(AtomicUsize::new(0)),
            renewal_script: Arc::new(Mutex::new(std::collections::VecDeque::new())),
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
                move |(),
                      _name: &str,
                      claim: ScheduledTaskClaim,
                      _started_at,
                      _finished_at,
                      outcome| {
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

    #[tokio::test]
    async fn leased_scheduled_runtime_group_runs_worker_and_scheduled_task() {
        let lease_store = AlwaysAcquireLeaseStore::new();
        let acquired = lease_store.acquired.clone();
        let released = lease_store.released.clone();
        let schedule_store = memory_store();
        let completions = schedule_store.completions.clone();
        let worker_runs = Arc::new(AtomicUsize::new(0));
        let scheduled_runs = Arc::new(AtomicUsize::new(0));
        let recorded_runs = Arc::new(AtomicUsize::new(0));
        let shutdown = CancellationToken::new();
        let config = LeasedScheduledRuntimeConfig::new(
            "aster_test",
            "aster_test.background",
            lease_store,
            schedule_store,
            (),
            |_| "panic",
            {
                let recorded_runs = recorded_runs.clone();
                let shutdown = shutdown.clone();
                move |(), _name, claim: ScheduledTaskClaim, _started_at, _finished_at, outcome| {
                    let recorded_runs = recorded_runs.clone();
                    let shutdown = shutdown.clone();
                    async move {
                        assert_eq!(claim.task_name, "cleanup");
                        assert_eq!(outcome, "ok");
                        recorded_runs.fetch_add(1, Ordering::SeqCst);
                        shutdown.cancel();
                    }
                }
            },
        )
        .claim_ttl(std::time::Duration::from_secs(30))
        .lease_ttl(std::time::Duration::from_secs(30))
        .lease_renew_interval(std::time::Duration::from_secs(10))
        .lease_standby_retry_interval(std::time::Duration::from_secs(5));
        let worker_runs_for_group = worker_runs.clone();
        let scheduled_runs_for_group = scheduled_runs.clone();

        config
            .run(
                shutdown.clone(),
                move |group: &mut ScheduledRuntimeTaskGroup<
                    '_,
                    TestRuntimeTask,
                    (),
                    _,
                    _,
                    _,
                    &'static str,
                >| {
                    let worker_runs = worker_runs_for_group.clone();
                    group.worker(move |shutdown_token, ()| async move {
                        worker_runs.fetch_add(1, Ordering::SeqCst);
                        shutdown_token.cancelled().await;
                    });
                    let scheduled_runs = scheduled_runs_for_group.clone();
                    group.scheduled(TestRuntimeTask::Cleanup, test_interval, None, move |()| {
                        let scheduled_runs = scheduled_runs.clone();
                        async move {
                            scheduled_runs.fetch_add(1, Ordering::SeqCst);
                            "ok"
                        }
                    });
                },
            )
            .await;

        assert_eq!(acquired.load(Ordering::SeqCst), 1);
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(worker_runs.load(Ordering::SeqCst), 1);
        assert_eq!(scheduled_runs.load(Ordering::SeqCst), 1);
        assert_eq!(recorded_runs.load(Ordering::SeqCst), 1);
        assert_eq!(completions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn next_scheduled_run_at_adds_interval() {
        let finished_at = Utc.with_ymd_and_hms(2026, 6, 26, 1, 2, 3).unwrap();
        assert_eq!(
            next_scheduled_run_at(finished_at, std::time::Duration::from_secs(60)),
            Some(Utc.with_ymd_and_hms(2026, 6, 26, 1, 3, 3).unwrap())
        );
    }

    fn test_claim() -> ScheduledTaskClaim {
        let now = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
        ScheduledTaskClaim {
            task_id: "aster_test:cleanup".to_string(),
            namespace: "aster_test".to_string(),
            task_name: "cleanup".to_string(),
            owner_id: "runtime-a".to_string(),
            scheduled_at: now,
            claimed_at: now,
            claim_expires_at: now,
        }
    }

    #[test]
    fn claim_renew_interval_is_one_third_of_ttl_with_floor() {
        assert_eq!(
            scheduled_claim_renew_interval(std::time::Duration::from_secs(120)),
            std::time::Duration::from_secs(40)
        );
        assert_eq!(
            scheduled_claim_renew_interval(std::time::Duration::from_millis(30)),
            std::time::Duration::from_millis(10)
        );
        // A zero TTL never reaches the store, but the interval must not panic.
        assert_eq!(
            scheduled_claim_renew_interval(std::time::Duration::ZERO),
            std::time::Duration::from_millis(10)
        );
    }

    #[tokio::test]
    async fn claim_renewal_loop_renews_until_stopped() {
        let store = memory_store();
        let renewals = store.renewals.clone();
        let stop = CancellationToken::new();

        let handle = tokio::spawn(run_scheduled_claim_renewal_loop(
            store,
            test_claim(),
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(1),
            stop.clone(),
        ));

        while renewals.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        stop.cancel();
        handle.await.expect("renewal loop should stop cleanly");
        assert!(renewals.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn claim_renewal_loop_stops_when_claim_is_lost() {
        let store = memory_store();
        store
            .renewal_script
            .lock()
            .expect("renewal script should lock")
            .push_back(Ok(false));
        let renewals = store.renewals.clone();

        // Ok(false) means the ownership predicate no longer matches: the loop
        // must stop on its own instead of hammering a reclaimed row.
        run_scheduled_claim_renewal_loop(
            store,
            test_claim(),
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(1),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(renewals.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn claim_renewal_loop_retries_after_transient_errors() {
        let store = memory_store();
        {
            let mut script = store
                .renewal_script
                .lock()
                .expect("renewal script should lock");
            script.push_back(Err("database temporarily unavailable".to_string()));
            script.push_back(Err("database temporarily unavailable".to_string()));
        }
        let renewals = store.renewals.clone();
        let stop = CancellationToken::new();

        let handle = tokio::spawn(run_scheduled_claim_renewal_loop(
            store,
            test_claim(),
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(1),
            stop.clone(),
        ));

        while renewals.load(Ordering::SeqCst) < 3 {
            tokio::task::yield_now().await;
        }
        stop.cancel();
        handle.await.expect("renewal loop should stop cleanly");
        assert!(renewals.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn scheduled_periodic_task_renews_claim_while_task_body_runs() {
        let shutdown = CancellationToken::new();
        let store = memory_store();
        let renewals = store.renewals.clone();
        let completions = store.completions.clone();
        let renewals_for_task = renewals.clone();
        let shutdown_for_hook = shutdown.clone();

        run_scheduled_periodic_task(ScheduledPeriodicTask {
            name: "cleanup",
            namespace: "aster_test",
            task_name: "cleanup",
            display_name: "Cleanup",
            owner_id: "runtime-a".to_string(),
            // 30ms TTL derives a 10ms renewal interval, so a body that waits for
            // two renewals provably outlives the original claim window.
            claim_ttl: std::time::Duration::from_millis(30),
            interval_fn: test_interval,
            jitter_cap: None,
            shutdown_token: shutdown.clone(),
            state: (),
            store,
            hooks: RecordedTaskHooks::new(
                move |()| {
                    let renewals = renewals_for_task.clone();
                    async move {
                        tokio::time::timeout(std::time::Duration::from_secs(5), async move {
                            while renewals.load(Ordering::SeqCst) < 2 {
                                tokio::task::yield_now().await;
                            }
                        })
                        .await
                        .expect("claim should be renewed while the task body runs");
                        "ok"
                    }
                },
                |_| "panic",
                move |(), _name, _claim, _started_at, _finished_at, _outcome| {
                    let shutdown = shutdown_for_hook.clone();
                    async move {
                        shutdown.cancel();
                    }
                },
            ),
        })
        .await;

        assert_eq!(completions.load(Ordering::SeqCst), 1);
        let renewed = renewals.load(Ordering::SeqCst);
        assert!(renewed >= 2);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            renewals.load(Ordering::SeqCst),
            renewed,
            "renewal loop must stop before the claim is completed"
        );
    }
}

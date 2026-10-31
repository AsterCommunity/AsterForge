//! Shared dispatcher mechanics and result counters.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};

use crate::{TaskCoreError, TaskLease, TaskRecord, task_lease_expires_at};

/// Product-owned dispatch lane configuration.
///
/// The lane enum and task kind enum stay in product crates, but the scheduler shape is shared:
/// every lane has a process-local concurrency limit, a fixed list of task kinds and an optional
/// shared quota. Product stores own the locking strategy used to atomically enforce that quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskLaneConfig<Kind: 'static, Lane> {
    /// Product lane identifier.
    pub lane: Lane,
    /// Product task kinds assigned to this lane.
    pub kinds: &'static [Kind],
    /// Maximum active tasks for this lane in one consumer process.
    pub limit: usize,
    /// Optional shared limit, atomically rechecked by the product store while claiming.
    pub cluster_limit: Option<usize>,
}

/// Candidate task selected for compare-and-swap claiming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskClaimCandidate {
    /// Index of the task in the originally listed due-task batch.
    pub index: usize,
    /// Persisted task identifier.
    pub task_id: i64,
    /// Processing token that must still be present for the claim to succeed.
    pub expected_processing_token: i64,
    /// Processing token to store when the claim succeeds.
    pub next_processing_token: i64,
}

/// Successfully claimed task metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimedTask {
    /// Index of the task in the originally listed due-task batch.
    pub index: usize,
    /// Persisted task identifier.
    pub task_id: i64,
    /// Processing token assigned to the successful claim.
    pub processing_token: i64,
}

/// Minimal task row view required by generic claim logic.
pub trait ClaimableTaskRecord<Kind>: TaskRecord<Kind> {
    /// Current processing token stored on the task row.
    fn processing_token(&self) -> i64;
}

/// One atomic acquisition request. Stores select due rows and perform fenced updates in the
/// same transaction; database adapters may use `FOR UPDATE SKIP LOCKED`.
#[derive(Debug, Clone, Copy)]
pub struct TaskClaimRequest<Kind: 'static, Lane> {
    pub lane: TaskLaneConfig<Kind, Lane>,
    pub max_claim: usize,
    pub now: DateTime<Utc>,
    pub stale_before: DateTime<Utc>,
    pub lease_expires_at: DateTime<Utc>,
    processing_stale_secs: i64,
}

impl<Kind: Copy, Lane: Copy> TaskClaimRequest<Kind, Lane> {
    /// Refreshes timing after waiting for an authoritative quota lock.
    #[must_use]
    pub fn at(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self.stale_before = now
            .checked_sub_signed(
                chrono::Duration::try_seconds(self.processing_stale_secs.max(1))
                    .unwrap_or(chrono::Duration::MAX),
            )
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        self.lease_expires_at = task_lease_expires_at(now, self.processing_stale_secs);
        self
    }
}

/// Product storage adapter for an atomic, capacity-bounded claim. Products own ORM rows, SQL and
/// shared quota authority; Forge owns request timing, token arithmetic and result validation.
#[async_trait::async_trait]
pub trait TaskClaimStore<Task, Kind: 'static, Lane>: Sync {
    type Error: From<TaskCoreError> + Send;
    /// Selects and claims at most `request.max_claim` rows atomically. Returns each pre-claim
    /// snapshot with its newly assigned lease. Shared limits must be rechecked inside this unit.
    async fn claim(
        &self,
        request: TaskClaimRequest<Kind, Lane>,
    ) -> std::result::Result<Vec<(Task, TaskLease)>, Self::Error>;
}

/// Converts locked row snapshots into checked compare-and-swap candidates.
/// # Errors
/// Returns an invalid-value error on a processing-token overflow.
pub fn task_claim_candidates<Task, Kind>(tasks: &[Task]) -> crate::Result<Vec<TaskClaimCandidate>>
where
    Task: ClaimableTaskRecord<Kind>,
{
    tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            Ok(TaskClaimCandidate {
                index,
                task_id: task.id(),
                expected_processing_token: task.processing_token(),
                next_processing_token: task.processing_token().checked_add(1).ok_or_else(|| {
                    TaskCoreError::invalid_value("background task processing token overflow")
                })?,
            })
        })
        .collect()
}

/// Claims due tasks while keeping selection and fenced updates inside the product store.
/// # Errors
/// Returns storage errors or rejects an invalid/duplicate/over-capacity store result.
pub async fn claim_due_for_lane<Store, Task, Kind, Lane, LaneFn>(
    store: &Store,
    lane_config: TaskLaneConfig<Kind, Lane>,
    available_slots: usize,
    processing_stale_secs: i64,
    task_lane: LaneFn,
) -> std::result::Result<Vec<(Task, TaskLease)>, Store::Error>
where
    Store: TaskClaimStore<Task, Kind, Lane>,
    Task: ClaimableTaskRecord<Kind> + Send + Sync,
    Kind: Copy + Eq + std::fmt::Debug + std::fmt::Display + Send + Sync + 'static,
    Lane: Copy + Eq + std::fmt::Debug + Send + Sync + 'static,
    LaneFn: Fn(Kind) -> Lane,
{
    let max_claim = available_slots.min(lane_config.limit);
    if max_claim == 0 {
        return Ok(Vec::new());
    }
    let now = Utc::now();
    let request = TaskClaimRequest {
        lane: lane_config,
        max_claim,
        now,
        stale_before: now,
        lease_expires_at: now,
        processing_stale_secs,
    }
    .at(now);
    let claimed = store.claim(request).await?;
    if claimed.len() > max_claim {
        return Err(TaskCoreError::invalid_value("store exceeded local claim capacity").into());
    }
    let mut seen = std::collections::HashSet::new();
    for (task, lease) in &claimed {
        if task_lane(task.kind()) != lane_config.lane
            || !lane_config.kinds.contains(&task.kind())
            || lease.task_id != task.id()
            || task.processing_token().checked_add(1) != Some(lease.processing_token)
            || !seen.insert(task.id())
        {
            return Err(TaskCoreError::invalid_value(
                "store returned an invalid or duplicate claim",
            )
            .into());
        }
    }
    Ok(claimed)
}

/// Returns remaining lane capacity after subtracting currently active tasks.
#[must_use]
pub fn available_lane_capacity(limit: usize, active: u64) -> usize {
    let active = usize::try_from(active).unwrap_or(usize::MAX);
    limit.saturating_sub(active)
}

/// Converts a lane limit into the `u64` shape used by database query limits.
#[must_use]
pub fn claim_limit_to_u64(limit: usize) -> u64 {
    u64::try_from(limit).unwrap_or(u64::MAX)
}

/// Runs item handlers with a bounded number of in-flight futures.
///
/// Claimed task batches are already capacity-checked by lane claiming. This helper only preserves
/// the product's requested upper bound while letting independent task futures finish out of order.
pub async fn run_with_concurrency_limit<T, O, F, Fut>(
    items: Vec<T>,
    limit: usize,
    handler: F,
) -> Vec<O>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = O>,
{
    stream::iter(items.into_iter().map(handler))
        .buffer_unordered(limit.max(1))
        .collect()
        .await
}

/// Runs lane dispatchers concurrently and aggregates lane statistics.
///
/// If one or more lanes fail, the first error is returned after all lanes have completed. This
/// mirrors the existing Yggdrasil and Drive behavior: independent lanes are allowed to finish, then
/// the dispatch pass reports the first lane failure to the caller.
///
/// # Errors
///
/// Returns the dispatcher error from the first lane that does not complete successfully.
pub async fn dispatch_lanes<Kind, Lane, Error, F, Fut>(
    lane_configs: Vec<TaskLaneConfig<Kind, Lane>>,
    lane_parallelism: usize,
    dispatch_lane: F,
) -> std::result::Result<DispatchStats, Error>
where
    Kind: Send + Sync + 'static,
    Lane: Send + Sync,
    F: FnMut(TaskLaneConfig<Kind, Lane>) -> Fut,
    Fut: Future<Output = std::result::Result<DispatchStats, Error>>,
{
    let lane_results = stream::iter(lane_configs.into_iter().map(dispatch_lane))
        .buffer_unordered(lane_parallelism.max(1))
        .collect::<Vec<_>>()
        .await;
    let mut stats = DispatchStats::default();
    let mut first_error = None;

    for result in lane_results {
        match result {
            Ok(lane_stats) => stats.add(lane_stats),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    if let Some(first_error) = first_error {
        return Err(first_error);
    }

    Ok(stats)
}

/// Runs an already claimed task batch and aggregates task execution outcomes.
///
/// The batch is sorted before execution so products can preserve stable created-at/id ordering
/// while still allowing individual task futures to finish out of order.
///
/// # Errors
///
/// Returns the handler error from a claimed task that does not complete successfully.
pub async fn run_claimed_task_batch<T, SortKey, Error, SortFn, HandlerFn, HandlerFut>(
    mut claimed_tasks: Vec<T>,
    sort_key: SortFn,
    handler: HandlerFn,
) -> std::result::Result<DispatchStats, Error>
where
    SortKey: Ord,
    SortFn: FnMut(&T) -> SortKey,
    HandlerFn: FnMut(T) -> HandlerFut,
    HandlerFut: Future<Output = std::result::Result<TaskDispatchOutcome, Error>>,
{
    let concurrency = claimed_tasks.len().max(1);
    claimed_tasks.sort_by_key(sort_key);

    let results = run_with_concurrency_limit(claimed_tasks, concurrency, handler).await;
    let mut stats = DispatchStats::default();
    let mut first_error = None;

    for result in results {
        match result {
            Ok(outcome) => stats.add_outcome(outcome),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    if let Some(first_error) = first_error {
        return Err(first_error);
    }

    Ok(stats)
}

/// Drains a dispatcher until it has no claimed work and no active processing tasks.
///
/// Product crates provide the real dispatch function and processing-count query. The helper keeps
/// the shared bounded retry loop and aggregate statistics in Forge while leaving storage and status
/// semantics outside this crate.
///
/// # Errors
///
/// Returns an error when dispatching or counting remaining work fails.
pub async fn drain_dispatcher<DispatchFn, DispatchFut, CountFn, CountFut, Error>(
    max_rounds: usize,
    processing_poll_interval: Duration,
    mut dispatch_due: DispatchFn,
    mut count_processing: CountFn,
) -> std::result::Result<DispatchStats, Error>
where
    DispatchFn: FnMut() -> DispatchFut,
    DispatchFut: Future<Output = std::result::Result<DispatchStats, Error>>,
    CountFn: FnMut() -> CountFut,
    CountFut: Future<Output = std::result::Result<u64, Error>>,
{
    let mut total = DispatchStats::default();
    tracing::debug!("draining background task dispatcher");

    for _ in 0..max_rounds {
        let stats = dispatch_due().await?;
        let claimed = stats.claimed;
        total.add(stats);
        if claimed > 0 {
            continue;
        }

        if count_processing().await? == 0 {
            tracing::debug!("background task drain finished because no tasks are processing");
            break;
        }

        tokio::time::sleep(processing_poll_interval).await;
    }

    tracing::debug!(
        claimed = total.claimed,
        succeeded = total.succeeded,
        retried = total.retried,
        failed = total.failed,
        "background task dispatcher drain completed"
    );
    Ok(total)
}

/// Aggregate counters returned by a background task dispatch pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DispatchStats {
    /// Number of acknowledged user cancellations.
    pub canceled: usize,
    /// Number of tasks claimed for execution.
    pub claimed: usize,
    /// Number of tasks completed successfully.
    pub succeeded: usize,
    /// Number of tasks scheduled for retry.
    pub retried: usize,
    /// Number of tasks permanently failed.
    pub failed: usize,
}

impl DispatchStats {
    /// Adds another dispatch counter set into this one.
    pub fn add(&mut self, other: Self) {
        self.claimed += other.claimed;
        self.succeeded += other.succeeded;
        self.retried += other.retried;
        self.failed += other.failed;
        self.canceled += other.canceled;
    }

    /// Returns whether any dispatch activity happened.
    #[must_use]
    pub const fn has_activity(&self) -> bool {
        self.claimed > 0
            || self.succeeded > 0
            || self.retried > 0
            || self.failed > 0
            || self.canceled > 0
    }

    /// Adds a task execution outcome to the aggregate counters.
    pub fn add_outcome(&mut self, outcome: TaskDispatchOutcome) {
        self.succeeded += outcome.succeeded;
        self.retried += outcome.retried;
        self.failed += outcome.failed;
        self.canceled += outcome.canceled;
    }
}

/// Counters returned by one claimed task execution.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TaskDispatchOutcome {
    /// Number of acknowledged user cancellations.
    pub canceled: usize,
    /// Number of tasks completed successfully.
    pub succeeded: usize,
    /// Number of tasks scheduled for retry.
    pub retried: usize,
    /// Number of tasks permanently failed.
    pub failed: usize,
}

impl TaskDispatchOutcome {
    /// Creates an acknowledged cancellation outcome.
    #[must_use]
    pub const fn canceled() -> Self {
        Self {
            succeeded: 0,
            retried: 0,
            failed: 0,
            canceled: 1,
        }
    }

    /// Creates a successful task outcome.
    #[must_use]
    pub const fn succeeded() -> Self {
        Self {
            succeeded: 1,
            retried: 0,
            canceled: 0,
            failed: 0,
        }
    }

    /// Creates a retried task outcome.
    #[must_use]
    pub const fn retried() -> Self {
        Self {
            succeeded: 0,
            retried: 1,
            canceled: 0,
            failed: 0,
        }
    }

    /// Creates a permanently failed task outcome.
    #[must_use]
    pub const fn failed() -> Self {
        Self {
            succeeded: 0,
            retried: 0,
            canceled: 0,
            failed: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::{
        ClaimableTaskRecord, DispatchStats, TaskClaimStore, TaskDispatchOutcome, TaskLaneConfig,
        available_lane_capacity, claim_due_for_lane, claim_limit_to_u64, dispatch_lanes,
        drain_dispatcher, run_claimed_task_batch, run_with_concurrency_limit,
    };
    use crate::TaskRecord;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestLane {
        Default,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestKind {
        Example,
    }

    impl std::fmt::Display for TestKind {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Example => formatter.write_str("example"),
            }
        }
    }

    #[derive(Debug, Clone)]
    struct TestTask {
        id: i64,
        kind: TestKind,
        processing_token: i64,
    }

    impl TaskRecord<TestKind> for TestTask {
        fn id(&self) -> i64 {
            self.id
        }

        fn kind(&self) -> TestKind {
            self.kind
        }

        fn payload_json(&self) -> &'static str {
            "{}"
        }

        fn result_json(&self) -> Option<&str> {
            None
        }
    }

    impl ClaimableTaskRecord<TestKind> for TestTask {
        fn processing_token(&self) -> i64 {
            self.processing_token
        }
    }

    struct TestClaimStore {
        due: Vec<TestTask>,
        active: u64,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl TaskClaimStore<TestTask, TestKind, TestLane> for TestClaimStore {
        type Error = crate::TaskCoreError;
        async fn claim(
            &self,
            request: super::TaskClaimRequest<TestKind, TestLane>,
        ) -> crate::Result<Vec<(TestTask, crate::TaskLease)>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let available = request
                .lane
                .cluster_limit
                .map_or(request.max_claim, |limit| {
                    available_lane_capacity(limit, self.active).min(request.max_claim)
                });
            self.due
                .iter()
                .take(available)
                .map(|task| {
                    Ok((
                        task.clone(),
                        crate::TaskLease::new(
                            task.id,
                            task.processing_token.checked_add(1).ok_or_else(|| {
                                crate::TaskCoreError::invalid_value("token overflow")
                            })?,
                        ),
                    ))
                })
                .collect()
        }
    }

    const TEST_KINDS: &[TestKind] = &[TestKind::Example];

    #[test]
    fn available_lane_capacity_saturates_at_zero() {
        assert_eq!(available_lane_capacity(4, 0), 4);
        assert_eq!(available_lane_capacity(4, 2), 2);
        assert_eq!(available_lane_capacity(4, 4), 0);
        assert_eq!(available_lane_capacity(4, 9), 0);
        assert_eq!(available_lane_capacity(4, u64::MAX), 0);
    }

    #[test]
    fn claim_limit_to_u64_accepts_common_limits() {
        assert_eq!(claim_limit_to_u64(0), 0);
        assert_eq!(claim_limit_to_u64(16), 16);
    }

    #[tokio::test]
    async fn claim_due_for_lane_returns_claimed_tasks_with_leases() {
        let store = TestClaimStore {
            due: vec![TestTask {
                id: 42,
                kind: TestKind::Example,
                processing_token: 7,
            }],
            active: 0,
            calls: AtomicUsize::new(0),
        };
        let lane_config = TaskLaneConfig {
            lane: TestLane::Default,
            kinds: TEST_KINDS,
            limit: 2,
            cluster_limit: Some(2),
        };

        let claimed = claim_due_for_lane(&store, lane_config, 2, 60, |_| TestLane::Default)
            .await
            .expect("claim should succeed");

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].0.id, 42);
        assert_eq!(claimed[0].1.task_id, 42);
        assert_eq!(claimed[0].1.processing_token, 8);
    }

    #[tokio::test]
    async fn claim_due_for_lane_skips_when_lane_is_at_capacity() {
        let store = TestClaimStore {
            due: vec![TestTask {
                id: 42,
                kind: TestKind::Example,
                processing_token: 7,
            }],
            active: 2,
            calls: AtomicUsize::new(0),
        };
        let lane_config = TaskLaneConfig {
            lane: TestLane::Default,
            kinds: TEST_KINDS,
            limit: 2,
            cluster_limit: Some(2),
        };

        let claimed = claim_due_for_lane(&store, lane_config, 2, 60, |_| TestLane::Default)
            .await
            .expect("claim should succeed");

        assert!(claimed.is_empty());
    }

    #[test]
    fn dispatch_stats_tracks_activity_and_adds_outcomes() {
        let mut stats = DispatchStats::default();
        assert!(!stats.has_activity());

        stats.claimed = 2;
        assert!(stats.has_activity());

        stats.add_outcome(TaskDispatchOutcome {
            canceled: 0,
            succeeded: 1,
            retried: 2,
            failed: 3,
        });
        assert_eq!(stats.succeeded, 1);
        assert_eq!(stats.retried, 2);
        assert_eq!(stats.failed, 3);

        stats.add(DispatchStats {
            canceled: 0,
            claimed: 4,
            succeeded: 5,
            retried: 6,
            failed: 7,
        });
        assert_eq!(stats.claimed, 6);
        assert_eq!(stats.succeeded, 6);
        assert_eq!(stats.retried, 8);
        assert_eq!(stats.failed, 10);
    }

    #[tokio::test]
    async fn run_with_concurrency_limit_caps_parallelism() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut results = run_with_concurrency_limit(vec![1, 2, 3, 4, 5], 2, {
            let active = active.clone();
            let peak = peak.clone();
            move |value| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    value * 2
                }
            }
        })
        .await;
        results.sort_unstable();

        assert_eq!(results, vec![2, 4, 6, 8, 10]);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn dispatch_lanes_aggregates_successful_lane_stats() {
        let lanes = vec![
            TaskLaneConfig {
                lane: TestLane::Default,
                kinds: TEST_KINDS,
                limit: 1,
                cluster_limit: Some(2),
            },
            TaskLaneConfig {
                lane: TestLane::Default,
                kinds: TEST_KINDS,
                limit: 2,
                cluster_limit: Some(2),
            },
        ];

        let stats = dispatch_lanes(lanes, 2, |lane| async move {
            Ok::<_, ()>(DispatchStats {
                canceled: 0,
                claimed: lane.limit,
                succeeded: lane.limit,
                retried: 0,
                failed: 0,
            })
        })
        .await
        .expect("lane dispatch should succeed");

        assert_eq!(stats.claimed, 3);
        assert_eq!(stats.succeeded, 3);
    }

    #[tokio::test]
    async fn dispatch_lanes_returns_first_error_after_all_lanes_finish() {
        let lanes = vec![
            TaskLaneConfig {
                lane: TestLane::Default,
                kinds: TEST_KINDS,
                limit: 1,
                cluster_limit: Some(2),
            },
            TaskLaneConfig {
                lane: TestLane::Default,
                kinds: TEST_KINDS,
                limit: 2,
                cluster_limit: Some(2),
            },
        ];
        let calls = Arc::new(AtomicUsize::new(0));

        let error = dispatch_lanes(lanes, 2, {
            let calls = calls.clone();
            move |lane| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if lane.limit == 1 {
                        Err("first")
                    } else {
                        Ok(DispatchStats::default())
                    }
                }
            }
        })
        .await
        .expect_err("lane dispatch should return first error");

        assert_eq!(error, "first");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn run_claimed_task_batch_sorts_and_aggregates_outcomes() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

        let stats = run_claimed_task_batch(
            vec![
                (2, TaskDispatchOutcome::retried()),
                (1, TaskDispatchOutcome::succeeded()),
            ],
            |(order, _)| *order,
            {
                let seen = seen.clone();
                move |(order, outcome)| {
                    let seen = seen.clone();
                    async move {
                        match seen.lock() {
                            Ok(mut seen) => seen.push(order),
                            Err(poisoned) => poisoned.into_inner().push(order),
                        }
                        Ok::<_, ()>(outcome)
                    }
                }
            },
        )
        .await
        .expect("claimed task batch should succeed");

        assert_eq!(stats.succeeded, 1);
        assert_eq!(stats.retried, 1);
        let seen = match seen.lock() {
            Ok(seen) => seen.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        assert_eq!(seen, vec![1, 2]);
    }

    #[tokio::test]
    async fn drain_dispatcher_accumulates_until_queue_and_processing_are_empty() {
        let dispatch_calls = Arc::new(AtomicUsize::new(0));
        let count_calls = Arc::new(AtomicUsize::new(0));

        let stats = drain_dispatcher(
            4,
            Duration::from_millis(1),
            {
                let dispatch_calls = dispatch_calls.clone();
                move || {
                    let dispatch_calls = dispatch_calls.clone();
                    async move {
                        let call = dispatch_calls.fetch_add(1, Ordering::SeqCst);
                        let stats = match call {
                            0 => DispatchStats {
                                canceled: 0,
                                claimed: 2,
                                succeeded: 1,
                                retried: 0,
                                failed: 0,
                            },
                            _ => DispatchStats::default(),
                        };
                        Ok::<_, ()>(stats)
                    }
                }
            },
            {
                let count_calls = count_calls.clone();
                move || {
                    let count_calls = count_calls.clone();
                    async move {
                        count_calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ()>(0)
                    }
                }
            },
        )
        .await
        .expect("drain should succeed");

        assert_eq!(stats.claimed, 2);
        assert_eq!(stats.succeeded, 1);
        assert_eq!(dispatch_calls.load(Ordering::SeqCst), 2);
        assert_eq!(count_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn drain_dispatcher_stops_at_max_rounds_when_processing_stays_active() {
        let dispatch_calls = Arc::new(AtomicUsize::new(0));
        let count_calls = Arc::new(AtomicUsize::new(0));

        let stats = drain_dispatcher(
            3,
            Duration::from_millis(1),
            {
                let dispatch_calls = dispatch_calls.clone();
                move || {
                    let dispatch_calls = dispatch_calls.clone();
                    async move {
                        dispatch_calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ()>(DispatchStats::default())
                    }
                }
            },
            {
                let count_calls = count_calls.clone();
                move || {
                    let count_calls = count_calls.clone();
                    async move {
                        count_calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ()>(1)
                    }
                }
            },
        )
        .await
        .expect("drain should succeed");

        assert_eq!(stats, DispatchStats::default());
        assert_eq!(dispatch_calls.load(Ordering::SeqCst), 3);
        assert_eq!(count_calls.load(Ordering::SeqCst), 3);
    }
    #[tokio::test]
    async fn invalid_store_results_are_rejected_without_panicking() {
        struct InvalidStore {
            id: i64,
            token: i64,
            count: usize,
        }
        #[async_trait::async_trait]
        impl TaskClaimStore<TestTask, TestKind, TestLane> for InvalidStore {
            type Error = crate::TaskCoreError;
            async fn claim(
                &self,
                _: super::TaskClaimRequest<TestKind, TestLane>,
            ) -> crate::Result<Vec<(TestTask, crate::TaskLease)>> {
                Ok((0..self.count)
                    .map(|_| {
                        (
                            TestTask {
                                id: 42,
                                kind: TestKind::Example,
                                processing_token: 7,
                            },
                            crate::TaskLease::new(self.id, self.token),
                        )
                    })
                    .collect())
            }
        }
        let lane = TaskLaneConfig {
            lane: TestLane::Default,
            kinds: TEST_KINDS,
            limit: 2,
            cluster_limit: None,
        };
        for store in [
            InvalidStore {
                id: 999,
                token: 8,
                count: 1,
            },
            InvalidStore {
                id: 42,
                token: 7,
                count: 1,
            },
            InvalidStore {
                id: 42,
                token: 8,
                count: 2,
            },
            InvalidStore {
                id: 42,
                token: 8,
                count: 3,
            },
        ] {
            assert!(
                claim_due_for_lane(&store, lane, 2, 60, |_| TestLane::Default)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn no_available_local_slots_never_calls_the_store() {
        let store = TestClaimStore {
            due: vec![],
            active: 0,
            calls: AtomicUsize::new(0),
        };
        for (limit, slots) in [(2, 0), (0, 2)] {
            let lane = TaskLaneConfig {
                lane: TestLane::Default,
                kinds: TEST_KINDS,
                limit,
                cluster_limit: None,
            };
            assert!(
                claim_due_for_lane(&store, lane, slots, 60, |_| TestLane::Default)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn store_token_overflow_propagates_without_an_invalid_lease() {
        let store = TestClaimStore {
            due: vec![TestTask {
                id: 42,
                kind: TestKind::Example,
                processing_token: i64::MAX,
            }],
            active: 0,
            calls: AtomicUsize::new(0),
        };
        let lane = TaskLaneConfig {
            lane: TestLane::Default,
            kinds: TEST_KINDS,
            limit: 1,
            cluster_limit: None,
        };
        let error = claim_due_for_lane(&store, lane, 1, 60, |_| TestLane::Default)
            .await
            .unwrap_err();
        assert_eq!(store.calls.load(Ordering::SeqCst), 1);
        assert_eq!(error, crate::TaskCoreError::invalid_value("token overflow"));
    }

    #[test]
    fn request_timing_saturates_and_token_overflow_is_checked() {
        let now = chrono::Utc::now();
        let request = super::TaskClaimRequest {
            lane: TaskLaneConfig {
                lane: TestLane::Default,
                kinds: TEST_KINDS,
                limit: 1,
                cluster_limit: None,
            },
            max_claim: 1,
            now,
            stale_before: now,
            lease_expires_at: now,
            processing_stale_secs: i64::MAX,
        }
        .at(now);
        assert_eq!(
            request.stale_before,
            chrono::DateTime::<chrono::Utc>::MIN_UTC
        );
        assert_eq!(
            request.lease_expires_at,
            chrono::DateTime::<chrono::Utc>::MAX_UTC
        );
        assert!(
            super::task_claim_candidates(&[TestTask {
                id: 1,
                kind: TestKind::Example,
                processing_token: i64::MAX
            }])
            .is_err()
        );
    }
}

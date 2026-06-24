//! Heartbeat loop for claimed background task leases.
//!
//! Product crates own the persistence update that extends a task lease. Forge owns the surrounding
//! loop: interval scheduling, cooperative stop handling, stale-worker detection, and transient
//! storage-error handling.

use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::{TaskCoreError, TaskLease, TaskLeaseGuard};

/// Product storage adapter used by the generic heartbeat loop.
#[async_trait::async_trait]
pub trait TaskHeartbeatStore: Send + Sync {
    /// Product error type returned by heartbeat storage operations.
    type Error: From<TaskCoreError> + std::fmt::Display + Send;

    /// Attempts to persist one heartbeat renewal for the given lease.
    ///
    /// Returning `Ok(false)` means the conditional update did not match the current task status or
    /// processing token, so the worker must treat the lease as lost. Returning `Err(_)` is treated
    /// as a transient storage failure unless the in-memory lease guard has already timed out.
    async fn touch_task_heartbeat(
        &self,
        lease: TaskLease,
        now: DateTime<Utc>,
        lease_expires_at: DateTime<Utc>,
    ) -> std::result::Result<bool, Self::Error>;
}

/// Runs heartbeat updates until stopped, the persisted lease is lost, or renewal times out.
pub async fn run_task_heartbeat_loop<Store, LeaseExpiresFn>(
    store: Store,
    lease_guard: TaskLeaseGuard,
    stop_token: CancellationToken,
    interval: std::time::Duration,
    lease_expires_at: LeaseExpiresFn,
) where
    Store: TaskHeartbeatStore,
    LeaseExpiresFn: Fn(DateTime<Utc>) -> DateTime<Utc> + Send + Sync,
{
    let mut heartbeat = tokio::time::interval(interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            _ = stop_token.cancelled() => return,
            _ = heartbeat.tick() => {
                let now = Utc::now();
                let result = tokio::select! {
                    _ = stop_token.cancelled() => return,
                    result = store.touch_task_heartbeat(
                        lease_guard.lease(),
                        now,
                        lease_expires_at(now),
                    ) => result,
                };

                if evaluate_heartbeat_result(&lease_guard, result).is_err() {
                    return;
                }
            }
        }
    }
}

/// Spawns a heartbeat worker with the provided interval.
pub fn spawn_task_heartbeat_with_interval<Store, LeaseExpiresFn>(
    store: Store,
    lease_guard: TaskLeaseGuard,
    stop_token: CancellationToken,
    interval: std::time::Duration,
    lease_expires_at: LeaseExpiresFn,
) -> JoinHandle<()>
where
    Store: TaskHeartbeatStore + 'static,
    LeaseExpiresFn: Fn(DateTime<Utc>) -> DateTime<Utc> + Send + Sync + 'static,
{
    tokio::spawn(async move {
        run_task_heartbeat_loop(store, lease_guard, stop_token, interval, lease_expires_at).await;
    })
}

/// Evaluates one persisted heartbeat result and updates the in-memory lease guard.
pub fn evaluate_heartbeat_result<Error>(
    lease_guard: &TaskLeaseGuard,
    result: std::result::Result<bool, Error>,
) -> std::result::Result<(), Error>
where
    Error: From<TaskCoreError> + std::fmt::Display,
{
    let lease = lease_guard.lease();
    match result {
        Ok(true) => {
            lease_guard.record_renewed();
            Ok(())
        }
        Ok(false) => {
            tracing::info!(
                task_id = lease.task_id,
                processing_token = lease.processing_token,
                "background task lease lost; stopping outdated worker"
            );
            Err(lease_guard.mark_lost().into())
        }
        Err(error) => {
            tracing::warn!(
                task_id = lease.task_id,
                processing_token = lease.processing_token,
                error = %error,
                "background task heartbeat update failed; continuing and retrying next heartbeat"
            );
            lease_guard.ensure_active().map_err(Error::from)
        }
    }
}

/// Stops and awaits a heartbeat worker.
pub async fn stop_task_heartbeat(stop_token: CancellationToken, heartbeat_handle: JoinHandle<()>) {
    stop_token.cancel();
    if let Err(error) = heartbeat_handle.await {
        tracing::warn!(error = %error, "background task heartbeat worker stopped unexpectedly");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use tokio_util::sync::CancellationToken;

    use super::{
        TaskHeartbeatStore, evaluate_heartbeat_result, run_task_heartbeat_loop,
        spawn_task_heartbeat_with_interval, stop_task_heartbeat,
    };
    use crate::{TaskCoreError, TaskLease, TaskLeaseGuard, task_lease_expires_at};

    struct TestHeartbeatStore {
        touches: Arc<AtomicUsize>,
        should_match: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl TaskHeartbeatStore for TestHeartbeatStore {
        type Error = TaskCoreError;

        async fn touch_task_heartbeat(
            &self,
            _lease: TaskLease,
            _now: DateTime<Utc>,
            _lease_expires_at: DateTime<Utc>,
        ) -> std::result::Result<bool, Self::Error> {
            self.touches.fetch_add(1, Ordering::SeqCst);
            Ok(self.should_match.load(Ordering::SeqCst))
        }
    }

    #[test]
    fn heartbeat_result_records_successful_renewal() {
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::from_secs(60));

        evaluate_heartbeat_result::<TaskCoreError>(&guard, Ok(true))
            .expect("heartbeat should renew");

        guard.ensure_active().expect("lease should remain active");
    }

    #[test]
    fn heartbeat_result_marks_lost_on_false_update() {
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::from_secs(60));

        let error = evaluate_heartbeat_result::<TaskCoreError>(&guard, Ok(false))
            .expect_err("false update should lose lease");

        assert!(error.is_task_lease_lost());
        assert!(
            guard
                .ensure_active()
                .is_err_and(|error| error.is_task_lease_lost())
        );
    }

    #[test]
    fn heartbeat_result_keeps_retrying_transient_error_before_timeout() {
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::from_secs(60));

        evaluate_heartbeat_result(
            &guard,
            Err(TaskCoreError::codec("database temporarily unavailable")),
        )
        .expect("transient error should keep lease alive before timeout");
    }

    #[test]
    fn heartbeat_result_stops_after_renewal_timeout() {
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::ZERO);

        let error = evaluate_heartbeat_result(
            &guard,
            Err(TaskCoreError::codec("database temporarily unavailable")),
        )
        .expect_err("timed-out lease should stop after transient error");

        assert!(error.is_task_lease_renewal_timed_out());
    }

    #[tokio::test]
    async fn heartbeat_loop_runs_until_stopped() {
        let touches = Arc::new(AtomicUsize::new(0));
        let store = TestHeartbeatStore {
            touches: touches.clone(),
            should_match: Arc::new(AtomicBool::new(true)),
        };
        let stop_token = CancellationToken::new();
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::from_secs(60));

        let handle = tokio::spawn(run_task_heartbeat_loop(
            store,
            guard,
            stop_token.clone(),
            Duration::from_millis(1),
            |now| task_lease_expires_at(now, 60),
        ));

        while touches.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        stop_task_heartbeat(stop_token, handle).await;
        assert!(touches.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn spawned_heartbeat_worker_can_be_stopped() {
        let touches = Arc::new(AtomicUsize::new(0));
        let store = TestHeartbeatStore {
            touches: touches.clone(),
            should_match: Arc::new(AtomicBool::new(true)),
        };
        let stop_token = CancellationToken::new();
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::from_secs(60));

        let handle = spawn_task_heartbeat_with_interval(
            store,
            guard,
            stop_token.clone(),
            Duration::from_millis(1),
            |now| task_lease_expires_at(now, 60),
        );

        while touches.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        stop_task_heartbeat(stop_token, handle).await;
        assert!(touches.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn heartbeat_loop_stops_when_persisted_lease_is_lost() {
        let touches = Arc::new(AtomicUsize::new(0));
        let store = TestHeartbeatStore {
            touches: touches.clone(),
            should_match: Arc::new(AtomicBool::new(false)),
        };
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::from_secs(60));

        run_task_heartbeat_loop(
            store,
            guard,
            CancellationToken::new(),
            Duration::from_millis(1),
            |now| task_lease_expires_at(now, 60),
        )
        .await;

        assert_eq!(touches.load(Ordering::SeqCst), 1);
    }
}

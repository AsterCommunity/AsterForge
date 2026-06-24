//! Processing lease guards for background task workers.
//!
//! A processing lease protects persisted task state from stale workers. Product crates still own
//! the database columns and compare-and-swap updates, but Forge owns the in-memory guard used by
//! task code and heartbeat loops to decide whether the current worker may keep writing progress.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::{Result, TaskCoreError};

/// Persisted processing lease assigned when a task is claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskLease {
    /// Persisted task identifier.
    pub task_id: i64,
    /// Processing token assigned by the successful claim.
    pub processing_token: i64,
}

impl TaskLease {
    /// Creates a task processing lease.
    pub const fn new(task_id: i64, processing_token: i64) -> Self {
        Self {
            task_id,
            processing_token,
        }
    }
}

/// Shared in-memory lease guard observed by task code and heartbeat code.
#[derive(Debug, Clone)]
pub struct TaskLeaseGuard {
    lease: TaskLease,
    renewal_timeout: Duration,
    shutdown_token: Option<CancellationToken>,
    state: Arc<Mutex<TaskLeaseGuardState>>,
}

#[derive(Debug)]
struct TaskLeaseGuardState {
    last_renewed_at: Instant,
    termination: Option<TaskLeaseTermination>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskLeaseTermination {
    Lost,
    RenewalTimedOut,
    ShutdownRequested,
}

impl TaskLeaseGuard {
    /// Creates a guard with the provided renewal timeout.
    pub fn new(lease: TaskLease, renewal_timeout: Duration) -> Self {
        Self {
            lease,
            renewal_timeout,
            shutdown_token: None,
            state: Arc::new(Mutex::new(TaskLeaseGuardState {
                last_renewed_at: Instant::now(),
                termination: None,
            })),
        }
    }

    /// Creates a guard that also observes worker shutdown.
    pub fn with_shutdown_token(
        lease: TaskLease,
        renewal_timeout: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self {
            shutdown_token: Some(shutdown_token),
            ..Self::new(lease, renewal_timeout)
        }
    }

    /// Returns the persisted lease represented by this guard.
    pub const fn lease(&self) -> TaskLease {
        self.lease
    }

    /// Records a successful persistent lease renewal.
    pub fn record_renewed(&self) {
        let mut state = self.lock_state();
        if state.termination.is_none() {
            state.last_renewed_at = Instant::now();
        }
    }

    /// Marks the lease as lost and returns the corresponding error.
    pub fn mark_lost(&self) -> TaskCoreError {
        let mut state = self.lock_state();
        state.termination = Some(TaskLeaseTermination::Lost);
        task_lease_lost(self.lease)
    }

    /// Marks worker shutdown and returns the corresponding error.
    pub fn mark_shutdown_requested(&self) -> TaskCoreError {
        let mut state = self.lock_state();
        state.termination = Some(TaskLeaseTermination::ShutdownRequested);
        task_worker_shutdown_requested(self.lease)
    }

    /// Returns success only while the lease is still safe for writes.
    pub fn ensure_active(&self) -> Result<()> {
        let mut state = self.lock_state();
        match state.termination {
            Some(TaskLeaseTermination::Lost) => return Err(task_lease_lost(self.lease)),
            Some(TaskLeaseTermination::RenewalTimedOut) => {
                return Err(task_lease_renewal_timed_out(self.lease));
            }
            Some(TaskLeaseTermination::ShutdownRequested) => {
                return Err(task_worker_shutdown_requested(self.lease));
            }
            None => {}
        }
        if self
            .shutdown_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            state.termination = Some(TaskLeaseTermination::ShutdownRequested);
            return Err(task_worker_shutdown_requested(self.lease));
        }
        if state.last_renewed_at.elapsed() >= self.renewal_timeout {
            state.termination = Some(TaskLeaseTermination::RenewalTimedOut);
            return Err(task_lease_renewal_timed_out(self.lease));
        }
        Ok(())
    }

    fn lock_state(&self) -> MutexGuard<'_, TaskLeaseGuardState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Task execution context passed to product task implementations.
#[derive(Debug, Clone)]
pub struct TaskExecutionContext {
    lease_guard: TaskLeaseGuard,
    shutdown_token: CancellationToken,
}

impl TaskExecutionContext {
    /// Creates a task execution context.
    pub fn new(
        lease: TaskLease,
        renewal_timeout: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self {
            lease_guard: TaskLeaseGuard::with_shutdown_token(
                lease,
                renewal_timeout,
                shutdown_token.clone(),
            ),
            shutdown_token,
        }
    }

    /// Returns the lease guard used by progress and heartbeat updates.
    pub const fn lease_guard(&self) -> &TaskLeaseGuard {
        &self.lease_guard
    }

    /// Returns success only while the worker should continue task execution.
    pub fn ensure_active(&self) -> Result<()> {
        self.lease_guard.ensure_active()
    }

    /// Sleeps until `duration` elapses or shutdown is requested.
    pub async fn sleep_or_shutdown(&self, duration: Duration) -> Result<()> {
        self.lease_guard.ensure_active()?;

        tokio::select! {
            biased;
            _ = self.shutdown_token.cancelled() => Err(self.lease_guard.mark_shutdown_requested()),
            _ = tokio::time::sleep(duration) => Ok(()),
        }
    }

    /// Waits for shutdown and then returns the shutdown-requested lease error.
    pub async fn shutdown_requested(&self) -> Result<()> {
        self.shutdown_token.cancelled().await;
        Err(self.lease_guard.mark_shutdown_requested())
    }
}

/// Creates the error used when a worker loses its persisted lease.
pub const fn task_lease_lost(lease: TaskLease) -> TaskCoreError {
    TaskCoreError::LeaseLost {
        task_id: lease.task_id,
        processing_token: lease.processing_token,
    }
}

/// Creates the error used when a worker exceeds its renewal timeout.
pub const fn task_lease_renewal_timed_out(lease: TaskLease) -> TaskCoreError {
    TaskCoreError::LeaseRenewalTimedOut {
        task_id: lease.task_id,
        processing_token: lease.processing_token,
    }
}

/// Creates the error used when a worker observes cooperative shutdown.
pub const fn task_worker_shutdown_requested(lease: TaskLease) -> TaskCoreError {
    TaskCoreError::WorkerShutdownRequested {
        task_id: lease.task_id,
        processing_token: lease.processing_token,
    }
}

/// Returns the persisted lease expiry timestamp for a claim or heartbeat update.
pub fn task_lease_expires_at(
    now: chrono::DateTime<chrono::Utc>,
    processing_stale_secs: i64,
) -> chrono::DateTime<chrono::Utc> {
    now + chrono::Duration::seconds(processing_stale_secs.max(1))
}

/// Returns the in-memory renewal timeout used to stop unsafe workers.
pub fn task_lease_renewal_timeout(processing_stale_secs: i64, heartbeat_secs: u64) -> Duration {
    let stale_secs = i64_to_u64_saturating(processing_stale_secs.max(1));
    let heartbeat_secs = heartbeat_secs.max(1);
    Duration::from_secs(stale_secs.saturating_sub(heartbeat_secs).max(1))
}

fn i64_to_u64_saturating(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    use super::{
        TaskExecutionContext, TaskLease, TaskLeaseGuard, task_lease_expires_at,
        task_lease_renewal_timeout,
    };

    #[test]
    fn lease_guard_reports_lost_lease_after_mark_lost() {
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::from_secs(60));

        let error = guard.mark_lost();

        assert!(error.is_task_lease_lost());
        assert!(
            guard
                .ensure_active()
                .is_err_and(|error| error.is_task_lease_lost())
        );
    }

    #[test]
    fn lease_guard_reports_renewal_timeout() {
        let guard = TaskLeaseGuard::new(TaskLease::new(7, 2), Duration::ZERO);

        let error = guard.ensure_active().expect_err("lease should time out");

        assert!(error.is_task_lease_renewal_timed_out());
    }

    #[test]
    fn lease_guard_observes_shutdown_token() {
        let shutdown_token = CancellationToken::new();
        let guard = TaskLeaseGuard::with_shutdown_token(
            TaskLease::new(7, 2),
            Duration::from_secs(60),
            shutdown_token.clone(),
        );

        shutdown_token.cancel();

        assert!(
            guard
                .ensure_active()
                .is_err_and(|error| error.is_task_worker_shutdown_requested())
        );
    }

    #[tokio::test]
    async fn execution_context_sleep_returns_on_shutdown() {
        let shutdown_token = CancellationToken::new();
        let context = TaskExecutionContext::new(
            TaskLease::new(7, 2),
            Duration::from_secs(60),
            shutdown_token.clone(),
        );

        shutdown_token.cancel();
        let error = context
            .sleep_or_shutdown(Duration::from_secs(60))
            .await
            .expect_err("sleep should stop for shutdown");

        assert!(error.is_task_worker_shutdown_requested());
    }

    #[test]
    fn lease_timing_helpers_match_yggdrasil_and_drive_policy() {
        let now = Utc::now();

        assert_eq!(
            task_lease_expires_at(now, 60),
            now + chrono::Duration::seconds(60)
        );
        assert_eq!(
            task_lease_expires_at(now, 0),
            now + chrono::Duration::seconds(1)
        );
        assert_eq!(task_lease_renewal_timeout(60, 10), Duration::from_secs(50));
        assert_eq!(task_lease_renewal_timeout(1, 10), Duration::from_secs(1));
    }
}

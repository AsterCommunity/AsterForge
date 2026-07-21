//! Runtime lease supervision for multi-instance services.
//!
//! A runtime lease protects process-level singleton groups such as schedulers,
//! cleanup loops, outbox dispatchers, and other background producers that must
//! run on only one service instance at a time. It is intentionally separate
//! from task-row processing leases: task leases protect one persisted work item,
//! while runtime leases decide which process is allowed to start a whole worker
//! group.
//!
//! The storage backend is abstracted by [`RuntimeLeaseStore`]. Database-backed
//! services can use the store provided by `aster_forge_db`; tests and other
//! deployments can provide their own implementation. The supervisor remains
//! conservative: if renewal fails or ownership is lost, it cancels the leased
//! workload before trying to acquire the lease again.

use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

/// Minimum lease TTL used when a caller provides a zero duration.
pub const DEFAULT_RUNTIME_LEASE_TTL: Duration = Duration::from_secs(30);
/// Minimum retry interval used when a caller provides a zero duration.
pub const DEFAULT_RUNTIME_LEASE_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Generates a process-unique runtime lease owner ID.
///
/// The owner ID identifies this running process instance, not a configured
/// deployment node. A fresh value on every process start prevents an old stuck
/// process and a newly restarted process from being treated as the same lease
/// owner.
pub fn new_runtime_lease_owner_id() -> String {
    aster_forge_utils::id::new_runtime_id()
}

/// Runtime lease settings for one singleton worker group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLeaseConfig {
    /// Stable lease key shared by all service instances.
    pub lease_id: String,
    /// Process-unique identifier for the current runtime owner.
    pub owner_id: String,
    /// Time after which another owner may take over if renewals stop.
    pub ttl: Duration,
    /// Interval used by the current owner to renew the lease.
    pub renew_interval: Duration,
    /// Interval used by standby instances before attempting acquisition again.
    pub standby_retry_interval: Duration,
}

impl RuntimeLeaseConfig {
    /// Creates runtime lease settings with conservative default intervals.
    pub fn new(lease_id: impl Into<String>, owner_id: impl Into<String>) -> Self {
        Self {
            lease_id: lease_id.into(),
            owner_id: owner_id.into(),
            ttl: DEFAULT_RUNTIME_LEASE_TTL,
            renew_interval: Duration::from_secs(10),
            standby_retry_interval: DEFAULT_RUNTIME_LEASE_RETRY_INTERVAL,
        }
    }

    /// Sets the lease TTL.
    pub const fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Sets the owner renewal interval.
    pub const fn renew_interval(mut self, renew_interval: Duration) -> Self {
        self.renew_interval = renew_interval;
        self
    }

    /// Sets the standby acquisition retry interval.
    pub const fn standby_retry_interval(mut self, standby_retry_interval: Duration) -> Self {
        self.standby_retry_interval = standby_retry_interval;
        self
    }

    fn effective_ttl(&self) -> Duration {
        if self.ttl.is_zero() {
            DEFAULT_RUNTIME_LEASE_TTL
        } else {
            self.ttl
        }
    }

    fn effective_renew_interval(&self) -> Duration {
        let ttl = self.effective_ttl();
        if self.renew_interval.is_zero() {
            return duration_third(ttl).max(Duration::from_secs(1));
        }
        if self.renew_interval >= ttl {
            return duration_half(ttl).max(Duration::from_secs(1));
        }
        self.renew_interval
    }

    fn effective_standby_retry_interval(&self) -> Duration {
        if self.standby_retry_interval.is_zero() {
            DEFAULT_RUNTIME_LEASE_RETRY_INTERVAL
        } else {
            self.standby_retry_interval
        }
    }

    fn expires_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        let ttl = chrono::Duration::from_std(self.effective_ttl()).unwrap_or(chrono::Duration::MAX);
        // `now + ttl` would panic inside chrono's Add impl when the sum overflows
        // DateTime's representable range (operator panics bypass clippy::panic).
        // A lease expiring at the end of time is the correct saturated semantics
        // for absurd TTLs.
        now.checked_add_signed(ttl)
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    }
}

/// Acquisition request passed to a runtime lease store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLeaseClaim<'a> {
    /// Stable lease key shared by all service instances.
    pub lease_id: &'a str,
    /// Process-unique identifier for the current runtime owner.
    pub owner_id: &'a str,
    /// Current timestamp chosen by the caller.
    pub now: DateTime<Utc>,
    /// Expiry timestamp to persist when acquisition succeeds.
    pub expires_at: DateTime<Utc>,
}

/// Current owner observed when a lease is held by another process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLeaseOwner {
    /// Owner identifier stored by the active process.
    pub owner_id: String,
    /// Current expiry timestamp for that owner.
    pub expires_at: DateTime<Utc>,
}

/// Result of one acquisition attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeLeaseAcquire {
    /// The caller owns the lease and may start the singleton worker group.
    Acquired,
    /// Another owner still holds the lease.
    Standby {
        /// Current owner details when available.
        owner: Option<RuntimeLeaseOwner>,
    },
}

impl RuntimeLeaseAcquire {
    /// Returns whether the caller acquired ownership.
    pub const fn acquired(&self) -> bool {
        matches!(self, Self::Acquired)
    }
}

/// Store contract used by runtime lease supervisors.
#[async_trait::async_trait]
pub trait RuntimeLeaseStore: Send + Sync + 'static {
    /// Store error type.
    type Error: Display + Send + Sync + 'static;

    /// Attempts to acquire the lease for the caller.
    async fn try_acquire(
        &self,
        claim: RuntimeLeaseClaim<'_>,
    ) -> Result<RuntimeLeaseAcquire, Self::Error>;

    /// Renews an owned lease and returns whether ownership was still held.
    async fn renew(
        &self,
        lease_id: &str,
        owner_id: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, Self::Error>;

    /// Releases an owned lease during cooperative shutdown.
    async fn release(&self, lease_id: &str, owner_id: &str) -> Result<(), Self::Error>;
}

/// Runs one singleton worker group behind a runtime lease.
///
/// The supervisor stays alive until `shutdown_token` is cancelled. Standby
/// instances retry acquisition periodically. The active owner renews its lease;
/// if renewal fails or reports lost ownership, the workload is cancelled and
/// stopped before the supervisor returns to standby mode.
pub async fn run_runtime_lease_supervisor<Store, StartFn, Workload, StopFn, StopFut>(
    store: Store,
    config: RuntimeLeaseConfig,
    shutdown_token: CancellationToken,
    mut start_workload: StartFn,
    mut stop_workload: StopFn,
) where
    Store: RuntimeLeaseStore,
    StartFn: FnMut(CancellationToken) -> Workload + Send,
    StopFn: FnMut(Workload) -> StopFut + Send,
    StopFut: Future<Output = ()> + Send,
{
    while !shutdown_token.is_cancelled() {
        let now = Utc::now();
        let claim = RuntimeLeaseClaim {
            lease_id: &config.lease_id,
            owner_id: &config.owner_id,
            now,
            expires_at: config.expires_at(now),
        };

        match store.try_acquire(claim).await {
            Ok(RuntimeLeaseAcquire::Acquired) => {
                tracing::info!(
                    lease_id = %config.lease_id,
                    owner_id = %config.owner_id,
                    "runtime lease acquired"
                );
                run_owned_runtime_lease(
                    &store,
                    &config,
                    shutdown_token.clone(),
                    &mut start_workload,
                    &mut stop_workload,
                )
                .await;
            }
            Ok(RuntimeLeaseAcquire::Standby { owner }) => {
                if let Some(owner) = owner {
                    tracing::debug!(
                        lease_id = %config.lease_id,
                        owner_id = %config.owner_id,
                        active_owner_id = %owner.owner_id,
                        active_expires_at = %owner.expires_at,
                        "runtime lease held by another owner"
                    );
                }
                sleep_or_shutdown(config.effective_standby_retry_interval(), &shutdown_token).await;
            }
            Err(error) => {
                tracing::warn!(
                    lease_id = %config.lease_id,
                    owner_id = %config.owner_id,
                    error = %error,
                    "runtime lease acquisition failed"
                );
                sleep_or_shutdown(config.effective_standby_retry_interval(), &shutdown_token).await;
            }
        }
    }
}

async fn run_owned_runtime_lease<Store, StartFn, Workload, StopFn, StopFut>(
    store: &Store,
    config: &RuntimeLeaseConfig,
    shutdown_token: CancellationToken,
    start_workload: &mut StartFn,
    stop_workload: &mut StopFn,
) where
    Store: RuntimeLeaseStore,
    StartFn: FnMut(CancellationToken) -> Workload + Send,
    StopFn: FnMut(Workload) -> StopFut + Send,
    StopFut: Future<Output = ()> + Send,
{
    let workload_token = CancellationToken::new();
    let workload = start_workload(workload_token.clone());
    let renew_interval = config.effective_renew_interval();

    loop {
        tokio::select! {
            biased;
            _ = shutdown_token.cancelled() => {
                workload_token.cancel();
                stop_workload(workload).await;
                if let Err(error) = store.release(&config.lease_id, &config.owner_id).await {
                    tracing::warn!(
                        lease_id = %config.lease_id,
                        owner_id = %config.owner_id,
                        error = %error,
                        "failed to release runtime lease during shutdown"
                    );
                }
                return;
            }
            _ = tokio::time::sleep(renew_interval) => {}
        }

        let now = Utc::now();
        match store
            .renew(
                &config.lease_id,
                &config.owner_id,
                now,
                config.expires_at(now),
            )
            .await
        {
            Ok(true) => {
                tracing::trace!(
                    lease_id = %config.lease_id,
                    owner_id = %config.owner_id,
                    "runtime lease renewed"
                );
            }
            Ok(false) => {
                tracing::warn!(
                    lease_id = %config.lease_id,
                    owner_id = %config.owner_id,
                    "runtime lease ownership lost"
                );
                workload_token.cancel();
                stop_workload(workload).await;
                return;
            }
            Err(error) => {
                tracing::warn!(
                    lease_id = %config.lease_id,
                    owner_id = %config.owner_id,
                    error = %error,
                    "runtime lease renewal failed"
                );
                workload_token.cancel();
                stop_workload(workload).await;
                return;
            }
        }
    }
}

async fn sleep_or_shutdown(duration: Duration, shutdown_token: &CancellationToken) {
    tokio::select! {
        biased;
        _ = shutdown_token.cancelled() => {}
        _ = tokio::time::sleep(duration) => {}
    }
}

fn duration_half(duration: Duration) -> Duration {
    Duration::from_secs_f64(duration.as_secs_f64() / 2.0)
}

fn duration_third(duration: Duration) -> Duration {
    Duration::from_secs_f64(duration.as_secs_f64() / 3.0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fmt;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    use super::{
        RuntimeLeaseAcquire, RuntimeLeaseClaim, RuntimeLeaseConfig, RuntimeLeaseOwner,
        RuntimeLeaseStore, new_runtime_lease_owner_id, run_runtime_lease_supervisor,
    };

    #[test]
    fn expires_at_saturates_instead_of_panicking_on_absurd_ttl() {
        let now = Utc::now();

        // Beyond chrono::Duration's range: from_std fails and Duration::MAX is used.
        let config =
            RuntimeLeaseConfig::new("test.background", "node-a").ttl(Duration::from_secs(u64::MAX));
        assert_eq!(config.expires_at(now), DateTime::<Utc>::MAX_UTC);

        // Inside chrono::Duration's range but beyond DateTime's representable range:
        // must saturate, not panic inside chrono's Add impl.
        let config = RuntimeLeaseConfig::new("test.background", "node-a")
            .ttl(Duration::from_secs(10_u64.pow(15)));
        assert_eq!(config.expires_at(now), DateTime::<Utc>::MAX_UTC);

        // Normal TTLs still add exactly.
        let config =
            RuntimeLeaseConfig::new("test.background", "node-a").ttl(Duration::from_secs(60));
        assert_eq!(config.expires_at(now), now + chrono::Duration::seconds(60));
    }

    #[derive(Debug, Clone)]
    struct TestLeaseError;
    impl fmt::Display for TestLeaseError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("test lease error")
        }
    }

    #[derive(Debug, Clone)]
    struct TestLeaseRow {
        owner_id: String,
        expires_at: DateTime<Utc>,
    }

    #[derive(Default)]
    struct TestLeaseStore {
        rows: Mutex<HashMap<String, TestLeaseRow>>,
        renew_results: Mutex<Vec<Result<bool, TestLeaseError>>>,
        releases: AtomicUsize,
    }

    impl TestLeaseStore {
        async fn hold(&self, lease_id: &str, owner_id: &str, expires_at: DateTime<Utc>) {
            self.rows.lock().await.insert(
                lease_id.to_string(),
                TestLeaseRow {
                    owner_id: owner_id.to_string(),
                    expires_at,
                },
            );
        }

        async fn push_renew_result(&self, result: Result<bool, TestLeaseError>) {
            self.renew_results.lock().await.push(result);
        }
    }

    #[async_trait::async_trait]
    impl RuntimeLeaseStore for Arc<TestLeaseStore> {
        type Error = TestLeaseError;

        async fn try_acquire(
            &self,
            claim: RuntimeLeaseClaim<'_>,
        ) -> Result<RuntimeLeaseAcquire, Self::Error> {
            let mut rows = self.rows.lock().await;
            match rows.get(claim.lease_id) {
                Some(row) if row.owner_id != claim.owner_id && row.expires_at > claim.now => {
                    Ok(RuntimeLeaseAcquire::Standby {
                        owner: Some(RuntimeLeaseOwner {
                            owner_id: row.owner_id.clone(),
                            expires_at: row.expires_at,
                        }),
                    })
                }
                _ => {
                    rows.insert(
                        claim.lease_id.to_string(),
                        TestLeaseRow {
                            owner_id: claim.owner_id.to_string(),
                            expires_at: claim.expires_at,
                        },
                    );
                    Ok(RuntimeLeaseAcquire::Acquired)
                }
            }
        }

        async fn renew(
            &self,
            lease_id: &str,
            owner_id: &str,
            _now: DateTime<Utc>,
            expires_at: DateTime<Utc>,
        ) -> Result<bool, Self::Error> {
            if let Some(result) = self.renew_results.lock().await.pop() {
                return result;
            }

            let mut rows = self.rows.lock().await;
            let Some(row) = rows.get_mut(lease_id) else {
                return Ok(false);
            };
            if row.owner_id != owner_id {
                return Ok(false);
            }
            row.expires_at = expires_at;
            Ok(true)
        }

        async fn release(&self, lease_id: &str, owner_id: &str) -> Result<(), Self::Error> {
            let mut rows = self.rows.lock().await;
            if rows
                .get(lease_id)
                .is_some_and(|row| row.owner_id == owner_id)
            {
                rows.remove(lease_id);
                self.releases.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn runtime_lease_owner_id_is_process_unique_shape() {
        let owner_id = new_runtime_lease_owner_id();
        assert!(owner_id.starts_with("runtime-"));
        assert_eq!(owner_id.len(), "runtime-".len() + 32);
    }

    #[tokio::test]
    async fn supervisor_starts_workload_after_acquire_and_releases_on_shutdown() {
        let store = Arc::new(TestLeaseStore::default());
        let started = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicUsize::new(0));
        let shutdown = CancellationToken::new();

        let handle = tokio::spawn(run_runtime_lease_supervisor(
            store.clone(),
            RuntimeLeaseConfig::new("test.background", "node-a")
                .ttl(Duration::from_secs(5))
                .renew_interval(Duration::from_millis(20)),
            shutdown.clone(),
            {
                let started = started.clone();
                move |_token| {
                    started.fetch_add(1, Ordering::SeqCst);
                }
            },
            {
                let stopped = stopped.clone();
                move |()| {
                    stopped.fetch_add(1, Ordering::SeqCst);
                    async {}
                }
            },
        ));

        tokio::time::sleep(Duration::from_millis(30)).await;
        shutdown.cancel();
        handle.await.expect("supervisor should join");

        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        assert_eq!(store.releases.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn supervisor_stays_standby_when_another_owner_holds_the_lease() {
        let store = Arc::new(TestLeaseStore::default());
        store
            .hold(
                "test.background",
                "node-b",
                Utc::now() + chrono::Duration::seconds(60),
            )
            .await;
        let started = Arc::new(AtomicUsize::new(0));
        let shutdown = CancellationToken::new();

        let handle = tokio::spawn(run_runtime_lease_supervisor(
            store,
            RuntimeLeaseConfig::new("test.background", "node-a")
                .standby_retry_interval(Duration::from_millis(50)),
            shutdown.clone(),
            {
                let started = started.clone();
                move |_token| {
                    started.fetch_add(1, Ordering::SeqCst);
                }
            },
            |()| async {},
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.cancel();
        handle.await.expect("supervisor should join");

        assert_eq!(started.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn supervisor_stops_workload_when_renewal_loses_ownership() {
        let store = Arc::new(TestLeaseStore::default());
        store.push_renew_result(Ok(false)).await;
        let started = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicUsize::new(0));
        let shutdown = CancellationToken::new();

        let handle = tokio::spawn(run_runtime_lease_supervisor(
            store,
            RuntimeLeaseConfig::new("test.background", "node-a")
                .ttl(Duration::from_secs(5))
                .renew_interval(Duration::from_millis(10))
                .standby_retry_interval(Duration::from_secs(60)),
            shutdown.clone(),
            {
                let started = started.clone();
                move |_token| {
                    started.fetch_add(1, Ordering::SeqCst);
                }
            },
            {
                let stopped = stopped.clone();
                let shutdown = shutdown.clone();
                move |()| {
                    stopped.fetch_add(1, Ordering::SeqCst);
                    shutdown.cancel();
                    async {}
                }
            },
        ));

        tokio::time::sleep(Duration::from_millis(40)).await;
        handle.await.expect("supervisor should join");

        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
    }
}

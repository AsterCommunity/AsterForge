//! Active CFAPI hydration waiter correlation and cancellation ownership.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use aster_forge_cloud_files_core::{
    ByteRange, HydrationCancellationHandle, HydrationCancellationOutcome,
};

use crate::{
    Result, WindowsCallbackInfoSnapshot, WindowsCancelFetchDataSnapshot, WindowsCloudFilesError,
    WindowsFetchDataCorrelation, WindowsFetchDataProgress, WindowsFetchDataSnapshot,
    WindowsFetchDataWatchdog, WindowsFetchDataWatchdogConfig, connection::FetchTerminalGate,
};

type FetchCorrelation = WindowsFetchDataCorrelation;

struct ActiveWaiter {
    id: u64,
    range: ByteRange,
    cancellation: HydrationCancellationHandle,
    terminal: FetchTerminalGate,
    watchdog: WindowsFetchDataWatchdog,
    progress: Option<WindowsFetchDataProgress>,
}

#[derive(Default)]
struct RegistryState {
    next_id: u64,
    waiters: HashMap<FetchCorrelation, Vec<ActiveWaiter>>,
    metrics: WindowsFetchDataWaiterMetrics,
    watchdog_config: WindowsFetchDataWatchdogConfig,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Monotonic process-local counters for active fetch waiters and cancellation matching.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsFetchDataWaiterMetrics {
    active_waiters: usize,
    registered_waiters: u64,
    cancellation_callbacks: u64,
    unmatched_cancellations: u64,
    fully_matched_waiters: u64,
    partially_matched_waiters: u64,
    cancelled_waiters: u64,
    already_cancelled_waiters: u64,
    completion_races: u64,
    watchdog_timeouts: u64,
    progress_callbacks: u64,
    progress_updates: u64,
}

impl WindowsFetchDataWaiterMetrics {
    /// Returns the number of hydration waiters currently registered for native cancellation.
    #[must_use]
    pub const fn active_waiters(self) -> usize {
        self.active_waiters
    }

    /// Returns the total number of waiters registered since this registry was created.
    #[must_use]
    pub const fn registered_waiters(self) -> u64 {
        self.registered_waiters
    }

    /// Returns the total number of cancellation callbacks applied to this registry.
    #[must_use]
    pub const fn cancellation_callbacks(self) -> u64 {
        self.cancellation_callbacks
    }

    /// Returns cancellation callbacks that matched no active waiter range.
    #[must_use]
    pub const fn unmatched_cancellations(self) -> u64 {
        self.unmatched_cancellations
    }

    /// Returns active waiters whose complete logical range was covered by a cancellation.
    #[must_use]
    pub const fn fully_matched_waiters(self) -> u64 {
        self.fully_matched_waiters
    }

    /// Returns active waiters retained because cancellation covered only part of their range.
    #[must_use]
    pub const fn partially_matched_waiters(self) -> u64 {
        self.partially_matched_waiters
    }

    /// Returns waiter cancellations that won the core terminal race.
    #[must_use]
    pub const fn cancelled_waiters(self) -> u64 {
        self.cancelled_waiters
    }

    /// Returns full-range cancellations repeated before the cancelled waiter unregistered.
    #[must_use]
    pub const fn already_cancelled_waiters(self) -> u64 {
        self.already_cancelled_waiters
    }

    /// Returns full-range cancellations that lost to normal waiter completion.
    #[must_use]
    pub const fn completion_races(self) -> u64 {
        self.completion_races
    }

    /// Returns waiters cancelled by the local callback watchdog.
    #[must_use]
    pub const fn watchdog_timeouts(self) -> u64 {
        self.watchdog_timeouts
    }

    /// Returns provider progress samples accepted by the registry.
    #[must_use]
    pub const fn progress_callbacks(self) -> u64 {
        self.progress_callbacks
    }

    /// Returns progress samples that refreshed at least one active waiter deadline.
    #[must_use]
    pub const fn progress_updates(self) -> u64 {
        self.progress_updates
    }
}

/// Per-callback result of matching one CFAPI cancellation range to active hydration waiters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsFetchDataCancellationOutcome {
    fully_matched: usize,
    partially_matched: usize,
    cancelled: usize,
    already_cancelled: usize,
    already_completed: usize,
}

/// Result of one host watchdog poll.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsFetchDataWatchdogOutcome {
    cancelled: usize,
    already_cancelled: usize,
    already_completed: usize,
}

impl WindowsFetchDataWatchdogOutcome {
    /// Returns waiters whose cancellation won the core terminal race.
    #[must_use]
    pub const fn cancelled(self) -> usize {
        self.cancelled
    }

    /// Returns waiters that were already cancelled before this poll.
    #[must_use]
    pub const fn already_cancelled(self) -> usize {
        self.already_cancelled
    }

    /// Returns waiters that completed before the watchdog cancellation.
    #[must_use]
    pub const fn already_completed(self) -> usize {
        self.already_completed
    }
}

impl WindowsFetchDataCancellationOutcome {
    /// Returns waiters whose complete range was covered by the callback range.
    #[must_use]
    pub const fn fully_matched(self) -> usize {
        self.fully_matched
    }

    /// Returns waiters retained because only part of their range was cancelled.
    #[must_use]
    pub const fn partially_matched(self) -> usize {
        self.partially_matched
    }

    /// Returns full-range cancellations that won the core waiter terminal race.
    #[must_use]
    pub const fn cancelled(self) -> usize {
        self.cancelled
    }

    /// Returns full-range cancellations already applied to the same active waiter.
    #[must_use]
    pub const fn already_cancelled(self) -> usize {
        self.already_cancelled
    }

    /// Returns full-range cancellations that arrived after waiter completion won.
    #[must_use]
    pub const fn already_completed(self) -> usize {
        self.already_completed
    }

    /// Returns whether any active waiter range overlapped this cancellation.
    #[must_use]
    pub const fn matched(self) -> bool {
        self.fully_matched != 0 || self.partially_matched != 0
    }
}

/// Shareable active-waiter registry for one or more CFAPI connection generations.
///
/// Correlation includes the session generation and every native operation key. A cancellation is
/// forwarded to core only when it covers an entire waiter range. Partial overlap is recorded but
/// retained because core waiter cancellation is atomic and CFAPI still requires bytes outside the
/// cancellation range.
#[derive(Clone, Default)]
pub struct WindowsFetchDataWaiterRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl WindowsFetchDataWaiterRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry with a host-controlled watchdog configuration.
    #[must_use]
    pub fn with_watchdog_config(config: WindowsFetchDataWatchdogConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState {
                watchdog_config: config,
                ..RegistryState::default()
            })),
        }
    }

    /// Returns a point-in-time counter snapshot.
    #[must_use]
    pub fn metrics(&self) -> WindowsFetchDataWaiterMetrics {
        lock(&self.state).metrics
    }

    /// Applies one owned CFAPI cancellation snapshot to correlated active waiters.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn cancel(
        &self,
        snapshot: &WindowsCancelFetchDataSnapshot,
    ) -> Result<WindowsFetchDataCancellationOutcome> {
        let cancellation_range = callback_range(snapshot.range(), snapshot.info().file_size())?;
        let correlation = FetchCorrelation::from_info(snapshot.info());
        let (targets, fully_matched, partially_matched) = {
            let mut state = lock(&self.state);
            state.metrics.cancellation_callbacks =
                state.metrics.cancellation_callbacks.saturating_add(1);

            let Some(waiters) = state.waiters.get(&correlation) else {
                state.metrics.unmatched_cancellations =
                    state.metrics.unmatched_cancellations.saturating_add(1);
                return Ok(WindowsFetchDataCancellationOutcome::default());
            };
            let mut targets = Vec::new();
            let mut fully_matched = 0usize;
            let mut partially_matched = 0usize;
            for waiter in waiters {
                if range_covers(cancellation_range, waiter.range) {
                    fully_matched = fully_matched.saturating_add(1);
                    targets.push((waiter.cancellation.clone(), waiter.terminal.clone()));
                } else if ranges_overlap(cancellation_range, waiter.range) {
                    partially_matched = partially_matched.saturating_add(1);
                }
            }
            if fully_matched == 0 && partially_matched == 0 {
                state.metrics.unmatched_cancellations =
                    state.metrics.unmatched_cancellations.saturating_add(1);
            }
            (targets, fully_matched, partially_matched)
        };

        let mut outcome = WindowsFetchDataCancellationOutcome {
            fully_matched,
            partially_matched,
            ..WindowsFetchDataCancellationOutcome::default()
        };
        for (cancellation, terminal) in targets {
            // Publish the terminal platform state before waking the waiter. Request drop observes
            // the same atomic gate, so aborting the hydration task cannot emit ProviderTerminated.
            terminal.cancel_by_platform();
            match cancellation.cancel() {
                HydrationCancellationOutcome::Cancelled => {
                    outcome.cancelled = outcome.cancelled.saturating_add(1);
                }
                HydrationCancellationOutcome::AlreadyCancelled => {
                    outcome.already_cancelled = outcome.already_cancelled.saturating_add(1);
                }
                HydrationCancellationOutcome::AlreadyCompleted => {
                    outcome.already_completed = outcome.already_completed.saturating_add(1);
                }
            }
        }

        let mut state = lock(&self.state);
        state.metrics.fully_matched_waiters = state
            .metrics
            .fully_matched_waiters
            .saturating_add(usize_to_u64(outcome.fully_matched));
        state.metrics.partially_matched_waiters = state
            .metrics
            .partially_matched_waiters
            .saturating_add(usize_to_u64(outcome.partially_matched));
        state.metrics.cancelled_waiters = state
            .metrics
            .cancelled_waiters
            .saturating_add(usize_to_u64(outcome.cancelled));
        state.metrics.already_cancelled_waiters = state
            .metrics
            .already_cancelled_waiters
            .saturating_add(usize_to_u64(outcome.already_cancelled));
        state.metrics.completion_races = state
            .metrics
            .completion_races
            .saturating_add(usize_to_u64(outcome.already_completed));
        Ok(outcome)
    }

    /// Applies provider progress to all active waiters with the same native correlation and
    /// refreshes their local watchdog deadline.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn report_progress(
        &self,
        snapshot: &WindowsCallbackInfoSnapshot,
        progress: WindowsFetchDataProgress,
        now: Instant,
    ) -> Result<usize> {
        self.report_progress_correlation(FetchCorrelation::from_info(snapshot), progress, now)
    }

    pub(crate) fn report_progress_correlation(
        &self,
        correlation: WindowsFetchDataCorrelation,
        progress: WindowsFetchDataProgress,
        now: Instant,
    ) -> Result<usize> {
        let mut state = lock(&self.state);
        state.metrics.progress_callbacks = state.metrics.progress_callbacks.saturating_add(1);
        let Some(waiters) = state.waiters.get_mut(&correlation) else {
            return Ok(0);
        };
        for waiter in &*waiters {
            if let Some(previous) = waiter.progress {
                progress.advance_from(previous)?;
            }
        }
        let mut updated = 0usize;
        for waiter in waiters {
            waiter.progress = Some(progress);
            waiter.watchdog.touch(now);
            updated = updated.saturating_add(1);
        }
        state.metrics.progress_updates = state
            .metrics
            .progress_updates
            .saturating_add(usize_to_u64(updated));
        Ok(updated)
    }

    /// Cancels pending waiters whose watchdog deadline has elapsed. The host owns the polling
    /// cadence; this method never starts a timer or blocks a native callback thread.
    #[must_use]
    pub fn poll_watchdog(&self, now: Instant) -> WindowsFetchDataWatchdogOutcome {
        let mut targets = Vec::new();
        {
            let mut state = lock(&self.state);
            for waiters in state.waiters.values_mut() {
                for waiter in waiters {
                    if waiter.terminal.watchdog_timed_out() || !waiter.watchdog.is_due(now) {
                        continue;
                    }
                    waiter.terminal.cancel_by_watchdog();
                    targets.push(waiter.cancellation.clone());
                }
            }
        }
        let mut outcome = WindowsFetchDataWatchdogOutcome::default();
        for cancellation in targets {
            match cancellation.cancel() {
                HydrationCancellationOutcome::Cancelled => outcome.cancelled += 1,
                HydrationCancellationOutcome::AlreadyCancelled => outcome.already_cancelled += 1,
                HydrationCancellationOutcome::AlreadyCompleted => outcome.already_completed += 1,
            }
        }
        let mut state = lock(&self.state);
        state.metrics.watchdog_timeouts = state
            .metrics
            .watchdog_timeouts
            .saturating_add(usize_to_u64(outcome.cancelled));
        outcome
    }

    pub(crate) fn register(
        &self,
        snapshot: &WindowsFetchDataSnapshot,
        range: ByteRange,
        cancellation: HydrationCancellationHandle,
        terminal: FetchTerminalGate,
    ) -> Result<WindowsFetchDataWaiterRegistration> {
        self.register_at(snapshot, range, cancellation, terminal, Instant::now())
    }

    fn register_at(
        &self,
        snapshot: &WindowsFetchDataSnapshot,
        range: ByteRange,
        cancellation: HydrationCancellationHandle,
        terminal: FetchTerminalGate,
        now: Instant,
    ) -> Result<WindowsFetchDataWaiterRegistration> {
        let correlation = FetchCorrelation::from_info(snapshot.info());
        let mut state = lock(&self.state);
        let id = state.next_id;
        let next_id = state
            .next_id
            .checked_add(1)
            .ok_or(WindowsCloudFilesError::ActiveFetchWaiterIdOverflow)?;
        let active_waiters = state
            .metrics
            .active_waiters
            .checked_add(1)
            .ok_or(WindowsCloudFilesError::ActiveFetchWaiterCountOverflow)?;
        state.next_id = next_id;
        state.metrics.active_waiters = active_waiters;
        state.metrics.registered_waiters = state.metrics.registered_waiters.saturating_add(1);
        let watchdog_config = state.watchdog_config;
        state
            .waiters
            .entry(correlation)
            .or_default()
            .push(ActiveWaiter {
                id,
                range,
                cancellation,
                terminal: terminal.clone(),
                watchdog: WindowsFetchDataWatchdog::started(watchdog_config, now),
                progress: None,
            });
        drop(state);
        Ok(WindowsFetchDataWaiterRegistration {
            registry: self.clone(),
            correlation,
            id,
            terminal,
            registered: true,
        })
    }

    fn unregister(&self, correlation: FetchCorrelation, id: u64) {
        let mut state = lock(&self.state);
        let mut removed = false;
        let mut empty = false;
        if let Some(waiters) = state.waiters.get_mut(&correlation) {
            let previous = waiters.len();
            waiters.retain(|waiter| waiter.id != id);
            removed = waiters.len() != previous;
            empty = waiters.is_empty();
        }
        if empty {
            state.waiters.remove(&correlation);
        }
        if removed {
            state.metrics.active_waiters = state.metrics.active_waiters.saturating_sub(1);
        }
    }
}

impl fmt::Debug for WindowsFetchDataWaiterRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsFetchDataWaiterRegistry")
            .field("metrics", &self.metrics())
            .finish()
    }
}

pub(crate) struct WindowsFetchDataWaiterRegistration {
    registry: WindowsFetchDataWaiterRegistry,
    correlation: FetchCorrelation,
    id: u64,
    terminal: FetchTerminalGate,
    registered: bool,
}

impl WindowsFetchDataWaiterRegistration {
    pub(crate) fn platform_cancelled(&self) -> bool {
        self.terminal.platform_cancelled()
    }

    pub(crate) fn watchdog_timed_out(&self) -> bool {
        self.terminal.watchdog_timed_out()
    }

    fn unregister(&mut self) {
        if !self.registered {
            return;
        }
        self.registry.unregister(self.correlation, self.id);
        self.registered = false;
    }
}

impl Drop for WindowsFetchDataWaiterRegistration {
    fn drop(&mut self) {
        self.unregister();
    }
}

fn callback_range(range: crate::WindowsCallbackRange, file_size: u64) -> Result<ByteRange> {
    let length = match range.length() {
        crate::WindowsCallbackRangeLength::Exact(length) => length,
        crate::WindowsCallbackRangeLength::ToEnd => file_size.checked_sub(range.offset()).ok_or(
            WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "cancellation range offset exceeds callback file size",
            },
        )?,
    };
    if length == 0 {
        return Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "cancellation range must not be empty",
        });
    }
    let end = range.offset() + length;
    if end > file_size {
        return Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "cancellation range exceeds callback file size",
        });
    }
    ByteRange::new(range.offset(), length).map_err(Into::into)
}

const fn range_covers(outer: ByteRange, inner: ByteRange) -> bool {
    outer.offset() <= inner.offset() && outer.end_exclusive() >= inner.end_exclusive()
}

const fn ranges_overlap(left: ByteRange, right: ByteRange) -> bool {
    left.offset() < right.end_exclusive() && right.offset() < left.end_exclusive()
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aster_forge_cloud_files_core::{
        Alignment, BackendResult, CloudBackendError, CloudBackendErrorKind, CloudContentBackend,
        CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope, ContentReadRange,
        ContentReadRequest, ContentReadResponse, ContentRevision, HydrationCoordinator,
        HydrationError, HydrationRequest, SessionGeneration,
    };
    use async_trait::async_trait;
    use bytes::Bytes;

    use super::*;
    use crate::{
        WindowsCallbackRange, WindowsCancelFetchDataFlags, WindowsConnectionKey,
        WindowsFetchDataFlags, WindowsFileIdentity, WindowsRequestKey, WindowsSyncRootIdentity,
        WindowsTransferKey,
    };

    struct ImmediateBackend;

    #[async_trait]
    impl CloudContentBackend for ImmediateBackend {
        async fn read_content(
            &self,
            request: &ContentReadRequest,
        ) -> BackendResult<ContentReadResponse> {
            let ContentReadRange::Range(range) = request.read_range() else {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::InvalidRequest,
                ));
            };
            ContentReadResponse::new(
                request.revision().clone(),
                range.offset(),
                Bytes::from(vec![
                    0x5a;
                    usize::try_from(range.length())
                        .expect("test range length should fit usize")
                ]),
                request.expected_size(),
            )
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
        }
    }

    fn generation() -> SessionGeneration {
        SessionGeneration::new(7).expect("generation fixture should be non-zero")
    }

    fn item_key() -> CloudItemKey {
        CloudItemKey::new(
            CloudScope::new(
                CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
                CloudRootId::new("root").expect("root fixture should be valid"),
            ),
            CloudItemId::new("item").expect("item fixture should be valid"),
        )
    }

    fn fetch_snapshot() -> WindowsFetchDataSnapshot {
        let key = item_key();
        let info = WindowsCallbackInfoSnapshot::new(
            generation(),
            WindowsConnectionKey::new(1),
            WindowsTransferKey::new(2),
            WindowsRequestKey::new(3),
            None,
            None,
            0,
            4,
            WindowsSyncRootIdentity::encode(key.scope())
                .expect("sync-root identity fixture should encode"),
            5,
            4096,
            WindowsFileIdentity::encode(&key).expect("file identity fixture should encode"),
            None,
            0,
            None,
        )
        .expect("callback info fixture should construct");
        WindowsFetchDataSnapshot::new(
            info,
            WindowsFetchDataFlags::default(),
            WindowsCallbackRange::exact(0, 4096).expect("range fixture should construct"),
            None,
            0,
            0,
        )
    }

    #[tokio::test]
    async fn cancellation_records_completion_that_won_before_registry_matching() {
        let snapshot = fetch_snapshot();
        let range = ByteRange::new(0, 4096).expect("range fixture should be valid");
        let revision =
            ContentRevision::from_slice(b"revision").expect("revision fixture should be valid");
        let coordinator = HydrationCoordinator::new(Arc::new(ImmediateBackend));
        let waiter = coordinator
            .request(HydrationRequest::range(
                item_key(),
                revision,
                4096,
                range,
                Alignment::ONE,
                generation(),
            ))
            .expect("waiter fixture should register");
        let registry = WindowsFetchDataWaiterRegistry::new();
        let registration = registry
            .register(
                &snapshot,
                range,
                waiter.cancellation_handle(),
                FetchTerminalGate::default(),
            )
            .expect("registry fixture should register");
        waiter
            .wait()
            .await
            .expect("immediate waiter should complete before cancellation");

        let cancel = WindowsCancelFetchDataSnapshot::new(
            snapshot.info().clone(),
            WindowsCancelFetchDataFlags::IO_ABORTED,
            WindowsCallbackRange::exact(0, 4096).expect("range fixture should construct"),
        );
        let outcome = registry
            .cancel(&cancel)
            .expect("cancellation fixture should validate");
        assert_eq!(outcome.already_completed(), 1);
        assert_eq!(registry.metrics().completion_races(), 1);
        drop(registration);
        assert_eq!(registry.metrics().active_waiters(), 0);
    }

    #[test]
    fn cancellation_publishes_shared_terminal_gate_before_waiter_resumes() {
        let snapshot = fetch_snapshot();
        let range = ByteRange::new(0, 4096).expect("range fixture should be valid");
        let revision =
            ContentRevision::from_slice(b"revision").expect("revision fixture should be valid");
        let coordinator = HydrationCoordinator::new(Arc::new(ImmediateBackend));
        let waiter = coordinator
            .request(HydrationRequest::range(
                item_key(),
                revision,
                4096,
                range,
                Alignment::ONE,
                generation(),
            ))
            .expect("waiter fixture should register");
        let terminal = FetchTerminalGate::default();
        let registry = WindowsFetchDataWaiterRegistry::new();
        let _registration = registry
            .register(
                &snapshot,
                range,
                waiter.cancellation_handle(),
                terminal.clone(),
            )
            .expect("registry fixture should register");
        let cancel = WindowsCancelFetchDataSnapshot::new(
            snapshot.info().clone(),
            WindowsCancelFetchDataFlags::IO_ABORTED,
            WindowsCallbackRange::exact(0, 4096).expect("range fixture should construct"),
        );

        registry
            .cancel(&cancel)
            .expect("cancellation fixture should validate");

        assert!(terminal.platform_cancelled());
        assert!(!terminal.begin_native());
    }

    #[test]
    fn registration_overflow_errors_do_not_insert_partial_state() {
        let snapshot = fetch_snapshot();
        let range = ByteRange::new(0, 4096).expect("range fixture should be valid");
        let revision =
            ContentRevision::from_slice(b"revision").expect("revision fixture should be valid");
        let coordinator = HydrationCoordinator::new(Arc::new(ImmediateBackend));
        let waiter = coordinator
            .request(HydrationRequest::range(
                item_key(),
                revision,
                4096,
                range,
                Alignment::ONE,
                generation(),
            ))
            .expect("waiter fixture should register");
        let cancellation = waiter.cancellation_handle();

        let identity_overflow = WindowsFetchDataWaiterRegistry::new();
        lock(&identity_overflow.state).next_id = u64::MAX;
        assert!(matches!(
            identity_overflow.register(
                &snapshot,
                range,
                cancellation.clone(),
                FetchTerminalGate::default(),
            ),
            Err(WindowsCloudFilesError::ActiveFetchWaiterIdOverflow)
        ));
        assert_eq!(identity_overflow.metrics().active_waiters(), 0);

        let count_overflow = WindowsFetchDataWaiterRegistry::new();
        lock(&count_overflow.state).metrics.active_waiters = usize::MAX;
        assert!(matches!(
            count_overflow.register(&snapshot, range, cancellation, FetchTerminalGate::default(),),
            Err(WindowsCloudFilesError::ActiveFetchWaiterCountOverflow)
        ));
        assert_eq!(lock(&count_overflow.state).next_id, 0);
        assert!(lock(&count_overflow.state).waiters.is_empty());
    }

    #[tokio::test]
    async fn watchdog_poll_cancels_pending_waiter_and_progress_refreshes_deadline() {
        let snapshot = fetch_snapshot();
        let range = ByteRange::new(0, 4096).expect("range fixture should be valid");
        let revision =
            ContentRevision::from_slice(b"revision").expect("revision fixture should be valid");
        let coordinator = HydrationCoordinator::new(Arc::new(ImmediateBackend));
        let waiter = coordinator
            .request(HydrationRequest::range(
                item_key(),
                revision,
                4096,
                range,
                Alignment::ONE,
                generation(),
            ))
            .expect("waiter fixture should register");
        let config = WindowsFetchDataWatchdogConfig::new(std::time::Duration::from_secs(5))
            .expect("watchdog fixture should be valid");
        let registry = WindowsFetchDataWaiterRegistry::with_watchdog_config(config);
        let start = std::time::Instant::now();
        let registration = registry
            .register_at(
                &snapshot,
                range,
                waiter.cancellation_handle(),
                FetchTerminalGate::default(),
                start,
            )
            .expect("registry fixture should register");
        let progress = WindowsFetchDataProgress::new(4096, 1).expect("progress should be valid");
        assert_eq!(
            registry
                .report_progress(
                    snapshot.info(),
                    progress,
                    start + std::time::Duration::from_secs(4)
                )
                .expect("progress should refresh watchdog"),
            1
        );
        assert_eq!(
            registry
                .poll_watchdog(start + std::time::Duration::from_secs(8))
                .cancelled(),
            0
        );
        let outcome = registry.poll_watchdog(start + std::time::Duration::from_secs(9));
        assert_eq!(outcome.cancelled(), 1);
        assert!(registration.watchdog_timed_out());
        assert_eq!(registry.metrics().watchdog_timeouts(), 1);
        assert!(matches!(
            waiter.wait().await,
            Err(HydrationError::Cancelled)
        ));
        drop(registration);
    }

    #[test]
    fn progress_regression_is_rejected_before_waiter_state_changes() {
        let snapshot = fetch_snapshot();
        let range = ByteRange::new(0, 4096).expect("range fixture should be valid");
        let revision =
            ContentRevision::from_slice(b"revision").expect("revision fixture should be valid");
        let coordinator = HydrationCoordinator::new(Arc::new(ImmediateBackend));
        let waiter = coordinator
            .request(HydrationRequest::range(
                item_key(),
                revision,
                4096,
                range,
                Alignment::ONE,
                generation(),
            ))
            .expect("waiter fixture should register");
        let registry = WindowsFetchDataWaiterRegistry::new();
        let start = Instant::now();
        let registration = registry
            .register_at(
                &snapshot,
                range,
                waiter.cancellation_handle(),
                FetchTerminalGate::default(),
                start,
            )
            .expect("registry fixture should register");
        registry
            .report_progress(
                snapshot.info(),
                WindowsFetchDataProgress::new(4096, 2048).expect("progress should be valid"),
                start,
            )
            .expect("first progress should be accepted");
        assert!(matches!(
            registry.report_progress(
                snapshot.info(),
                WindowsFetchDataProgress::new(4096, 1024).expect("sample should be valid"),
                start + std::time::Duration::from_secs(1),
            ),
            Err(WindowsCloudFilesError::InvalidProviderProgress { .. })
        ));
        assert_eq!(registry.metrics().progress_callbacks(), 2);
        assert_eq!(registry.metrics().progress_updates(), 1);
        drop(registration);
    }
}

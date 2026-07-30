//! Runtime-neutral hydration work sharing, revision fencing, and waiter-scoped cancellation.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

use bytes::{Bytes, BytesMut};
use futures::{
    FutureExt,
    channel::oneshot,
    future::{BoxFuture, Either, Shared, select, try_join_all},
};

use crate::{
    Alignment, ByteRange, CloudBackendError, CloudBackendErrorKind, CloudContentBackend,
    CloudFilesCoreError, CloudItemKey, ContentReadRange, ContentReadRequest, ContentReadResponse,
    ContentRevision, SessionGeneration,
};

/// Result returned by hydration coordination and waiter completion.
pub type HydrationResult<T> = std::result::Result<T, HydrationError>;

/// Product-neutral hydration coordination failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HydrationError {
    /// The individual waiter was cancelled before it won the completion race.
    #[error("hydration waiter was cancelled")]
    Cancelled,
    /// The product-owned content backend failed the revision-bound read.
    #[error(transparent)]
    Backend(#[from] CloudBackendError),
    /// Backend bytes violated the requested revision, range, or size contract.
    #[error(transparent)]
    Contract(#[from] CloudFilesCoreError),
    /// The coordinator reached its configured global or per-content work limit.
    #[error("hydration in-flight work limit exceeded ({scope})")]
    InFlightLimitExceeded {
        /// Whether the global coordinator or one content key reached its limit.
        scope: &'static str,
    },
}

/// Bounded hydration work limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HydrationLimits {
    /// Maximum number of backend work items retained by one coordinator.
    pub max_in_flight_work: usize,
    /// Maximum number of backend work items retained for one content key.
    pub max_in_flight_per_content: usize,
}

impl HydrationLimits {
    /// Conservative defaults suitable for a provider process.
    pub const DEFAULT: Self = Self {
        max_in_flight_work: 1_024,
        max_in_flight_per_content: 128,
    };
}

/// One logical hydration request plus the physical alignment required by its adapter boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HydrationRequest {
    read: ContentReadRequest,
    alignment: Alignment,
    session_generation: SessionGeneration,
}

impl HydrationRequest {
    /// Creates a hydration request from an exact content read and physical transfer alignment.
    pub const fn new(
        read: ContentReadRequest,
        alignment: Alignment,
        session_generation: SessionGeneration,
    ) -> Self {
        Self {
            read,
            alignment,
            session_generation,
        }
    }

    /// Creates a complete-file hydration request.
    pub const fn whole(
        key: CloudItemKey,
        revision: ContentRevision,
        expected_size: u64,
        session_generation: SessionGeneration,
    ) -> Self {
        Self::new(
            ContentReadRequest::whole(key, revision, expected_size),
            Alignment::ONE,
            session_generation,
        )
    }

    /// Creates a range hydration request with adapter-owned physical alignment.
    pub const fn range(
        key: CloudItemKey,
        revision: ContentRevision,
        expected_size: u64,
        range: ByteRange,
        alignment: Alignment,
        session_generation: SessionGeneration,
    ) -> Self {
        Self::new(
            ContentReadRequest::range(key, revision, expected_size, range),
            alignment,
            session_generation,
        )
    }

    /// Returns the exact logical content request.
    pub const fn read(&self) -> &ContentReadRequest {
        &self.read
    }

    /// Returns the physical range alignment owned by the active adapter boundary.
    pub const fn alignment(&self) -> Alignment {
        self.alignment
    }

    /// Returns the platform session generation that owns this waiter and its completion.
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HydrationContentKey {
    key: CloudItemKey,
    revision: ContentRevision,
    expected_size: u64,
    session_generation: SessionGeneration,
    alignment: Alignment,
}

impl HydrationContentKey {
    fn from_request(request: &HydrationRequest) -> Self {
        Self {
            key: request.read().key().clone(),
            revision: request.read().revision().clone(),
            expected_size: request.read().expected_size(),
            session_generation: request.session_generation(),
            alignment: request.alignment(),
        }
    }
}

type SharedRead = Shared<BoxFuture<'static, HydrationResult<ContentReadResponse>>>;

#[derive(Clone)]
struct WorkDependency {
    id: u64,
    start: u64,
    end: u64,
    future: SharedRead,
}

struct WorkEntry {
    content: HydrationContentKey,
    start: u64,
    end: u64,
    whole: bool,
    future: SharedRead,
    waiter_count: usize,
}

#[derive(Default)]
struct CoordinatorState {
    next_work_id: u64,
    work: HashMap<u64, WorkEntry>,
    by_content: HashMap<HydrationContentKey, BTreeSet<(u64, u64, u64)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Runtime-neutral coordinator for deduplicated revision-bound content reads.
///
/// The coordinator does not spawn tasks and does not own physical materialization. Returned
/// waiters drive shared backend futures on the caller's executor. When the last waiter releases a
/// work item, dropping the backend future provides the narrow cancellation boundary available to
/// the backend adapter.
#[derive(Clone)]
pub struct HydrationCoordinator {
    backend: Arc<dyn CloudContentBackend>,
    state: Arc<Mutex<CoordinatorState>>,
    limits: HydrationLimits,
}

impl HydrationCoordinator {
    /// Creates an empty coordinator scoped to one product-owned content backend adapter.
    pub fn new(backend: Arc<dyn CloudContentBackend>) -> Self {
        Self::with_limits(backend, HydrationLimits::DEFAULT)
    }

    /// Creates a coordinator with explicit global and per-content backpressure limits.
    pub fn with_limits(backend: Arc<dyn CloudContentBackend>, limits: HydrationLimits) -> Self {
        Self {
            backend,
            state: Arc::new(Mutex::new(CoordinatorState::default())),
            limits,
        }
    }

    /// Returns the configured in-flight work limits.
    pub const fn limits(&self) -> HydrationLimits {
        self.limits
    }

    /// Registers one waiter and returns immediately without spawning backend work.
    ///
    /// Exact requests share one backend future. Overlapping range requests reuse existing work and
    /// add backend reads only for uncovered physical gaps. All sharing keys include item scope,
    /// content revision, and expected size.
    pub fn request(&self, request: HydrationRequest) -> HydrationResult<HydrationWaiter> {
        let content = HydrationContentKey::from_request(&request);
        let (target_start, target_end) = logical_extent(request.read())?;
        let dependencies = match request.read().read_range() {
            ContentReadRange::Whole => self.register_whole(&content, request.read())?,
            ContentReadRange::Range(_) if target_start == target_end => {
                self.register_empty_range(&content, request.read(), target_start)?
            }
            ContentReadRange::Range(_) => {
                let (physical_start, physical_end) = aligned_extent(
                    target_start,
                    target_end,
                    content.expected_size,
                    request.alignment(),
                )?;
                self.register_range(&content, request.read(), physical_start, physical_end)?
            }
        };

        let work_ids = dependencies
            .iter()
            .map(|dependency| dependency.id)
            .collect();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let inner = Arc::new(WaiterInner {
            terminal: AtomicU8::new(TERMINAL_PENDING),
            released: AtomicBool::new(false),
            coordinator: Arc::downgrade(&self.state),
            work_ids,
            cancel_tx: Mutex::new(Some(cancel_tx)),
        });
        Ok(HydrationWaiter {
            request,
            target_start,
            target_end,
            dependencies,
            cancel_rx,
            inner,
            finished: false,
        })
    }

    fn register_whole(
        &self,
        content: &HydrationContentKey,
        request: &ContentReadRequest,
    ) -> HydrationResult<Vec<WorkDependency>> {
        let mut state = lock(&self.state);
        if let Some((id, entry)) = state
            .work
            .iter_mut()
            .find(|(_, entry)| entry.content == *content && entry.whole)
        {
            entry.waiter_count = entry.waiter_count.saturating_add(1);
            return Ok(vec![WorkDependency {
                id: *id,
                start: entry.start,
                end: entry.end,
                future: entry.future.clone(),
            }]);
        }

        ensure_capacity(&state, content, 1, self.limits)?;
        let dependency = insert_work(
            &mut state,
            Arc::clone(&self.backend),
            content.clone(),
            request.clone(),
            0,
            content.expected_size,
            true,
        )?;
        if let Some(entry) = state.work.get_mut(&dependency.id) {
            entry.waiter_count = 1;
        }
        Ok(vec![dependency])
    }

    fn register_empty_range(
        &self,
        content: &HydrationContentKey,
        request: &ContentReadRequest,
        offset: u64,
    ) -> HydrationResult<Vec<WorkDependency>> {
        let mut state = lock(&self.state);
        if let Some((id, entry)) = state.work.iter_mut().find(|(_, entry)| {
            entry.content == *content
                && !entry.whole
                && entry.start == offset
                && entry.end == offset
        }) {
            entry.waiter_count = entry.waiter_count.saturating_add(1);
            return Ok(vec![WorkDependency {
                id: *id,
                start: entry.start,
                end: entry.end,
                future: entry.future.clone(),
            }]);
        }

        ensure_capacity(&state, content, 1, self.limits)?;
        let dependency = insert_work(
            &mut state,
            Arc::clone(&self.backend),
            content.clone(),
            request.clone(),
            offset,
            offset,
            false,
        )?;
        if let Some(entry) = state.work.get_mut(&dependency.id) {
            entry.waiter_count = 1;
        }
        Ok(vec![dependency])
    }

    fn register_range(
        &self,
        content: &HydrationContentKey,
        original_request: &ContentReadRequest,
        physical_start: u64,
        physical_end: u64,
    ) -> HydrationResult<Vec<WorkDependency>> {
        let mut state = lock(&self.state);
        let mut dependencies = state
            .by_content
            .get(content)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .take_while(|(start, _, _)| *start < physical_end)
            .filter_map(|(start, end, id)| {
                let entry = state.work.get(id)?;
                if entry.whole || ranges_overlap(physical_start, physical_end, *start, *end) {
                    Some(WorkDependency {
                        id: *id,
                        start: *start,
                        end: *end,
                        future: entry.future.clone(),
                    })
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        dependencies.sort_by_key(|dependency| (dependency.start, dependency.end));

        let mut cursor = physical_start;
        let existing = dependencies.clone();
        let mut gaps = Vec::new();
        for dependency in existing {
            if dependency.end > cursor {
                if dependency.start > cursor {
                    let gap_end = dependency.start.min(physical_end);
                    if cursor < gap_end {
                        gaps.push((cursor, gap_end));
                    }
                }
                cursor = cursor.max(dependency.end.min(physical_end));
                if cursor >= physical_end {
                    break;
                }
            }
        }
        if cursor < physical_end {
            gaps.push((cursor, physical_end));
        }
        ensure_capacity(&state, content, gaps.len(), self.limits)?;
        for (start, end) in gaps {
            dependencies.push(insert_range_work(
                &mut state,
                Arc::clone(&self.backend),
                content.clone(),
                original_request,
                start,
                end,
            )?);
        }

        dependencies.sort_by_key(|dependency| (dependency.start, dependency.end));
        for dependency in &dependencies {
            if let Some(entry) = state.work.get_mut(&dependency.id) {
                entry.waiter_count = entry.waiter_count.saturating_add(1);
            }
        }
        Ok(dependencies)
    }
}

fn insert_range_work(
    state: &mut CoordinatorState,
    backend: Arc<dyn CloudContentBackend>,
    content: HydrationContentKey,
    original_request: &ContentReadRequest,
    start: u64,
    end: u64,
) -> HydrationResult<WorkDependency> {
    let range = ByteRange::new(start, end.saturating_sub(start))?;
    let request = ContentReadRequest::range(
        original_request.key().clone(),
        original_request.revision().clone(),
        original_request.expected_size(),
        range,
    );
    insert_work(state, backend, content, request, start, end, false)
}

fn insert_work(
    state: &mut CoordinatorState,
    backend: Arc<dyn CloudContentBackend>,
    content: HydrationContentKey,
    request: ContentReadRequest,
    start: u64,
    end: u64,
    whole: bool,
) -> HydrationResult<WorkDependency> {
    let id = state.next_work_id;
    state.next_work_id = state.next_work_id.checked_add(1).ok_or_else(|| {
        CloudFilesCoreError::invalid_content_response("hydration work identity exhausted")
    })?;
    let future_request = request.clone();
    let future = async move {
        let response = backend.read_content(&future_request).await?;
        future_request.validate_response(&response)?;
        Ok(response)
    }
    .boxed()
    .shared();
    let index_content = content.clone();
    state.work.insert(
        id,
        WorkEntry {
            content,
            start,
            end,
            whole,
            future: future.clone(),
            waiter_count: 0,
        },
    );
    state
        .by_content
        .entry(index_content)
        .or_default()
        .insert((start, end, id));
    Ok(WorkDependency {
        id,
        start,
        end,
        future,
    })
}

fn ensure_capacity(
    state: &CoordinatorState,
    content: &HydrationContentKey,
    additional: usize,
    limits: HydrationLimits,
) -> HydrationResult<()> {
    if limits.max_in_flight_work == 0
        || state.work.len().saturating_add(additional) > limits.max_in_flight_work
    {
        return Err(HydrationError::InFlightLimitExceeded { scope: "global" });
    }
    let per_content = state.by_content.get(content).map_or(0, BTreeSet::len);
    if limits.max_in_flight_per_content == 0
        || per_content.saturating_add(additional) > limits.max_in_flight_per_content
    {
        return Err(HydrationError::InFlightLimitExceeded { scope: "content" });
    }
    Ok(())
}

fn logical_extent(request: &ContentReadRequest) -> HydrationResult<(u64, u64)> {
    match request.read_range() {
        ContentReadRange::Whole => Ok((0, request.expected_size())),
        ContentReadRange::Range(range) => {
            if range.offset() > request.expected_size() {
                return Err(CloudBackendError::new(CloudBackendErrorKind::InvalidRequest).into());
            }
            Ok((
                range.offset(),
                range.end_exclusive().min(request.expected_size()),
            ))
        }
    }
}

fn aligned_extent(
    start: u64,
    end: u64,
    total_size: u64,
    alignment: Alignment,
) -> HydrationResult<(u64, u64)> {
    let alignment = alignment.get();
    let aligned_start = start / alignment * alignment;
    if end == total_size {
        return Ok((aligned_start, total_size));
    }
    let remainder = end % alignment;
    let aligned_end = if remainder == 0 {
        end
    } else {
        end.checked_add(alignment - remainder).ok_or_else(|| {
            CloudFilesCoreError::invalid_byte_range("aligned hydration range exceeds u64")
        })?
    };
    Ok((aligned_start, aligned_end.min(total_size)))
}

const fn ranges_overlap(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> bool {
    left_start < right_end && right_start < left_end
}

const TERMINAL_PENDING: u8 = 0;
const TERMINAL_COMPLETED: u8 = 1;
const TERMINAL_CANCELLED: u8 = 2;

struct WaiterInner {
    terminal: AtomicU8,
    released: AtomicBool,
    coordinator: Weak<Mutex<CoordinatorState>>,
    work_ids: Vec<u64>,
    cancel_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl WaiterInner {
    fn cancel(&self) -> HydrationCancellationOutcome {
        match self.terminal.compare_exchange(
            TERMINAL_PENDING,
            TERMINAL_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                if let Some(sender) = lock(&self.cancel_tx).take() {
                    let _ = sender.send(());
                }
                self.release_work();
                HydrationCancellationOutcome::Cancelled
            }
            Err(TERMINAL_CANCELLED) => HydrationCancellationOutcome::AlreadyCancelled,
            Err(_) => HydrationCancellationOutcome::AlreadyCompleted,
        }
    }

    fn complete(&self) -> bool {
        if self
            .terminal
            .compare_exchange(
                TERMINAL_PENDING,
                TERMINAL_COMPLETED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            lock(&self.cancel_tx).take();
            self.release_work();
            true
        } else {
            false
        }
    }

    fn release_work(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(coordinator) = self.coordinator.upgrade() else {
            return;
        };
        let mut state = lock(&coordinator);
        for id in &self.work_ids {
            let remove = if let Some(entry) = state.work.get_mut(id) {
                entry.waiter_count = entry.waiter_count.saturating_sub(1);
                entry.waiter_count == 0
            } else {
                false
            };
            if remove && let Some(entry) = state.work.remove(id) {
                let empty = if let Some(index) = state.by_content.get_mut(&entry.content) {
                    index.remove(&(entry.start, entry.end, *id));
                    index.is_empty()
                } else {
                    false
                };
                if empty {
                    state.by_content.remove(&entry.content);
                }
            }
        }
    }
}

/// Result of attempting to cancel one hydration waiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HydrationCancellationOutcome {
    /// Cancellation won the terminal race for this waiter.
    Cancelled,
    /// This waiter had already been cancelled.
    AlreadyCancelled,
    /// Completion won before cancellation was applied.
    AlreadyCompleted,
}

/// Cloneable cancellation ownership for exactly one hydration waiter.
#[derive(Clone)]
pub struct HydrationCancellationHandle {
    inner: Arc<WaiterInner>,
}

impl HydrationCancellationHandle {
    /// Attempts to cancel this waiter without cancelling work still owned by other waiters.
    pub fn cancel(&self) -> HydrationCancellationOutcome {
        self.inner.cancel()
    }
}

/// One registered hydration waiter that resolves to exactly its requested logical bytes.
pub struct HydrationWaiter {
    request: HydrationRequest,
    target_start: u64,
    target_end: u64,
    dependencies: Vec<WorkDependency>,
    cancel_rx: oneshot::Receiver<()>,
    inner: Arc<WaiterInner>,
    finished: bool,
}

impl HydrationWaiter {
    /// Returns an independently cloneable cancellation handle for this waiter.
    pub fn cancellation_handle(&self) -> HydrationCancellationHandle {
        HydrationCancellationHandle {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Drives shared backend work and returns only the exact logical bytes requested by this waiter.
    pub async fn wait(mut self) -> HydrationResult<ContentReadResponse> {
        if self.inner.terminal.load(Ordering::Acquire) == TERMINAL_CANCELLED {
            self.finished = true;
            return Err(HydrationError::Cancelled);
        }

        let futures = self
            .dependencies
            .iter()
            .map(|dependency| dependency.future.clone());
        let selected = {
            let work = try_join_all(futures).boxed();
            let cancelled = (&mut self.cancel_rx).map(|_| ()).boxed();
            match select(work, cancelled).await {
                Either::Left((result, _)) => Some(result),
                Either::Right(((), _)) => None,
            }
        };
        let result = match selected {
            Some(result) => {
                if !self.inner.complete() {
                    Err(HydrationError::Cancelled)
                } else {
                    result.and_then(|responses| self.assemble(responses))
                }
            }
            None => Err(HydrationError::Cancelled),
        };
        self.finished = true;
        result
    }

    fn assemble(
        &self,
        responses: Vec<ContentReadResponse>,
    ) -> HydrationResult<ContentReadResponse> {
        if self.target_start == self.target_end {
            return ContentReadResponse::new(
                self.request.read().revision().clone(),
                self.target_start,
                Bytes::new(),
                self.request.read().expected_size(),
            )
            .map_err(Into::into);
        }
        if responses.len() != self.dependencies.len() {
            return Err(CloudFilesCoreError::invalid_content_response(
                "hydration dependency result count changed",
            )
            .into());
        }

        let mut cursor = self.target_start;
        let mut slices = Vec::new();
        for (dependency, response) in self.dependencies.iter().zip(responses) {
            if dependency.end <= cursor || dependency.start >= self.target_end {
                continue;
            }
            let copy_start = cursor.max(dependency.start);
            let copy_end = self.target_end.min(dependency.end);
            let relative_start = copy_start.saturating_sub(response.offset());
            let relative_end = copy_end.saturating_sub(response.offset());
            if relative_end > response.bytes().len() as u64 || relative_start > relative_end {
                return Err(CloudFilesCoreError::invalid_content_response(
                    "hydration dependency did not cover its claimed logical range",
                )
                .into());
            }
            // Both offsets were checked against `Bytes::len()`, so conversion cannot truncate.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "both offsets were bounded by the usize-sized response buffer"
            )]
            let (relative_start, relative_end) = (relative_start as usize, relative_end as usize);
            slices.push(response.bytes().slice(relative_start..relative_end));
            cursor = copy_end;
            if cursor == self.target_end {
                break;
            }
        }
        if cursor != self.target_end {
            return Err(CloudFilesCoreError::invalid_content_response(
                "hydration dependencies left an uncovered logical range",
            )
            .into());
        }

        let bytes = combine_slices(slices)?;
        let response = ContentReadResponse::new(
            self.request.read().revision().clone(),
            self.target_start,
            bytes,
            self.request.read().expected_size(),
        )?;
        self.request.read().validate_response(&response)?;
        Ok(response)
    }
}

impl Drop for HydrationWaiter {
    fn drop(&mut self) {
        if !self.finished {
            self.inner.cancel();
        }
    }
}

fn combine_slices(mut slices: Vec<Bytes>) -> HydrationResult<Bytes> {
    match slices.len() {
        0 => Ok(Bytes::new()),
        1 => Ok(slices.remove(0)),
        _ => {
            let total_len = checked_slice_total(slices.iter().map(Bytes::len))?;
            let mut combined = BytesMut::with_capacity(total_len);
            for bytes in slices {
                combined.extend_from_slice(&bytes);
            }
            Ok(combined.freeze())
        }
    }
}

fn checked_slice_total(lengths: impl IntoIterator<Item = usize>) -> HydrationResult<usize> {
    lengths.into_iter().try_fold(0usize, |total, length| {
        total.checked_add(length).ok_or_else(|| {
            CloudFilesCoreError::invalid_content_response("hydration result length exceeds usize")
                .into()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestBackend;

    #[async_trait::async_trait]
    impl CloudContentBackend for TestBackend {
        async fn read_content(
            &self,
            _request: &ContentReadRequest,
        ) -> crate::BackendResult<ContentReadResponse> {
            Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ))
        }
    }

    fn request(range: ContentReadRange) -> HydrationRequest {
        let scope = crate::CloudScope::new(
            crate::CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
            crate::CloudRootId::new("root").expect("root fixture should be valid"),
        );
        let key = CloudItemKey::new(
            scope,
            crate::CloudItemId::new("item").expect("item fixture should be valid"),
        );
        let revision = ContentRevision::from_slice(b"content-v1")
            .expect("content revision fixture should be valid");
        let read = match range {
            ContentReadRange::Whole => ContentReadRequest::whole(key, revision, 4),
            ContentReadRange::Range(range) => ContentReadRequest::range(key, revision, 4, range),
        };
        HydrationRequest::new(
            read,
            Alignment::ONE,
            SessionGeneration::new(1).expect("generation fixture should be valid"),
        )
    }

    fn response(offset: u64, bytes: &'static [u8]) -> ContentReadResponse {
        ContentReadResponse::new(
            ContentRevision::from_slice(b"content-v1")
                .expect("content revision fixture should be valid"),
            offset,
            Bytes::from_static(bytes),
            4,
        )
        .expect("response fixture should be valid")
    }

    fn dependency(id: u64, start: u64, end: u64) -> WorkDependency {
        WorkDependency {
            id,
            start,
            end,
            future: futures::future::ready(Err(HydrationError::Cancelled))
                .boxed()
                .shared(),
        }
    }

    fn waiter(
        request: HydrationRequest,
        target_start: u64,
        target_end: u64,
        dependencies: Vec<WorkDependency>,
    ) -> HydrationWaiter {
        let (_, cancel_rx) = oneshot::channel();
        HydrationWaiter {
            request,
            target_start,
            target_end,
            dependencies,
            cancel_rx,
            inner: Arc::new(WaiterInner {
                terminal: AtomicU8::new(TERMINAL_PENDING),
                released: AtomicBool::new(false),
                coordinator: Weak::new(),
                work_ids: Vec::new(),
                cancel_tx: Mutex::new(None),
            }),
            finished: false,
        }
    }

    #[test]
    fn poisoned_coordinator_mutex_recovers_its_inner_state() {
        let mutex = Arc::new(Mutex::new(7usize));
        let poison_target = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poison_target.lock().expect("mutex should initially lock");
            panic!("poison coordinator fixture");
        })
        .join();

        assert_eq!(*lock(&mutex), 7);
    }

    #[test]
    fn work_identity_and_alignment_overflow_are_contract_failures() {
        let mut state = CoordinatorState {
            next_work_id: u64::MAX,
            ..CoordinatorState::default()
        };
        let scope = crate::CloudScope::new(
            crate::CloudNamespaceId::new("namespace").expect("namespace fixture should be valid"),
            crate::CloudRootId::new("root").expect("root fixture should be valid"),
        );
        let key = CloudItemKey::new(
            scope,
            crate::CloudItemId::new("item").expect("item fixture should be valid"),
        );
        let revision = ContentRevision::from_slice(b"content-v1")
            .expect("content revision fixture should be valid");
        let request = HydrationRequest::whole(
            key,
            revision,
            1,
            SessionGeneration::new(1).expect("generation fixture should be valid"),
        );
        let content = HydrationContentKey::from_request(&request);
        assert!(futures::executor::block_on(TestBackend.read_content(request.read())).is_err());

        assert!(matches!(
            insert_work(
                &mut state,
                Arc::new(TestBackend),
                content,
                request.read().clone(),
                0,
                1,
                true,
            ),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));
        assert!(matches!(
            aligned_extent(
                u64::MAX - 1,
                u64::MAX - 1,
                u64::MAX,
                Alignment::new(8).expect("alignment fixture should be valid"),
            ),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidByteRange { .. }
            ))
        ));
    }

    #[test]
    fn coordinator_propagates_work_identity_and_alignment_failures_from_each_request_shape() {
        let coordinator = HydrationCoordinator {
            backend: Arc::new(TestBackend),
            state: Arc::new(Mutex::new(CoordinatorState {
                next_work_id: u64::MAX,
                ..CoordinatorState::default()
            })),
            limits: HydrationLimits::DEFAULT,
        };

        let whole = request(ContentReadRange::Whole);
        assert!(matches!(
            coordinator.request(whole),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));

        let empty = request(ContentReadRange::Range(
            ByteRange::new(4, 1).expect("range fixture should be valid"),
        ));
        assert!(matches!(
            coordinator.request(empty),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));

        let range = request(ContentReadRange::Range(
            ByteRange::new(0, 1).expect("range fixture should be valid"),
        ));
        assert!(matches!(
            coordinator.request(range),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));

        let overflow = HydrationRequest::range(
            request(ContentReadRange::Whole).read().key().clone(),
            ContentRevision::from_slice(b"content-v1")
                .expect("content revision fixture should be valid"),
            u64::MAX,
            ByteRange::new(u64::MAX - 2, 1).expect("range fixture should be valid"),
            Alignment::new(8).expect("alignment fixture should be valid"),
            SessionGeneration::new(1).expect("generation fixture should be valid"),
        );
        assert!(matches!(
            coordinator.request(overflow),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidByteRange { .. }
            ))
        ));
    }

    #[test]
    fn release_work_is_idempotent_and_tolerates_missing_internal_indexes() {
        let empty_state = Arc::new(Mutex::new(CoordinatorState::default()));
        let repeated = WaiterInner {
            terminal: AtomicU8::new(TERMINAL_PENDING),
            released: AtomicBool::new(false),
            coordinator: Arc::downgrade(&empty_state),
            work_ids: Vec::new(),
            cancel_tx: Mutex::new(None),
        };
        repeated.release_work();
        repeated.release_work();

        let missing_work = WaiterInner {
            terminal: AtomicU8::new(TERMINAL_PENDING),
            released: AtomicBool::new(false),
            coordinator: Arc::downgrade(&empty_state),
            work_ids: vec![7],
            cancel_tx: Mutex::new(None),
        };
        missing_work.release_work();

        let request = request(ContentReadRange::Whole);
        let content = HydrationContentKey::from_request(&request);
        let mut state = CoordinatorState::default();
        state.work.insert(
            9,
            WorkEntry {
                content,
                start: 0,
                end: 4,
                whole: true,
                future: futures::future::ready(Err(HydrationError::Cancelled))
                    .boxed()
                    .shared(),
                waiter_count: 1,
            },
        );
        let missing_index_state = Arc::new(Mutex::new(state));
        let missing_index = WaiterInner {
            terminal: AtomicU8::new(TERMINAL_PENDING),
            released: AtomicBool::new(false),
            coordinator: Arc::downgrade(&missing_index_state),
            work_ids: vec![9],
            cancel_tx: Mutex::new(None),
        };
        missing_index.release_work();
        assert!(lock(&missing_index_state).work.is_empty());
    }

    #[test]
    fn assembly_rejects_changed_missing_and_short_dependencies() {
        let request = request(ContentReadRange::Range(
            ByteRange::new(0, 4).expect("range fixture should be valid"),
        ));

        let changed = waiter(request.clone(), 0, 4, vec![dependency(1, 0, 4)]);
        assert!(matches!(
            changed.assemble(Vec::new()),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));

        let missing = waiter(request.clone(), 0, 4, vec![dependency(2, 4, 4)]);
        assert!(matches!(
            missing.assemble(vec![response(4, b"")]),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));

        let short = waiter(request.clone(), 0, 4, vec![dependency(3, 0, 4)]);
        assert!(matches!(
            short.assemble(vec![response(0, b"ab")]),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));

        let complete = waiter(request, 0, 4, vec![dependency(4, 0, 4)]);
        assert_eq!(
            complete
                .assemble(vec![response(0, b"abcd")])
                .expect("complete dependency should assemble")
                .bytes()
                .as_ref(),
            b"abcd"
        );
        assert!(
            combine_slices(Vec::new())
                .expect("empty slices should combine")
                .is_empty()
        );
    }

    #[test]
    fn slice_length_accumulator_rejects_usize_overflow() {
        assert!(matches!(
            checked_slice_total([usize::MAX, 1]),
            Err(HydrationError::Contract(
                CloudFilesCoreError::InvalidContentResponse { .. }
            ))
        ));
    }
}

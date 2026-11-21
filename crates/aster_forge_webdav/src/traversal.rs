//! Bounded directory paging and recursive traversal resource accounting.

use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    DavBackendError, DavDirectoryEntry, DavDirectoryEnumerator, DavDirectoryPage,
    DavDirectoryPageRequest, DavPath,
};
use bytes::Bytes;

/// Cancellation checkpoint controlled by the product or transport adapter.
pub trait DavCancellation: Send + Sync {
    /// Reports whether the owning transport or product has requested cancellation.
    fn is_cancelled(&self) -> bool;
}

/// Cloneable cancellation source shared by a transport response and product backend work.
///
/// Dropping a clone does not cancel the operation. The owning transport or request lifecycle must
/// call [`Self::cancel`] when the response body is abandoned or its execution budget expires.
#[derive(Debug, Clone, Default)]
pub struct DavCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl DavCancellationToken {
    /// Creates a token in the not-cancelled state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation; every later checkpoint observes it.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Reports whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl DavCancellation for DavCancellationToken {
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

/// Cancellation checkpoint used when the caller has no cancellation source.
#[derive(Debug, Clone, Copy, Default)]
pub struct DavNeverCancelled;

impl DavCancellation for DavNeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Maximum directory entries accepted from one product page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavDirectoryPageLimits {
    /// Maximum entries accepted from one product page.
    pub maximum_entries: usize,
    /// Maximum page requests performed for one enumeration.
    pub maximum_pages: usize,
}

impl DavDirectoryPageLimits {
    /// Creates non-zero directory page limits.
    ///
    /// # Errors
    ///
    /// Returns [`DavDirectoryReadError`] when either directory-page limit is zero.
    pub const fn new(
        maximum_entries: usize,
        maximum_pages: usize,
    ) -> Result<Self, DavDirectoryReadError> {
        if maximum_entries == 0 || maximum_pages == 0 {
            Err(DavDirectoryReadError::InvalidLimit)
        } else {
            Ok(Self {
                maximum_entries,
                maximum_pages,
            })
        }
    }
}

/// Invalid product page classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavDirectoryPageValidationError {
    /// Configured page entry bound is zero.
    #[error("directory page size limit must be non-zero")]
    InvalidLimit,
    /// Page carries more entries than the request allowed.
    #[error("directory page exceeds the requested entry limit")]
    TooManyEntries,
    /// Page returned no entries but still claims a continuation cursor.
    #[error("empty directory page cannot carry a continuation cursor")]
    EmptyContinuation,
    /// Continuation cursor repeats a cursor already observed.
    #[error("directory continuation cursor did not advance")]
    CursorLoop,
    /// Page contains an entry whose stable key is empty.
    #[error("directory page contains an empty stable key")]
    EmptyStableKey,
    /// Page contains an entry key equal to one already returned.
    #[error("directory page contains a duplicate entry key")]
    DuplicateEntry,
    /// Page entries do not continue the stable ascending order.
    #[error("directory page entries are not in stable ascending order")]
    OutOfOrderEntry,
}

/// Validates product page bounds, ordering and cursor progress before protocol consumption.
///
/// # Errors
///
/// Returns an error when limits, ordering, entries, or cursor progress are invalid.
pub fn validate_directory_page<E, C, S>(
    current_cursor: Option<&C>,
    seen_cursors: &HashSet<C, S>,
    previous_stable_key: Option<&[u8]>,
    maximum_entries: usize,
    page: &DavDirectoryPage<E, C>,
) -> Result<(), DavDirectoryPageValidationError>
where
    E: DavDirectoryEntry,
    C: Eq + Hash,
    S: BuildHasher,
{
    if maximum_entries == 0 {
        return Err(DavDirectoryPageValidationError::InvalidLimit);
    }
    if page.entries.len() > maximum_entries {
        return Err(DavDirectoryPageValidationError::TooManyEntries);
    }
    if page.entries.is_empty() && page.next_cursor.is_some() {
        return Err(DavDirectoryPageValidationError::EmptyContinuation);
    }
    if page
        .next_cursor
        .as_ref()
        .is_some_and(|cursor| current_cursor == Some(cursor) || seen_cursors.contains(cursor))
    {
        return Err(DavDirectoryPageValidationError::CursorLoop);
    }
    let mut previous: Option<&[u8]> = None;
    for entry in &page.entries {
        let key = entry.stable_key();
        if key.is_empty() {
            return Err(DavDirectoryPageValidationError::EmptyStableKey);
        }
        if previous.is_none()
            && let Some(previous_page_key) = previous_stable_key
        {
            match previous_page_key.cmp(key) {
                std::cmp::Ordering::Equal => {
                    return Err(DavDirectoryPageValidationError::DuplicateEntry);
                }
                std::cmp::Ordering::Greater => {
                    return Err(DavDirectoryPageValidationError::OutOfOrderEntry);
                }
                std::cmp::Ordering::Less => {}
            }
        }
        if let Some(previous) = previous {
            match previous.cmp(key) {
                std::cmp::Ordering::Equal => {
                    return Err(DavDirectoryPageValidationError::DuplicateEntry);
                }
                std::cmp::Ordering::Greater => {
                    return Err(DavDirectoryPageValidationError::OutOfOrderEntry);
                }
                std::cmp::Ordering::Less => {}
            }
        }
        previous = Some(key);
    }
    Ok(())
}

/// Opaque cursor state retained between bounded directory page requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavDirectoryPageState<C: Eq + Hash> {
    current_cursor: Option<C>,
    seen_cursors: HashSet<C>,
    last_stable_key: Option<Bytes>,
    poisoned: Option<DavDirectoryPageValidationError>,
    finished: bool,
    pages_read: usize,
}

impl<C: Eq + Hash> DavDirectoryPageState<C> {
    /// Creates empty paging state for one enumeration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            current_cursor: None,
            seen_cursors: HashSet::new(),
            last_stable_key: None,
            poisoned: None,
            finished: false,
            pages_read: 0,
        }
    }

    /// Returns the continuation cursor for the next backend request, if any.
    #[must_use]
    pub fn cursor(&self) -> Option<&C> {
        self.current_cursor.as_ref()
    }

    /// Reports whether the enumeration reached its last page.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Returns how many validated pages this state has read.
    #[must_use]
    pub const fn pages_read(&self) -> usize {
        self.pages_read
    }
}

impl<C: Eq + Hash> Default for DavDirectoryPageState<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// Validated entries returned by one bounded page request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavValidatedDirectoryPage<E> {
    /// Entries that passed bounds, ordering, and key validation.
    pub entries: Vec<E>,
    /// Whether a continuation cursor remains for the next request.
    pub has_more: bool,
}

/// Failure while requesting the next bounded directory page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavDirectoryReadError {
    /// Requested entries or configured page limits are zero.
    #[error("directory page size limit must be non-zero")]
    InvalidLimit,
    /// Cancellation checkpoint fired before the page was requested.
    #[error("directory enumeration was cancelled")]
    Cancelled,
    /// The enumeration already consumed its configured page budget.
    #[error("directory page limit exceeded")]
    PageLimitExceeded,
    /// Backend enumeration call failed.
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    /// Product page violated the validation contract.
    #[error(transparent)]
    InvalidPage(#[from] DavDirectoryPageValidationError),
}

/// Requests and validates the next directory page.
///
/// Cancellation is checked before every backend call. The product cursor remains opaque and is
/// moved into the state only after the page passes all validation. An invalid product page poisons
/// the state so subsequent calls return the same validation error without repeating the backend
/// request.
///
/// # Errors
///
/// Returns an error on cancellation, backend failure, exhausted limits, or an invalid page.
pub async fn read_next_directory_page<E, C>(
    enumerator: &E,
    path: &DavPath,
    state: &mut DavDirectoryPageState<E::Cursor>,
    requested_entries: usize,
    limits: DavDirectoryPageLimits,
    cancellation: &C,
) -> Result<Option<DavValidatedDirectoryPage<E::Entry>>, DavDirectoryReadError>
where
    E: DavDirectoryEnumerator,
    C: DavCancellation,
{
    if let Some(error) = state.poisoned {
        return Err(DavDirectoryReadError::InvalidPage(error));
    }
    if state.finished {
        return Ok(None);
    }
    if requested_entries == 0 || limits.maximum_entries == 0 || limits.maximum_pages == 0 {
        return Err(DavDirectoryReadError::InvalidLimit);
    }
    if cancellation.is_cancelled() {
        return Err(DavDirectoryReadError::Cancelled);
    }
    if state.pages_read >= limits.maximum_pages {
        return Err(DavDirectoryReadError::PageLimitExceeded);
    }
    let maximum_entries = requested_entries.min(limits.maximum_entries);
    let page = enumerator
        .read_directory_page(DavDirectoryPageRequest {
            path,
            cursor: state.current_cursor.as_ref(),
            maximum_entries,
        })
        .await?;
    if let Err(error) = validate_directory_page(
        state.current_cursor.as_ref(),
        &state.seen_cursors,
        state.last_stable_key.as_deref(),
        maximum_entries,
        &page,
    ) {
        state.poisoned = Some(error);
        return Err(DavDirectoryReadError::InvalidPage(error));
    }

    let has_more = page.next_cursor.is_some();
    if let Some(last) = page.entries.last() {
        state.last_stable_key = Some(Bytes::copy_from_slice(last.stable_key()));
    }
    if let Some(cursor) = page.next_cursor {
        if let Some(previous) = state.current_cursor.replace(cursor) {
            state.seen_cursors.insert(previous);
        }
    } else {
        state.current_cursor = None;
    }
    state.finished = !has_more;
    state.pages_read += 1;
    Ok(Some(DavValidatedDirectoryPage {
        entries: page.entries,
        has_more,
    }))
}

/// Hard limits for recursive resource traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavTraversalLimits {
    /// Upper bound on resources one traversal may visit.
    pub maximum_visited_resources: usize,
    /// Upper bound on work items reserved in the traversal queue at once.
    pub maximum_queued_work_items: usize,
    /// Upper bound on recorded per-resource failures before the traversal stops.
    pub maximum_failures: usize,
    /// Upper bound on recursion depth; `None` means unlimited up to overflow guards.
    pub maximum_depth: Option<usize>,
}

impl DavTraversalLimits {
    /// Creates traversal limits with all four bounds supplied explicitly.
    #[must_use]
    pub const fn new(
        maximum_visited_resources: usize,
        maximum_queued_work_items: usize,
        maximum_failures: usize,
        maximum_depth: Option<usize>,
    ) -> Self {
        Self {
            maximum_visited_resources,
            maximum_queued_work_items,
            maximum_failures,
            maximum_depth,
        }
    }
}

/// Current recursive traversal accounting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DavTraversalProgress {
    /// Resources visited so far.
    pub visited_resources: usize,
    /// Work items currently reserved in the queue.
    pub queued_work_items: usize,
    /// Per-resource failures recorded so far.
    pub failures: usize,
    /// Mutation commits that succeeded so far.
    pub completed_mutations: usize,
}

/// Typed recursive traversal stop reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavTraversalErrorKind {
    /// A required traversal limit is zero.
    #[error("invalid recursive traversal limits")]
    InvalidLimits,
    /// Cancellation checkpoint observed a cancellation request.
    #[error("recursive traversal was cancelled")]
    Cancelled,
    /// Visited more resources than the limit allows.
    #[error("recursive traversal visited-resource limit exceeded")]
    VisitedResourceLimitExceeded,
    /// Reserved more queued work than the limit allows.
    #[error("recursive traversal work-queue limit exceeded")]
    QueuedWorkLimitExceeded,
    /// Recorded more failures than the limit allows.
    #[error("recursive traversal failure limit exceeded")]
    FailureLimitExceeded,
    /// Descended deeper than the configured maximum depth.
    #[error("recursive traversal depth limit exceeded")]
    DepthLimitExceeded,
}

/// Recursive traversal failure with partial-execution accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct DavTraversalError {
    /// Typed stop reason.
    pub kind: DavTraversalErrorKind,
    /// Accounting snapshot captured when the failure was raised.
    pub progress: DavTraversalProgress,
}

impl DavTraversalError {
    /// Reports whether at least one mutation committed before the failure.
    #[must_use]
    pub const fn partial_execution(self) -> bool {
        self.progress.completed_mutations != 0
    }
}

/// Allocation-free recursive traversal budget state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavTraversalBudget {
    limits: DavTraversalLimits,
    progress: DavTraversalProgress,
}

impl DavTraversalBudget {
    ///
    /// # Errors
    ///
    /// Returns [`DavTraversalError`] when any required traversal limit is zero.
    pub fn new(limits: DavTraversalLimits) -> Result<Self, DavTraversalError> {
        if limits.maximum_visited_resources == 0
            || limits.maximum_queued_work_items == 0
            || limits.maximum_failures == 0
        {
            return Err(DavTraversalError {
                kind: DavTraversalErrorKind::InvalidLimits,
                progress: DavTraversalProgress::default(),
            });
        }
        Ok(Self {
            limits,
            progress: DavTraversalProgress::default(),
        })
    }

    /// Returns the accounting snapshot observed so far.
    #[must_use]
    pub const fn progress(self) -> DavTraversalProgress {
        self.progress
    }

    ///
    /// # Errors
    ///
    /// Returns [`DavTraversalError`] when cancellation has been requested.
    pub fn checkpoint(&self, cancellation: &impl DavCancellation) -> Result<(), DavTraversalError> {
        if cancellation.is_cancelled() {
            Err(self.error(DavTraversalErrorKind::Cancelled))
        } else {
            Ok(())
        }
    }

    ///
    /// # Errors
    ///
    /// Returns [`DavTraversalError`] when depth or visited-resource limits are exceeded.
    pub fn visit(&mut self, depth: usize) -> Result<(), DavTraversalError> {
        if self
            .limits
            .maximum_depth
            .is_some_and(|maximum| depth > maximum)
        {
            return Err(self.error(DavTraversalErrorKind::DepthLimitExceeded));
        }
        self.progress.visited_resources = checked_increment(
            self.progress.visited_resources,
            self.limits.maximum_visited_resources,
        )
        .ok_or_else(|| self.error(DavTraversalErrorKind::VisitedResourceLimitExceeded))?;
        Ok(())
    }

    ///
    /// # Errors
    ///
    /// Returns [`DavTraversalError`] when the queued-work counter would exceed its limit.
    pub fn reserve_work(&mut self, additional: usize) -> Result<(), DavTraversalError> {
        let Some(queued) = self.progress.queued_work_items.checked_add(additional) else {
            return Err(self.error(DavTraversalErrorKind::QueuedWorkLimitExceeded));
        };
        if queued > self.limits.maximum_queued_work_items {
            return Err(self.error(DavTraversalErrorKind::QueuedWorkLimitExceeded));
        }
        self.progress.queued_work_items = queued;
        Ok(())
    }

    /// Releases one reserved work item after it has been dequeued.
    pub fn complete_work(&mut self) {
        self.progress.queued_work_items = self.progress.queued_work_items.saturating_sub(1);
    }

    ///
    /// # Errors
    ///
    /// Returns [`DavTraversalError`] when the failure counter would exceed its limit.
    pub fn record_failure(&mut self) -> Result<(), DavTraversalError> {
        self.progress.failures =
            checked_increment(self.progress.failures, self.limits.maximum_failures)
                .ok_or_else(|| self.error(DavTraversalErrorKind::FailureLimitExceeded))?;
        Ok(())
    }

    /// Records one successful mutation commit for partial-execution accounting.
    pub fn record_completed_mutation(&mut self) {
        self.progress.completed_mutations = self.progress.completed_mutations.saturating_add(1);
    }

    const fn error(&self, kind: DavTraversalErrorKind) -> DavTraversalError {
        DavTraversalError {
            kind,
            progress: self.progress,
        }
    }
}

fn checked_increment(value: usize, maximum: usize) -> Option<usize> {
    value.checked_add(1).filter(|next| *next <= maximum)
}

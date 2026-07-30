use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavCancellation, DavDirectoryEntry,
    DavDirectoryEnumerator, DavDirectoryPage, DavDirectoryPageLimits, DavDirectoryPageRequest,
    DavDirectoryPageState, DavDirectoryPageValidationError, DavDirectoryReadError, DavMetaData,
    DavNeverCancelled, DavPath, DavTraversalBudget, DavTraversalErrorKind, DavTraversalLimits,
    FsError, read_next_directory_page, validate_directory_page,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestMeta;

impl DavMetaData for TestMeta {
    fn len(&self) -> u64 {
        0
    }

    fn modified(&self) -> Result<SystemTime, FsError> {
        Ok(SystemTime::UNIX_EPOCH)
    }

    fn is_dir(&self) -> bool {
        false
    }

    fn etag(&self) -> Option<String> {
        None
    }

    fn created(&self) -> Result<SystemTime, FsError> {
        Ok(SystemTime::UNIX_EPOCH)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestEntry {
    name: Vec<u8>,
    stable_key: Vec<u8>,
    metadata: TestMeta,
}

impl TestEntry {
    fn new(name: &str) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            stable_key: name.as_bytes().to_vec(),
            metadata: TestMeta,
        }
    }

    fn with_key(name: &str, stable_key: &str) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            stable_key: stable_key.as_bytes().to_vec(),
            metadata: TestMeta,
        }
    }
}

impl DavDirectoryEntry for TestEntry {
    type Metadata = TestMeta;

    fn name(&self) -> &[u8] {
        &self.name
    }

    fn metadata(&self) -> &Self::Metadata {
        &self.metadata
    }

    fn stable_key(&self) -> &[u8] {
        &self.stable_key
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NameKeyEntry {
    name: Vec<u8>,
    metadata: TestMeta,
}

impl NameKeyEntry {
    fn new(name: &str) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            metadata: TestMeta,
        }
    }
}

impl DavDirectoryEntry for NameKeyEntry {
    type Metadata = TestMeta;

    fn name(&self) -> &[u8] {
        &self.name
    }

    fn metadata(&self) -> &Self::Metadata {
        &self.metadata
    }
}

fn page_limits(maximum_entries: usize, maximum_pages: usize) -> DavDirectoryPageLimits {
    DavDirectoryPageLimits::new(maximum_entries, maximum_pages).expect("valid page limits")
}

#[test]
fn page_validation_covers_exact_limit_cursor_and_order_boundaries() {
    assert_eq!(
        DavDirectoryPageLimits::new(0, 1),
        Err(DavDirectoryReadError::InvalidLimit)
    );
    assert_eq!(
        DavDirectoryPageLimits::new(1, 0),
        Err(DavDirectoryReadError::InvalidLimit)
    );
    let valid = DavDirectoryPage {
        entries: vec![TestEntry::new("a"), TestEntry::new("b")],
        next_cursor: Some(1_u64),
    };
    validate_directory_page(None, &HashSet::new(), None, 2, &valid).expect("exact page limit");

    let cases = [
        (
            0,
            DavDirectoryPage {
                entries: vec![],
                next_cursor: None,
            },
            DavDirectoryPageValidationError::InvalidLimit,
        ),
        (
            1,
            valid.clone(),
            DavDirectoryPageValidationError::TooManyEntries,
        ),
        (
            2,
            DavDirectoryPage {
                entries: vec![],
                next_cursor: Some(2),
            },
            DavDirectoryPageValidationError::EmptyContinuation,
        ),
        (
            2,
            DavDirectoryPage {
                entries: vec![TestEntry::with_key("a", "")],
                next_cursor: None,
            },
            DavDirectoryPageValidationError::EmptyStableKey,
        ),
        (
            2,
            DavDirectoryPage {
                entries: vec![TestEntry::new("a"), TestEntry::new("a")],
                next_cursor: None,
            },
            DavDirectoryPageValidationError::DuplicateEntry,
        ),
        (
            2,
            DavDirectoryPage {
                entries: vec![TestEntry::new("b"), TestEntry::new("a")],
                next_cursor: None,
            },
            DavDirectoryPageValidationError::OutOfOrderEntry,
        ),
    ];
    for (maximum, page, expected) in cases {
        assert_eq!(
            validate_directory_page(None, &HashSet::new(), None, maximum, &page),
            Err(expected)
        );
    }

    assert_eq!(
        validate_directory_page(Some(&1_u64), &HashSet::new(), None, 2, &valid),
        Err(DavDirectoryPageValidationError::CursorLoop)
    );
    assert_eq!(
        validate_directory_page(
            None,
            &HashSet::new(),
            Some(b"b"),
            1,
            &DavDirectoryPage::<TestEntry, u64> {
                entries: vec![TestEntry::new("a")],
                next_cursor: None,
            },
        ),
        Err(DavDirectoryPageValidationError::OutOfOrderEntry)
    );
    validate_directory_page(
        Some(&1_u64),
        &HashSet::new(),
        None,
        2,
        &DavDirectoryPage::<TestEntry, u64> {
            entries: vec![],
            next_cursor: None,
        },
    )
    .expect("empty final page is valid");
}

#[test]
fn default_stable_key_uses_name_for_duplicate_and_order_validation() {
    let valid = DavDirectoryPage::<NameKeyEntry, u64> {
        entries: vec![NameKeyEntry::new("a"), NameKeyEntry::new("b")],
        next_cursor: None,
    };
    validate_directory_page(None, &HashSet::new(), None, 2, &valid)
        .expect("names should provide stable ascending keys");

    for (entries, expected) in [
        (
            vec![NameKeyEntry::new("a"), NameKeyEntry::new("a")],
            DavDirectoryPageValidationError::DuplicateEntry,
        ),
        (
            vec![NameKeyEntry::new("b"), NameKeyEntry::new("a")],
            DavDirectoryPageValidationError::OutOfOrderEntry,
        ),
    ] {
        assert_eq!(
            validate_directory_page(
                None,
                &HashSet::new(),
                None,
                2,
                &DavDirectoryPage::<NameKeyEntry, u64> {
                    entries,
                    next_cursor: None,
                },
            ),
            Err(expected)
        );
    }
}

struct RecordingEnumerator {
    pages: Mutex<VecDeque<Result<DavDirectoryPage<TestEntry, u64>, DavBackendError>>>,
    calls: Mutex<Vec<(Option<u64>, usize)>>,
}

impl RecordingEnumerator {
    fn new(pages: impl IntoIterator<Item = DavDirectoryPage<TestEntry, u64>>) -> Self {
        Self {
            pages: Mutex::new(pages.into_iter().map(Ok).collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl DavDirectoryEnumerator for RecordingEnumerator {
    type Cursor = u64;
    type Entry = TestEntry;

    async fn read_directory_page<'a>(
        &'a self,
        request: DavDirectoryPageRequest<'a, Self::Cursor>,
    ) -> Result<DavDirectoryPage<Self::Entry, Self::Cursor>, DavBackendError> {
        self.calls
            .lock()
            .expect("call log")
            .push((request.cursor.copied(), request.maximum_entries));
        self.pages
            .lock()
            .expect("page queue")
            .pop_front()
            .unwrap_or_else(|| Err(DavBackendError::new(DavBackendErrorKind::Internal)))
    }
}

struct ToggleCancellation {
    cancelled: AtomicBool,
    checks: AtomicUsize,
}

impl ToggleCancellation {
    const fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            checks: AtomicUsize::new(0),
        }
    }
}

impl DavCancellation for ToggleCancellation {
    fn is_cancelled(&self) -> bool {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[test]
fn pager_applies_product_limit_preserves_cursor_and_stops_after_completion() {
    futures::executor::block_on(async {
        let enumerator = RecordingEnumerator::new([
            DavDirectoryPage {
                entries: vec![TestEntry::new("a"), TestEntry::new("b")],
                next_cursor: Some(7),
            },
            DavDirectoryPage {
                entries: vec![TestEntry::new("c")],
                next_cursor: None,
            },
        ]);
        let path = DavPath::new("/docs/").expect("path");
        let mut state = DavDirectoryPageState::default();
        assert_eq!(state, DavDirectoryPageState::new());
        let limits = page_limits(2, 2);

        let first = read_next_directory_page(
            &enumerator,
            &path,
            &mut state,
            10,
            limits,
            &DavNeverCancelled,
        )
        .await
        .expect("first page")
        .expect("page");
        assert_eq!(first.entries.len(), 2);
        assert!(first.has_more);
        assert_eq!(state.cursor(), Some(&7));

        let second = read_next_directory_page(
            &enumerator,
            &path,
            &mut state,
            10,
            limits,
            &DavNeverCancelled,
        )
        .await
        .expect("second page")
        .expect("page");
        assert_eq!(second.entries[0].name(), b"c");
        assert!(!second.has_more);
        assert!(state.is_finished());
        assert_eq!(state.pages_read(), 2);
        assert!(
            read_next_directory_page(
                &enumerator,
                &path,
                &mut state,
                10,
                limits,
                &DavNeverCancelled,
            )
            .await
            .expect("completed pager")
            .is_none()
        );
        assert_eq!(
            enumerator.calls.lock().expect("calls").as_slice(),
            &[(None, 2), (Some(7), 2)]
        );
    });
}

#[test]
fn pager_checks_cancellation_before_requesting_the_next_page() {
    futures::executor::block_on(async {
        let enumerator = RecordingEnumerator::new([
            DavDirectoryPage {
                entries: vec![TestEntry::new("a")],
                next_cursor: Some(1),
            },
            DavDirectoryPage {
                entries: vec![TestEntry::new("b")],
                next_cursor: None,
            },
        ]);
        let path = DavPath::new("/docs/").expect("path");
        let mut state = DavDirectoryPageState::new();
        let cancellation = ToggleCancellation::new();
        read_next_directory_page(
            &enumerator,
            &path,
            &mut state,
            1,
            page_limits(1, 2),
            &cancellation,
        )
        .await
        .expect("first page");
        cancellation.cancelled.store(true, Ordering::SeqCst);
        assert_eq!(
            read_next_directory_page(
                &enumerator,
                &path,
                &mut state,
                1,
                page_limits(1, 2),
                &cancellation,
            )
            .await,
            Err(DavDirectoryReadError::Cancelled)
        );
        assert_eq!(enumerator.calls.lock().expect("calls").len(), 1);
        assert_eq!(state.cursor(), Some(&1));
        assert_eq!(cancellation.checks.load(Ordering::SeqCst), 2);
    });
}

#[test]
fn pager_preserves_state_on_invalid_limit_page_or_backend_failure() {
    futures::executor::block_on(async {
        let path = DavPath::new("/docs/").expect("path");
        let enumerator = RecordingEnumerator::new([DavDirectoryPage {
            entries: vec![TestEntry::new("b"), TestEntry::new("a")],
            next_cursor: None,
        }]);
        let mut state = DavDirectoryPageState::new();
        assert_eq!(
            read_next_directory_page(
                &enumerator,
                &path,
                &mut state,
                0,
                page_limits(1, 1),
                &DavNeverCancelled,
            )
            .await,
            Err(DavDirectoryReadError::InvalidLimit)
        );
        assert!(enumerator.calls.lock().expect("calls").is_empty());

        assert_eq!(
            read_next_directory_page(
                &enumerator,
                &path,
                &mut state,
                2,
                page_limits(2, 1),
                &DavNeverCancelled,
            )
            .await,
            Err(DavDirectoryReadError::InvalidPage(
                DavDirectoryPageValidationError::OutOfOrderEntry
            ))
        );
        assert_eq!(enumerator.calls.lock().expect("calls").len(), 1);
        assert_eq!(state.cursor(), None);
        assert_eq!(state.pages_read(), 0);
        assert_eq!(
            read_next_directory_page(
                &enumerator,
                &path,
                &mut state,
                2,
                page_limits(2, 1),
                &DavNeverCancelled,
            )
            .await,
            Err(DavDirectoryReadError::InvalidPage(
                DavDirectoryPageValidationError::OutOfOrderEntry
            ))
        );
        assert_eq!(enumerator.calls.lock().expect("calls").len(), 1);

        let backend = RecordingEnumerator {
            pages: Mutex::new(VecDeque::from([
                Err(DavBackendError::new(DavBackendErrorKind::Forbidden)),
                Ok(DavDirectoryPage {
                    entries: vec![TestEntry::new("recovered")],
                    next_cursor: None,
                }),
            ])),
            calls: Mutex::new(Vec::new()),
        };
        let mut backend_state = DavDirectoryPageState::new();
        assert_eq!(
            read_next_directory_page(
                &backend,
                &path,
                &mut backend_state,
                1,
                page_limits(1, 1),
                &DavNeverCancelled,
            )
            .await,
            Err(DavDirectoryReadError::Backend(DavBackendError::new(
                DavBackendErrorKind::Forbidden
            )))
        );
        assert_eq!(backend_state, DavDirectoryPageState::new());

        let recovered = read_next_directory_page(
            &backend,
            &path,
            &mut backend_state,
            1,
            page_limits(1, 1),
            &DavNeverCancelled,
        )
        .await
        .expect("backend retry")
        .expect("recovered page");
        assert_eq!(recovered.entries[0].name(), b"recovered");
        assert!(!recovered.has_more);
        assert_eq!(
            backend.calls.lock().expect("calls").as_slice(),
            &[(None, 1), (None, 1)]
        );
        assert_eq!(backend_state.pages_read(), 1);
        assert!(backend_state.is_finished());
    });
}

#[test]
fn pager_rejects_cross_page_order_regressions_cursor_cycles_and_page_exhaustion() {
    futures::executor::block_on(async {
        let path = DavPath::new("/docs/").expect("path");
        let cross_page = RecordingEnumerator::new([
            DavDirectoryPage {
                entries: vec![TestEntry::new("b")],
                next_cursor: Some(1),
            },
            DavDirectoryPage {
                entries: vec![TestEntry::new("b")],
                next_cursor: None,
            },
        ]);
        let mut state = DavDirectoryPageState::new();
        let limits = page_limits(1, 2);
        read_next_directory_page(
            &cross_page,
            &path,
            &mut state,
            1,
            limits,
            &DavNeverCancelled,
        )
        .await
        .expect("first page");
        assert_eq!(
            read_next_directory_page(
                &cross_page,
                &path,
                &mut state,
                1,
                limits,
                &DavNeverCancelled,
            )
            .await,
            Err(DavDirectoryReadError::InvalidPage(
                DavDirectoryPageValidationError::DuplicateEntry
            ))
        );
        assert_eq!(state.cursor(), Some(&1));
        assert_eq!(state.pages_read(), 1);

        let cursor_cycle = RecordingEnumerator::new([
            DavDirectoryPage {
                entries: vec![TestEntry::new("a")],
                next_cursor: Some(1),
            },
            DavDirectoryPage {
                entries: vec![TestEntry::new("b")],
                next_cursor: Some(2),
            },
            DavDirectoryPage {
                entries: vec![TestEntry::new("c")],
                next_cursor: Some(1),
            },
        ]);
        let mut state = DavDirectoryPageState::new();
        let limits = page_limits(1, 3);
        for _ in 0..2 {
            read_next_directory_page(
                &cursor_cycle,
                &path,
                &mut state,
                1,
                limits,
                &DavNeverCancelled,
            )
            .await
            .expect("acyclic page");
        }
        assert_eq!(
            read_next_directory_page(
                &cursor_cycle,
                &path,
                &mut state,
                1,
                limits,
                &DavNeverCancelled,
            )
            .await,
            Err(DavDirectoryReadError::InvalidPage(
                DavDirectoryPageValidationError::CursorLoop
            ))
        );
        assert_eq!(state.cursor(), Some(&2));
        assert_eq!(state.pages_read(), 2);

        let exhausted = RecordingEnumerator::new([DavDirectoryPage {
            entries: vec![TestEntry::new("a")],
            next_cursor: Some(1),
        }]);
        let mut state = DavDirectoryPageState::new();
        let limits = page_limits(1, 1);
        read_next_directory_page(&exhausted, &path, &mut state, 1, limits, &DavNeverCancelled)
            .await
            .expect("first page");
        assert_eq!(
            read_next_directory_page(&exhausted, &path, &mut state, 1, limits, &DavNeverCancelled,)
                .await,
            Err(DavDirectoryReadError::PageLimitExceeded)
        );
        assert_eq!(exhausted.calls.lock().expect("calls").len(), 1);
    });
}

#[test]
fn traversal_budget_enforces_every_exact_boundary_without_allocating_work_storage() {
    let limits = DavTraversalLimits::new(2, 2, 1, Some(1));
    let mut budget = DavTraversalBudget::new(limits).expect("valid budget");
    budget
        .checkpoint(&DavNeverCancelled)
        .expect("not cancelled");
    budget.visit(0).expect("first visit");
    budget.visit(1).expect("exact visit and depth limit");
    assert_eq!(
        budget.visit(1).expect_err("visit limit").kind,
        DavTraversalErrorKind::VisitedResourceLimitExceeded
    );
    assert_eq!(
        budget.visit(2).expect_err("depth limit").kind,
        DavTraversalErrorKind::DepthLimitExceeded
    );

    budget.reserve_work(2).expect("exact queue limit");
    assert_eq!(
        budget.reserve_work(1).expect_err("queue limit").kind,
        DavTraversalErrorKind::QueuedWorkLimitExceeded
    );
    budget.complete_work();
    budget.reserve_work(1).expect("released queue slot");
    budget.complete_work();
    budget.complete_work();
    budget.complete_work();
    assert_eq!(budget.progress().queued_work_items, 0);

    budget.reserve_work(1).expect("one queued item");
    let overflow = budget
        .reserve_work(usize::MAX)
        .expect_err("queue count addition should not overflow");
    assert_eq!(
        overflow.kind,
        DavTraversalErrorKind::QueuedWorkLimitExceeded
    );
    assert_eq!(overflow.progress.queued_work_items, 1);
    budget.complete_work();

    budget.record_failure().expect("exact failure limit");
    assert_eq!(
        budget.record_failure().expect_err("failure limit").kind,
        DavTraversalErrorKind::FailureLimitExceeded
    );
}

#[test]
fn traversal_errors_retain_partial_execution_and_cancellation_progress() {
    for limits in [
        DavTraversalLimits::new(0, 1, 1, None),
        DavTraversalLimits::new(1, 0, 1, None),
        DavTraversalLimits::new(1, 1, 0, None),
    ] {
        assert_eq!(
            DavTraversalBudget::new(limits)
                .expect_err("zero limit")
                .kind,
            DavTraversalErrorKind::InvalidLimits
        );
    }

    let cancellation = ToggleCancellation::new();
    let mut budget =
        DavTraversalBudget::new(DavTraversalLimits::new(1, 1, 1, None)).expect("budget");
    budget.visit(0).expect("visit");
    budget.record_completed_mutation();
    cancellation.cancelled.store(true, Ordering::SeqCst);
    let cancelled = budget
        .checkpoint(&cancellation)
        .expect_err("cancelled after a mutation");
    assert_eq!(cancelled.kind, DavTraversalErrorKind::Cancelled);
    assert!(cancelled.partial_execution());
    assert_eq!(cancelled.progress.visited_resources, 1);
    assert_eq!(cancelled.progress.completed_mutations, 1);
}

#[test]
fn entry_metadata_is_page_owned_and_does_not_require_async_per_entry_lookup() {
    let entry = TestEntry::new("file.txt");
    assert_eq!(entry.name(), b"file.txt");
    assert_eq!(entry.stable_key(), b"file.txt");
    assert_eq!(entry.metadata().len(), 0);
}

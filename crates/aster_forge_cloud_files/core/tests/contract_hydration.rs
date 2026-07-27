mod support;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use aster_forge_cloud_files_core::{
    Alignment, BackendResult, ByteRange, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, ContentReadRange, ContentReadRequest, ContentReadResponse,
    HydrationCancellationOutcome, HydrationCoordinator, HydrationError, HydrationRequest,
    SessionGeneration,
};
use async_trait::async_trait;
use bytes::Bytes;

use support::{
    SyntheticBackend,
    hydration::{
        BlockingContentBackend, RecordingContentBackend, RevisionContent, revision,
        wait_until_started,
    },
};

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("session generation fixture should be valid")
}

fn range(offset: u64, length: u64) -> ByteRange {
    ByteRange::new(offset, length).expect("range fixture should be valid")
}

fn content_fixture() -> (SyntheticBackend, RevisionContent) {
    let backend = SyntheticBackend::full();
    let file = backend.moved_file();
    let revision = file
        .content()
        .expect("synthetic file should have content metadata")
        .revision()
        .clone();
    (
        backend,
        RevisionContent {
            revision,
            bytes: Bytes::from_static(b"0123456789abcdef"),
        },
    )
}

fn request(
    backend: &SyntheticBackend,
    revision: aster_forge_cloud_files_core::ContentRevision,
    offset: u64,
    length: u64,
    alignment: u64,
    session: SessionGeneration,
) -> HydrationRequest {
    HydrationRequest::range(
        backend.moved_file().key().clone(),
        revision,
        16,
        range(offset, length),
        Alignment::new(alignment).expect("alignment fixture should be valid"),
        session,
    )
}

struct FailingContentBackend {
    requests: AtomicUsize,
    error: CloudBackendError,
}

impl FailingContentBackend {
    fn new(error: CloudBackendError) -> Self {
        Self {
            requests: AtomicUsize::new(0),
            error,
        }
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::Acquire)
    }
}

#[async_trait]
impl CloudContentBackend for FailingContentBackend {
    async fn read_content(
        &self,
        _request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        self.requests.fetch_add(1, Ordering::AcqRel);
        Err(self.error.clone())
    }
}

#[derive(Clone, Copy)]
enum ResponseFault {
    WrongOffset,
    WrongTotalSize,
    ShortRange,
    LongRange,
    ShortWhole,
}

struct FaultyContentBackend {
    fault: ResponseFault,
}

#[async_trait]
impl CloudContentBackend for FaultyContentBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        let (offset, byte_len, total_size) = match (self.fault, request.read_range()) {
            (ResponseFault::WrongOffset, ContentReadRange::Range(range)) => {
                (range.offset() - 1, range.length(), request.expected_size())
            }
            (ResponseFault::WrongTotalSize, ContentReadRange::Range(range)) => {
                (range.offset(), range.length(), request.expected_size() + 1)
            }
            (ResponseFault::ShortRange, ContentReadRange::Range(range)) => {
                (range.offset(), range.length() - 1, request.expected_size())
            }
            (ResponseFault::LongRange, ContentReadRange::Range(range)) => {
                (range.offset(), range.length() + 1, request.expected_size())
            }
            (ResponseFault::ShortWhole, ContentReadRange::Whole) => {
                (0, request.expected_size() - 1, request.expected_size())
            }
            _ => unreachable!("fault fixture must match the request shape"),
        };
        ContentReadResponse::new(
            request.revision().clone(),
            offset,
            Bytes::from(vec![
                b'x';
                usize::try_from(byte_len)
                    .expect("fixture length fits usize")
            ]),
            total_size,
        )
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

#[test]
fn hydration_request_constructors_and_accessors_preserve_all_boundaries() {
    let (fixture, content) = content_fixture();
    let whole = HydrationRequest::whole(
        fixture.moved_file().key().clone(),
        content.revision.clone(),
        16,
        generation(3),
    );
    assert_eq!(whole.read().read_range(), ContentReadRange::Whole);
    assert_eq!(whole.alignment(), Alignment::ONE);
    assert_eq!(whole.session_generation(), generation(3));

    let range_request = HydrationRequest::new(
        ContentReadRequest::range(
            fixture.moved_file().key().clone(),
            content.revision,
            16,
            range(2, 4),
        ),
        Alignment::new(8).expect("alignment fixture should be valid"),
        generation(4),
    );
    assert_eq!(
        range_request.read().read_range(),
        ContentReadRange::Range(range(2, 4))
    );
    assert_eq!(range_request.alignment().get(), 8);
    assert_eq!(range_request.session_generation(), generation(4));
}

#[tokio::test]
async fn identical_concurrent_waiters_share_one_backend_read() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = request(&fixture, content.revision, 2, 6, 1, generation(1));

    let first = coordinator
        .request(request.clone())
        .expect("first waiter registration should succeed");
    let second = coordinator
        .request(request)
        .expect("second waiter registration should succeed");
    let (first, second) = tokio::join!(first.wait(), second.wait());

    assert_eq!(
        first
            .expect("first hydration should succeed")
            .bytes()
            .as_ref(),
        b"234567"
    );
    assert_eq!(
        second
            .expect("second hydration should succeed")
            .bytes()
            .as_ref(),
        b"234567"
    );
    assert_eq!(backend.requests().len(), 1);
}

#[tokio::test]
async fn whole_file_waiters_share_one_whole_backend_read() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = HydrationRequest::whole(
        fixture.moved_file().key().clone(),
        content.revision,
        16,
        generation(1),
    );
    let first = coordinator
        .request(request.clone())
        .expect("first whole waiter registration should succeed");
    let second = coordinator
        .request(request)
        .expect("second whole waiter registration should succeed");

    let (first, second) = tokio::join!(first.wait(), second.wait());
    assert_eq!(
        first.expect("first whole hydration should succeed").bytes(),
        &content.bytes
    );
    assert_eq!(
        second
            .expect("second whole hydration should succeed")
            .bytes(),
        &content.bytes
    );
    let requests = backend.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].read_range(), ContentReadRange::Whole);
}

#[tokio::test]
async fn overlapping_ranges_reuse_existing_work_and_fetch_only_the_gap() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let first = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            0,
            8,
            1,
            generation(1),
        ))
        .expect("first waiter registration should succeed");
    let second = coordinator
        .request(request(&fixture, content.revision, 4, 8, 1, generation(1)))
        .expect("overlapping waiter registration should succeed");

    let (first, second) = tokio::join!(first.wait(), second.wait());
    assert_eq!(
        first
            .expect("first hydration should succeed")
            .bytes()
            .as_ref(),
        b"01234567"
    );
    assert_eq!(
        second
            .expect("second hydration should succeed")
            .bytes()
            .as_ref(),
        b"456789ab"
    );

    let mut ranges = backend
        .requests()
        .into_iter()
        .map(|request| match request.read_range() {
            ContentReadRange::Range(range) => (range.offset(), range.length()),
            ContentReadRange::Whole => panic!("overlap test should use range reads"),
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    assert_eq!(ranges, vec![(0, 8), (8, 4)]);
}

#[tokio::test]
async fn physical_alignment_expands_backend_read_but_waiter_gets_exact_bytes() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(request(&fixture, content.revision, 3, 3, 4, generation(1)))
        .expect("aligned waiter registration should succeed");

    let response = waiter.wait().await.expect("hydration should succeed");
    assert_eq!(response.offset(), 3);
    assert_eq!(response.bytes().as_ref(), b"345");
    let requests = backend.requests();
    assert_eq!(requests.len(), 1);
    assert!(matches!(
        requests[0].read_range(),
        ContentReadRange::Range(read) if read.offset() == 0 && read.length() == 8
    ));
}

#[tokio::test]
async fn different_revisions_and_session_generations_never_share_work() {
    let (fixture, first_content) = content_fixture();
    let second_content = RevisionContent {
        revision: revision("content-v-next"),
        bytes: Bytes::from_static(b"ABCDEFGHIJKLMNOP"),
    };
    let backend = Arc::new(RecordingContentBackend::new(vec![
        first_content.clone(),
        second_content.clone(),
    ]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let revision_one = coordinator
        .request(request(
            &fixture,
            first_content.revision,
            0,
            4,
            1,
            generation(1),
        ))
        .expect("first revision registration should succeed");
    let revision_two = coordinator
        .request(request(
            &fixture,
            second_content.revision.clone(),
            0,
            4,
            1,
            generation(1),
        ))
        .expect("second revision registration should succeed");
    let new_session = coordinator
        .request(request(
            &fixture,
            second_content.revision,
            0,
            4,
            1,
            generation(2),
        ))
        .expect("new session registration should succeed");

    let (revision_one, revision_two, new_session) =
        tokio::join!(revision_one.wait(), revision_two.wait(), new_session.wait());
    assert_eq!(
        revision_one
            .expect("first revision should succeed")
            .bytes()
            .as_ref(),
        b"0123"
    );
    assert_eq!(
        revision_two
            .expect("second revision should succeed")
            .bytes()
            .as_ref(),
        b"ABCD"
    );
    assert_eq!(
        new_session
            .expect("new session should succeed")
            .bytes()
            .as_ref(),
        b"ABCD"
    );
    assert_eq!(backend.requests().len(), 3);
}

#[tokio::test]
async fn backend_revision_drift_fails_the_shared_assembly() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(
        RecordingContentBackend::new(vec![content.clone()])
            .with_response_revision(revision("wrong-revision")),
    );
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(request(&fixture, content.revision, 0, 4, 1, generation(1)))
        .expect("waiter registration should succeed");

    let error = waiter
        .wait()
        .await
        .expect_err("revision drift must fail hydration");
    assert!(matches!(error, HydrationError::Contract(_)));
}

#[tokio::test]
async fn cancelling_one_waiter_keeps_shared_backend_work_for_other_waiters() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(BlockingContentBackend::new(content.clone()));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = request(&fixture, content.revision, 0, 8, 1, generation(1));
    let first = coordinator
        .request(request.clone())
        .expect("first waiter registration should succeed");
    let first_cancel = first.cancellation_handle();
    let second = coordinator
        .request(request)
        .expect("second waiter registration should succeed");
    let first_task = tokio::spawn(first.wait());
    let second_task = tokio::spawn(second.wait());
    wait_until_started(&backend, 1).await;

    assert_eq!(
        first_cancel.cancel(),
        HydrationCancellationOutcome::Cancelled
    );
    assert_eq!(
        first_cancel.cancel(),
        HydrationCancellationOutcome::AlreadyCancelled
    );
    backend.release();

    assert_eq!(
        first_task
            .await
            .expect("first waiter task should join")
            .expect_err("first waiter should be cancelled"),
        HydrationError::Cancelled
    );
    assert_eq!(
        second_task
            .await
            .expect("second waiter task should join")
            .expect("second waiter should complete")
            .bytes()
            .as_ref(),
        b"01234567"
    );
    assert_eq!(backend.started(), 1);
    assert_eq!(backend.completed(), 1);
    assert_eq!(backend.cancelled(), 0);
}

#[tokio::test]
async fn subset_cancellation_releases_only_the_unshared_range_gap() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(BlockingContentBackend::new(content.clone()));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let first = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            0,
            8,
            1,
            generation(1),
        ))
        .expect("first waiter registration should succeed");
    let second = coordinator
        .request(request(&fixture, content.revision, 4, 8, 1, generation(1)))
        .expect("overlapping waiter registration should succeed");
    let second_cancel = second.cancellation_handle();
    let first_task = tokio::spawn(first.wait());
    let second_task = tokio::spawn(second.wait());
    wait_until_started(&backend, 2).await;

    assert_eq!(
        second_cancel.cancel(),
        HydrationCancellationOutcome::Cancelled
    );
    for _ in 0..20 {
        if backend.cancelled() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(backend.cancelled(), 1);
    backend.release();

    assert_eq!(
        first_task
            .await
            .expect("first waiter task should join")
            .expect("shared range should still complete")
            .bytes()
            .as_ref(),
        b"01234567"
    );
    assert!(matches!(
        second_task.await.expect("second waiter task should join"),
        Err(HydrationError::Cancelled)
    ));
    assert_eq!(backend.started(), 2);
    assert_eq!(backend.completed(), 1);

    let mut ranges = backend
        .requests()
        .into_iter()
        .map(|request| match request.read_range() {
            ContentReadRange::Range(range) => (range.offset(), range.length()),
            ContentReadRange::Whole => panic!("subset cancellation should use range reads"),
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    assert_eq!(ranges, vec![(0, 8), (8, 4)]);
}

#[tokio::test]
async fn cancelling_every_waiter_drops_the_unowned_backend_future() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(BlockingContentBackend::new(content.clone()));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = request(&fixture, content.revision, 0, 8, 1, generation(1));
    let first = coordinator
        .request(request.clone())
        .expect("first waiter registration should succeed");
    let first_cancel = first.cancellation_handle();
    let second = coordinator
        .request(request)
        .expect("second waiter registration should succeed");
    let second_cancel = second.cancellation_handle();
    let first_task = tokio::spawn(first.wait());
    let second_task = tokio::spawn(second.wait());
    wait_until_started(&backend, 1).await;

    assert_eq!(
        first_cancel.cancel(),
        HydrationCancellationOutcome::Cancelled
    );
    assert_eq!(
        second_cancel.cancel(),
        HydrationCancellationOutcome::Cancelled
    );
    assert!(matches!(
        first_task.await.expect("first waiter task should join"),
        Err(HydrationError::Cancelled)
    ));
    assert!(matches!(
        second_task.await.expect("second waiter task should join"),
        Err(HydrationError::Cancelled)
    ));
    for _ in 0..20 {
        if backend.cancelled() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(backend.started(), 1);
    assert_eq!(backend.completed(), 0);
    assert_eq!(backend.cancelled(), 1);
}

#[tokio::test]
async fn cancellation_and_completion_produce_one_terminal_outcome() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(BlockingContentBackend::new(content.clone()));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(request(&fixture, content.revision, 0, 4, 1, generation(1)))
        .expect("waiter registration should succeed");
    let cancellation = waiter.cancellation_handle();
    let task = tokio::spawn(waiter.wait());
    wait_until_started(&backend, 1).await;
    backend.release();
    let response = task
        .await
        .expect("waiter task should join")
        .expect("completion should win");
    assert_eq!(response.bytes().as_ref(), b"0123");
    assert_eq!(
        cancellation.cancel(),
        HydrationCancellationOutcome::AlreadyCompleted
    );
}

#[tokio::test]
async fn range_at_eof_still_checks_the_requested_revision() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(request(&fixture, content.revision, 16, 4, 1, generation(1)))
        .expect("EOF waiter registration should succeed");

    let response = waiter.wait().await.expect("EOF hydration should succeed");
    assert_eq!(response.offset(), 16);
    assert!(response.bytes().is_empty());
    let requests = backend.requests();
    assert_eq!(requests.len(), 1);
    assert!(matches!(
        requests[0].read_range(),
        ContentReadRange::Range(range) if range.offset() == 16 && range.length() == 4
    ));
}

#[tokio::test]
async fn range_beyond_eof_is_rejected_before_backend_work() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let error =
        match coordinator.request(request(&fixture, content.revision, 17, 1, 1, generation(1))) {
            Ok(_) => panic!("range beyond EOF should be rejected"),
            Err(error) => error,
        };

    assert!(matches!(
        error,
        HydrationError::Backend(backend_error)
            if backend_error.kind() == CloudBackendErrorKind::InvalidRequest
    ));
    assert!(backend.requests().is_empty());
}

#[tokio::test]
async fn zero_byte_whole_file_and_duplicate_eof_ranges_share_and_return_empty_bytes() {
    let fixture = SyntheticBackend::full();
    let empty_revision = revision("empty-content");
    let backend = Arc::new(RecordingContentBackend::new(vec![RevisionContent {
        revision: empty_revision.clone(),
        bytes: Bytes::new(),
    }]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let whole = coordinator
        .request(HydrationRequest::whole(
            fixture.moved_file().key().clone(),
            empty_revision,
            0,
            generation(1),
        ))
        .expect("empty whole waiter should register");
    let result = whole
        .wait()
        .await
        .expect("empty whole hydration should succeed");
    assert_eq!(result.offset(), 0);
    assert!(result.bytes().is_empty());
    assert!(result.is_complete_file());

    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = request(&fixture, content.revision, 16, 4, 4, generation(1));
    let first = coordinator
        .request(request.clone())
        .expect("first EOF waiter should register");
    let second = coordinator
        .request(request)
        .expect("second EOF waiter should register");
    let (first, second) = tokio::join!(first.wait(), second.wait());
    assert!(
        first
            .expect("first EOF waiter should succeed")
            .bytes()
            .is_empty()
    );
    assert!(
        second
            .expect("second EOF waiter should succeed")
            .bytes()
            .is_empty()
    );
    assert_eq!(backend.requests().len(), 1);
}

#[tokio::test]
async fn disjoint_ranges_use_independent_backend_work() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let first = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            0,
            4,
            1,
            generation(1),
        ))
        .expect("first waiter should register");
    let second = coordinator
        .request(request(&fixture, content.revision, 8, 4, 1, generation(1)))
        .expect("second waiter should register");
    let (first, second) = tokio::join!(first.wait(), second.wait());
    assert_eq!(
        first.expect("first should succeed").bytes().as_ref(),
        b"0123"
    );
    assert_eq!(
        second.expect("second should succeed").bytes().as_ref(),
        b"89ab"
    );
    assert_eq!(backend.requests().len(), 2);
}

#[tokio::test]
async fn subset_range_reuses_one_existing_backend_read() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let outer = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            2,
            10,
            1,
            generation(1),
        ))
        .expect("outer waiter should register");
    let inner = coordinator
        .request(request(&fixture, content.revision, 4, 3, 1, generation(1)))
        .expect("inner waiter should register");
    let (outer, inner) = tokio::join!(outer.wait(), inner.wait());
    assert_eq!(
        outer.expect("outer should succeed").bytes().as_ref(),
        b"23456789ab"
    );
    assert_eq!(
        inner.expect("inner should succeed").bytes().as_ref(),
        b"456"
    );
    assert_eq!(backend.requests().len(), 1);
}

#[tokio::test]
async fn whole_file_work_can_satisfy_a_range_waiter_without_a_second_read() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let whole = coordinator
        .request(HydrationRequest::whole(
            fixture.moved_file().key().clone(),
            content.revision.clone(),
            16,
            generation(1),
        ))
        .expect("whole waiter should register");
    let range_waiter = coordinator
        .request(request(&fixture, content.revision, 5, 4, 1, generation(1)))
        .expect("range waiter should register");
    let (whole, range_result) = tokio::join!(whole.wait(), range_waiter.wait());
    assert_eq!(
        whole.expect("whole should succeed").bytes().as_ref(),
        b"0123456789abcdef"
    );
    assert_eq!(
        range_result.expect("range should succeed").bytes().as_ref(),
        b"5678"
    );
    assert_eq!(backend.requests().len(), 1);
}

#[tokio::test]
async fn one_range_can_join_two_existing_segments_and_fetch_only_the_middle_gap() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let left = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            0,
            4,
            1,
            generation(1),
        ))
        .expect("left waiter should register");
    let right = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            8,
            4,
            1,
            generation(1),
        ))
        .expect("right waiter should register");
    let joined = coordinator
        .request(request(&fixture, content.revision, 0, 12, 1, generation(1)))
        .expect("joined waiter should register");

    let (left, right, joined) = tokio::join!(left.wait(), right.wait(), joined.wait());
    assert_eq!(left.expect("left should succeed").bytes().as_ref(), b"0123");
    assert_eq!(
        right.expect("right should succeed").bytes().as_ref(),
        b"89ab"
    );
    assert_eq!(
        joined.expect("joined should succeed").bytes().as_ref(),
        b"0123456789ab"
    );
    let mut ranges = backend
        .requests()
        .into_iter()
        .map(|request| match request.read_range() {
            ContentReadRange::Range(range) => (range.offset(), range.length()),
            ContentReadRange::Whole => unreachable!("fixture uses range reads"),
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    assert_eq!(ranges, vec![(0, 4), (4, 4), (8, 4)]);
}

#[tokio::test]
async fn one_range_can_fill_two_gaps_around_existing_segments() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let middle = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            4,
            4,
            1,
            generation(1),
        ))
        .expect("middle waiter should register");
    let tail = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            12,
            4,
            1,
            generation(1),
        ))
        .expect("tail waiter should register");
    let whole_range = coordinator
        .request(request(&fixture, content.revision, 0, 16, 1, generation(1)))
        .expect("covering waiter should register");
    let (_, _, whole_range) = tokio::join!(middle.wait(), tail.wait(), whole_range.wait());
    assert_eq!(
        whole_range
            .expect("covering request should succeed")
            .bytes()
            .as_ref(),
        b"0123456789abcdef"
    );
    let mut ranges = backend
        .requests()
        .into_iter()
        .map(|request| match request.read_range() {
            ContentReadRange::Range(range) => (range.offset(), range.length()),
            ContentReadRange::Whole => unreachable!("fixture uses range reads"),
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    assert_eq!(ranges, vec![(0, 4), (4, 4), (8, 4), (12, 4)]);
}

#[tokio::test]
async fn same_item_revision_and_session_with_different_expected_sizes_do_not_share() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let normal = coordinator
        .request(request(
            &fixture,
            content.revision.clone(),
            0,
            4,
            1,
            generation(1),
        ))
        .expect("normal waiter should register");
    let wrong_size = coordinator
        .request(HydrationRequest::range(
            fixture.moved_file().key().clone(),
            content.revision,
            15,
            range(0, 4),
            Alignment::ONE,
            generation(1),
        ))
        .expect("different-size waiter should register");
    let (normal, wrong_size) = tokio::join!(normal.wait(), wrong_size.wait());
    assert!(normal.is_ok());
    assert!(matches!(wrong_size, Err(HydrationError::Contract(_))));
    assert_eq!(backend.requests().len(), 2);
}

#[tokio::test]
async fn shared_backend_error_is_delivered_to_every_waiter_with_one_backend_call() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(FailingContentBackend::new(CloudBackendError::retryable(
        CloudBackendErrorKind::TemporarilyUnavailable,
    )));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = request(&fixture, content.revision, 0, 4, 1, generation(1));
    let first = coordinator
        .request(request.clone())
        .expect("first waiter should register");
    let second = coordinator
        .request(request)
        .expect("second waiter should register");
    let (first, second) = tokio::join!(first.wait(), second.wait());
    for result in [first, second] {
        assert!(matches!(
            result,
            Err(HydrationError::Backend(error))
                if error.kind() == CloudBackendErrorKind::TemporarilyUnavailable
        ));
    }
    assert_eq!(backend.requests(), 1);
}

#[tokio::test]
async fn backend_response_contract_rejects_wrong_offset_size_and_range_length() {
    let (fixture, content) = content_fixture();
    for fault in [
        ResponseFault::WrongOffset,
        ResponseFault::WrongTotalSize,
        ResponseFault::ShortRange,
        ResponseFault::LongRange,
    ] {
        let backend = Arc::new(FaultyContentBackend { fault });
        let coordinator = HydrationCoordinator::new(backend as Arc<dyn CloudContentBackend>);
        let waiter = coordinator
            .request(request(
                &fixture,
                content.revision.clone(),
                2,
                4,
                1,
                generation(1),
            ))
            .expect("waiter should register");
        assert!(matches!(
            waiter.wait().await,
            Err(HydrationError::Contract(_))
        ));
    }

    let backend = Arc::new(FaultyContentBackend {
        fault: ResponseFault::ShortWhole,
    });
    let coordinator = HydrationCoordinator::new(backend as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(HydrationRequest::whole(
            fixture.moved_file().key().clone(),
            content.revision,
            16,
            generation(1),
        ))
        .expect("whole waiter should register");
    assert!(matches!(
        waiter.wait().await,
        Err(HydrationError::Contract(_))
    ));
}

#[tokio::test]
async fn cancellation_before_poll_never_starts_backend_work() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(request(&fixture, content.revision, 0, 4, 1, generation(1)))
        .expect("waiter should register");
    let cancellation = waiter.cancellation_handle();
    assert_eq!(
        cancellation.cancel(),
        HydrationCancellationOutcome::Cancelled
    );
    assert_eq!(
        cancellation.cancel(),
        HydrationCancellationOutcome::AlreadyCancelled
    );
    assert_eq!(waiter.wait().await, Err(HydrationError::Cancelled));
    assert!(backend.requests().is_empty());
}

#[tokio::test]
async fn dropping_cancellation_handle_does_not_cancel_waiter() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(request(&fixture, content.revision, 0, 4, 1, generation(1)))
        .expect("waiter should register");
    drop(waiter.cancellation_handle());
    assert_eq!(
        waiter
            .wait()
            .await
            .expect("waiter should complete")
            .bytes()
            .as_ref(),
        b"0123"
    );
}

#[tokio::test]
async fn dropping_unpolled_waiter_releases_work_and_next_request_starts_fresh() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = request(&fixture, content.revision, 0, 4, 1, generation(1));
    let abandoned = coordinator
        .request(request.clone())
        .expect("abandoned waiter should register");
    drop(abandoned);
    assert!(backend.requests().is_empty());

    let replacement = coordinator
        .request(request)
        .expect("replacement waiter should register");
    assert_eq!(
        replacement
            .wait()
            .await
            .expect("replacement should complete")
            .bytes()
            .as_ref(),
        b"0123"
    );
    assert_eq!(backend.requests().len(), 1);
}

#[tokio::test]
async fn completed_work_is_removed_and_same_request_later_starts_fresh() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator =
        HydrationCoordinator::new(Arc::clone(&backend) as Arc<dyn CloudContentBackend>);
    let request = request(&fixture, content.revision, 0, 4, 1, generation(1));
    coordinator
        .request(request.clone())
        .expect("first waiter should register")
        .wait()
        .await
        .expect("first waiter should complete");
    coordinator
        .request(request)
        .expect("second waiter should register")
        .wait()
        .await
        .expect("second waiter should complete");
    assert_eq!(backend.requests().len(), 2);
}

#[test]
fn waiter_drop_after_coordinator_drop_is_self_contained() {
    let (fixture, content) = content_fixture();
    let backend = Arc::new(RecordingContentBackend::new(vec![content.clone()]));
    let coordinator = HydrationCoordinator::new(backend as Arc<dyn CloudContentBackend>);
    let waiter = coordinator
        .request(request(&fixture, content.revision, 0, 4, 1, generation(1)))
        .expect("waiter should register");
    drop(coordinator);
    drop(waiter);
}

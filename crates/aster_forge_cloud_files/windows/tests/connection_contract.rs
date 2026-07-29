use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use aster_forge_cloud_files_core::{
    Alignment, BackendResult, ByteRange, CloudBackendError, CloudBackendErrorKind,
    CloudContentBackend, CloudFilesCoreError, CloudItemId, CloudItemKey, CloudNamespaceId,
    CloudRootId, CloudScope, ContentReadRange, ContentReadRequest, ContentReadResponse,
    ContentRevision, HydrationCoordinator, HydrationError, SessionGeneration, SessionState,
};
use aster_forge_cloud_files_windows::{
    CFAPI_MAX_PRIORITY_HINT, CFAPI_TRANSFER_ALIGNMENT_BYTES, WindowsCallbackInfoSnapshot,
    WindowsCallbackRange, WindowsCallbackRangeLength, WindowsCallbackRequest,
    WindowsCancelFetchDataFlags, WindowsCancelFetchDataSnapshot, WindowsCloudFilesError,
    WindowsConnectionKey, WindowsConnectionSession, WindowsFetchDataFailure, WindowsFetchDataFlags,
    WindowsFetchDataPreparation, WindowsFetchDataProgress, WindowsFetchDataRequest,
    WindowsFetchDataSnapshot, WindowsFetchDataTransfer, WindowsFetchDataWaiterRegistry,
    WindowsFetchDataWatchdogConfig, WindowsFileIdentity, WindowsObservedNotification,
    WindowsPreflightSnapshot, WindowsProcessInfoSnapshot, WindowsRequestKey,
    WindowsRestartHydration, WindowsSyncRootConnectOptions, WindowsSyncRootIdentity,
    WindowsTransferKey,
};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::Notify;

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("generation fixture should be non-zero")
}

fn scope(namespace: &str, root: &str) -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new(namespace).expect("namespace fixture should be valid"),
        CloudRootId::new(root).expect("root fixture should be valid"),
    )
}

fn key(namespace: &str, root: &str, item: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope(namespace, root),
        CloudItemId::new(item).expect("item fixture should be valid"),
    )
}

fn info_snapshot() -> WindowsCallbackInfoSnapshot {
    info_snapshot_with_size(i64::MAX as u64)
}

fn info_snapshot_with_size(file_size: u64) -> WindowsCallbackInfoSnapshot {
    WindowsCallbackInfoSnapshot::new(
        generation(7),
        WindowsConnectionKey::new(i64::MIN),
        WindowsTransferKey::new(-42),
        WindowsRequestKey::new(i64::MAX),
        Some(OsString::from(r"\\?\Volume{fixture}")),
        Some(OsString::from("X:")),
        u32::MAX,
        -17,
        WindowsSyncRootIdentity::encode(&scope("namespace", "root"))
            .expect("sync-root identity fixture should encode"),
        i64::MAX,
        file_size,
        WindowsFileIdentity::encode(&key("namespace", "root", "item"))
            .expect("file identity fixture should encode"),
        Some(PathBuf::from(r"\folder\file.txt")),
        CFAPI_MAX_PRIORITY_HINT,
        Some(WindowsProcessInfoSnapshot {
            process_id: u32::MAX,
            image_path: Some(PathBuf::from(r"C:\fixture.exe")),
            package_name: Some(OsString::from("package")),
            application_id: Some(OsString::from("application")),
            command_line: Some(OsString::from("fixture.exe --test")),
            session_id: u32::MAX,
        }),
    )
    .expect("callback info fixture should be valid")
}

fn info_snapshot_with_generation(generation: SessionGeneration) -> WindowsCallbackInfoSnapshot {
    let original = info_snapshot();
    WindowsCallbackInfoSnapshot::new(
        generation,
        original.connection_key(),
        original.transfer_key(),
        original.request_key(),
        original.volume_guid_name().map(OsString::from),
        original.volume_dos_name().map(OsString::from),
        original.volume_serial_number(),
        original.sync_root_file_id(),
        original.sync_root_identity().clone(),
        original.file_id(),
        original.file_size(),
        original.file_identity().clone(),
        original.normalized_path().map(PathBuf::from),
        original.priority_hint(),
        original.process().cloned(),
    )
    .expect("generation fixture should preserve callback info contract")
}

fn fetch_request(file_size: u64, range: WindowsCallbackRange) -> WindowsFetchDataRequest {
    let session = WindowsConnectionSession::new(generation(7));
    let lease = session
        .begin_callback(generation(7))
        .expect("fetch fixture should acquire a callback lease");
    let request = WindowsCallbackRequest::fetch_data(
        WindowsFetchDataSnapshot::new(
            info_snapshot_with_size(file_size),
            WindowsFetchDataFlags::default(),
            range,
            None,
            0,
            0,
        ),
        lease,
    )
    .expect("fetch fixture snapshot and lease should match");
    let WindowsCallbackRequest::FetchData(request) = request else {
        panic!("fetch fixture should construct a fetch request");
    };
    request
}

#[test]
fn progress_reporter_keeps_only_compact_native_correlation() {
    let request = fetch_request(
        4096,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    );
    let info = request.snapshot().info();
    let reporter = request.progress_reporter();
    let correlation = reporter.correlation();

    assert_eq!(correlation.generation(), info.generation());
    assert_eq!(correlation.connection_key(), info.connection_key());
    assert_eq!(correlation.transfer_key(), info.transfer_key());
    assert_eq!(correlation.request_key(), info.request_key());
    assert_eq!(correlation.file_id(), info.file_id());
    assert!(std::mem::size_of_val(&reporter) <= 64);
}

struct RecordingContentBackend {
    requests: Mutex<Vec<ContentReadRequest>>,
    failure: Option<CloudBackendError>,
}

impl RecordingContentBackend {
    fn successful() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            failure: None,
        }
    }

    fn failing(error: CloudBackendError) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            failure: Some(error),
        }
    }

    fn requests(&self) -> Vec<ContentReadRequest> {
        match self.requests.lock() {
            Ok(requests) => requests.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[async_trait]
impl CloudContentBackend for RecordingContentBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        match self.requests.lock() {
            Ok(mut requests) => requests.push(request.clone()),
            Err(poisoned) => poisoned.into_inner().push(request.clone()),
        }
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
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
                usize::try_from(range.length()).map_err(|_| {
                    CloudBackendError::new(CloudBackendErrorKind::Internal)
                })?
            ]),
            request.expected_size(),
        )
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

struct BlockingContentBackend {
    requests: Mutex<Vec<ContentReadRequest>>,
    started: Notify,
    release: Notify,
}

impl BlockingContentBackend {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            started: Notify::new(),
            release: Notify::new(),
        }
    }

    fn requests(&self) -> Vec<ContentReadRequest> {
        match self.requests.lock() {
            Ok(requests) => requests.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn release(&self) {
        self.release.notify_waiters();
    }
}

#[async_trait]
impl CloudContentBackend for BlockingContentBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        match self.requests.lock() {
            Ok(mut requests) => requests.push(request.clone()),
            Err(poisoned) => poisoned.into_inner().push(request.clone()),
        }
        self.started.notify_waiters();
        self.release.notified().await;
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
                usize::try_from(range.length()).map_err(|_| {
                    CloudBackendError::new(CloudBackendErrorKind::Internal)
                })?
            ]),
            request.expected_size(),
        )
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }
}

#[test]
fn native_scalar_keys_preserve_the_complete_signed_domain() {
    for value in [i64::MIN, -1, 0, 1, i64::MAX] {
        assert_eq!(WindowsConnectionKey::new(value).get(), value);
        assert_eq!(WindowsTransferKey::new(value).get(), value);
        assert_eq!(WindowsRequestKey::new(value).get(), value);
    }
}

#[test]
fn connect_options_default_to_no_optional_native_behavior() {
    assert_eq!(
        WindowsSyncRootConnectOptions::default(),
        WindowsSyncRootConnectOptions {
            require_process_info: false,
            require_full_file_path: false,
            block_self_implicit_hydration: false,
        }
    );
}

#[test]
fn fetch_failure_classifications_are_explicit_and_non_overlapping() {
    let failures = [
        WindowsFetchDataFailure::InvalidRequest,
        WindowsFetchDataFailure::InsufficientResources,
        WindowsFetchDataFailure::ProviderTerminated,
        WindowsFetchDataFailure::AuthenticationFailed,
        WindowsFetchDataFailure::AccessDenied,
        WindowsFetchDataFailure::NotInSync,
        WindowsFetchDataFailure::NetworkUnavailable,
        WindowsFetchDataFailure::NotSupported,
        WindowsFetchDataFailure::Cancelled,
        WindowsFetchDataFailure::Unsuccessful,
    ];
    for (index, failure) in failures.iter().enumerate() {
        assert!(!failures[..index].contains(failure));
        assert!(!format!("{failure:?}").is_empty());
    }
}

#[test]
fn hydration_request_binds_stable_identity_revision_size_generation_and_alignment() {
    let revision =
        ContentRevision::from_slice(b"revision-7").expect("revision fixture should be non-empty");
    let request = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("aligned range should be valid"),
    );
    let hydration = request
        .hydration_request(
            revision.clone(),
            Alignment::new(65_536).expect("backend alignment should be non-zero"),
        )
        .expect("valid callback should build hydration request");

    assert_eq!(hydration.session_generation(), generation(7));
    assert_eq!(hydration.alignment().get(), 65_536);
    assert_eq!(hydration.read().key(), &key("namespace", "root", "item"));
    assert_eq!(hydration.read().revision(), &revision);
    assert_eq!(hydration.read().expected_size(), 8192);
    assert_eq!(
        hydration.read().read_range(),
        ContentReadRange::Range(ByteRange::new(0, 4096).expect("range should be valid"))
    );
}

#[test]
fn hydration_request_resolves_cf_eof_and_rejects_every_native_range_boundary() {
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let eof = fetch_request(
        5000,
        WindowsCallbackRange::to_end(4096).expect("EOF range should be valid"),
    )
    .hydration_request(revision.clone(), Alignment::ONE)
    .expect("aligned EOF tail should build hydration request");
    assert_eq!(eof.alignment().get(), CFAPI_TRANSFER_ALIGNMENT_BYTES);
    assert_eq!(
        eof.read().read_range(),
        ContentReadRange::Range(ByteRange::new(4096, 904).expect("EOF range should be valid"))
    );

    let cases = [
        (
            fetch_request(
                8192,
                WindowsCallbackRange::exact(1, 4096).expect("scalar range should construct"),
            ),
            "CFAPI transfer offset must be aligned to 4096 bytes",
        ),
        (
            fetch_request(
                8192,
                WindowsCallbackRange::exact(0, 1).expect("scalar range should construct"),
            ),
            "CFAPI transfer length must be aligned to 4096 bytes unless it reaches end-of-file",
        ),
        (
            fetch_request(
                4096,
                WindowsCallbackRange::to_end(4096).expect("EOF range should construct"),
            ),
            "CFAPI fetch range resolves to an empty successful transfer",
        ),
        (
            fetch_request(
                4096,
                WindowsCallbackRange::exact(0, 8192).expect("scalar range should construct"),
            ),
            "CFAPI fetch range exceeds callback file size",
        ),
    ];
    for (request, reason) in cases {
        assert!(matches!(
            request.hydration_request(revision.clone(), Alignment::ONE),
            Err(WindowsCloudFilesError::InvalidFetchTransfer {
                reason: actual
            }) if actual == reason
        ));
    }

    let overflow = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    )
    .hydration_request(
        revision,
        Alignment::new(u64::MAX).expect("maximum alignment should be non-zero"),
    );
    assert!(matches!(
        overflow,
        Err(WindowsCloudFilesError::Core(
            CloudFilesCoreError::AlignmentIntersectionOverflow {
                left: CFAPI_TRANSFER_ALIGNMENT_BYTES,
                right: u64::MAX
            }
        ))
    ));
}

#[test]
fn transfer_requires_exact_revision_offset_length_and_total_size_without_copying_semantics() {
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let request = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    );
    let bytes = Bytes::from(vec![0x5a; 4096]);
    let transfer = WindowsFetchDataTransfer::from_response(
        request.snapshot(),
        &revision,
        ContentReadResponse::new(revision.clone(), 0, bytes.clone(), 8192)
            .expect("response fixture should be valid"),
    )
    .expect("exact response should become a transfer");
    assert_eq!(transfer.offset(), 0);
    assert_eq!(transfer.length(), 4096);
    assert_eq!(transfer.bytes(), bytes.as_ref());

    let wrong_revision =
        ContentRevision::from_slice(b"other").expect("revision fixture should be non-empty");
    assert!(matches!(
        WindowsFetchDataTransfer::from_response(
            request.snapshot(),
            &revision,
            ContentReadResponse::new(wrong_revision, 0, Bytes::from(vec![0; 4096]), 8192)
                .expect("response shape should construct")
        ),
        Err(WindowsCloudFilesError::Core(
            CloudFilesCoreError::InvalidContentResponse {
                reason: "response revision does not match the requested revision"
            }
        ))
    ));
    assert!(matches!(
        WindowsFetchDataTransfer::from_response(
            request.snapshot(),
            &revision,
            ContentReadResponse::new(revision.clone(), 4096, Bytes::from(vec![0; 4096]), 8192)
                .expect("response shape should construct")
        ),
        Err(WindowsCloudFilesError::Core(
            CloudFilesCoreError::InvalidContentResponse {
                reason: "range response starts at a different offset"
            }
        ))
    ));
    assert!(matches!(
        WindowsFetchDataTransfer::from_response(
            request.snapshot(),
            &revision,
            ContentReadResponse::new(revision.clone(), 0, Bytes::from(vec![0; 4095]), 8192)
                .expect("response shape should construct")
        ),
        Err(WindowsCloudFilesError::Core(
            CloudFilesCoreError::InvalidContentResponse {
                reason: "range response length does not match the requested extent"
            }
        ))
    ));
    assert!(matches!(
        WindowsFetchDataTransfer::from_response(
            request.snapshot(),
            &revision,
            ContentReadResponse::new(revision.clone(), 0, Bytes::from(vec![0; 4096]), 16_384)
                .expect("response shape should construct")
        ),
        Err(WindowsCloudFilesError::Core(
            CloudFilesCoreError::InvalidContentResponse {
                reason: "response size does not match metadata for the requested revision"
            }
        ))
    ));
}

#[test]
fn hydration_errors_map_to_specific_cfapi_failure_classes() {
    assert_eq!(
        WindowsFetchDataFailure::from_hydration_error(&HydrationError::InFlightLimitExceeded {
            scope: "global",
        }),
        WindowsFetchDataFailure::InsufficientResources
    );
    assert_eq!(
        WindowsFetchDataFailure::from_hydration_error(&HydrationError::Cancelled),
        WindowsFetchDataFailure::Cancelled
    );
    assert_eq!(
        WindowsFetchDataFailure::from_hydration_error(&HydrationError::Contract(
            CloudFilesCoreError::InvalidContentResponse { reason: "fixture" }
        )),
        WindowsFetchDataFailure::InvalidRequest
    );

    let cases = [
        (
            CloudBackendErrorKind::NotFound,
            WindowsFetchDataFailure::NotInSync,
        ),
        (
            CloudBackendErrorKind::AuthenticationRequired,
            WindowsFetchDataFailure::AuthenticationFailed,
        ),
        (
            CloudBackendErrorKind::PermissionDenied,
            WindowsFetchDataFailure::AccessDenied,
        ),
        (
            CloudBackendErrorKind::Conflict,
            WindowsFetchDataFailure::NotInSync,
        ),
        (
            CloudBackendErrorKind::PreconditionFailed,
            WindowsFetchDataFailure::NotInSync,
        ),
        (
            CloudBackendErrorKind::InvalidRequest,
            WindowsFetchDataFailure::InvalidRequest,
        ),
        (
            CloudBackendErrorKind::RateLimited,
            WindowsFetchDataFailure::NetworkUnavailable,
        ),
        (
            CloudBackendErrorKind::TemporarilyUnavailable,
            WindowsFetchDataFailure::NetworkUnavailable,
        ),
        (
            CloudBackendErrorKind::Unsupported,
            WindowsFetchDataFailure::NotSupported,
        ),
        (
            CloudBackendErrorKind::InvalidResponse,
            WindowsFetchDataFailure::Unsuccessful,
        ),
        (
            CloudBackendErrorKind::Internal,
            WindowsFetchDataFailure::Unsuccessful,
        ),
    ];
    for (kind, expected) in cases {
        let error = HydrationError::Backend(CloudBackendError::new(kind));
        assert_eq!(
            WindowsFetchDataFailure::from_hydration_error(&error),
            expected
        );
    }
}

#[tokio::test]
async fn prepare_transfer_drives_coordinator_and_keeps_terminal_ownership_with_request() {
    let backend = Arc::new(RecordingContentBackend::successful());
    let coordinator = HydrationCoordinator::new(backend.clone() as Arc<dyn CloudContentBackend>);
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let request = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    );

    let transfer = request
        .prepare_transfer(&coordinator, revision.clone(), Alignment::ONE)
        .await
        .expect("coordinator result should prepare a native transfer");
    assert_eq!(transfer.offset(), 0);
    assert_eq!(transfer.length(), 4096);
    assert_eq!(transfer.bytes(), vec![0x5a; 4096]);
    assert!(!request.terminal_attempted());

    let requests = backend.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].key(), &key("namespace", "root", "item"));
    assert_eq!(requests[0].revision(), &revision);
    assert_eq!(requests[0].expected_size(), 8192);
    assert_eq!(
        requests[0].read_range(),
        ContentReadRange::Range(ByteRange::new(0, 4096).expect("range should be valid"))
    );
}

#[tokio::test]
async fn prepare_transfer_preserves_backend_failure_and_does_not_consume_terminal_gate() {
    let backend_error = CloudBackendError::retryable(CloudBackendErrorKind::TemporarilyUnavailable);
    let backend = Arc::new(RecordingContentBackend::failing(backend_error.clone()));
    let coordinator = HydrationCoordinator::new(backend as Arc<dyn CloudContentBackend>);
    let request = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    );
    let result = request
        .prepare_transfer(
            &coordinator,
            ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty"),
            Alignment::ONE,
        )
        .await;

    assert!(matches!(
        result,
        Err(WindowsCloudFilesError::Hydration(HydrationError::Backend(error)))
            if error == backend_error
    ));
    assert!(!request.terminal_attempted());
}

#[tokio::test]
async fn registered_transfer_full_cancel_wakes_waiter_without_native_completion() {
    let backend = Arc::new(BlockingContentBackend::new());
    let coordinator = HydrationCoordinator::new(backend.clone() as Arc<dyn CloudContentBackend>);
    let registry = WindowsFetchDataWaiterRegistry::new();
    let mut request = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    );
    let cancel = WindowsCancelFetchDataSnapshot::new(
        info_snapshot(),
        WindowsCancelFetchDataFlags::IO_ABORTED,
        WindowsCallbackRange::exact(0, 4096).expect("cancel range should be valid"),
    );
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let task = tokio::spawn({
        let coordinator = coordinator.clone();
        let registry = registry.clone();
        async move {
            let result = request
                .prepare_registered_transfer(&coordinator, revision, Alignment::ONE, &registry)
                .await;
            (request, result)
        }
    });
    for _ in 0..100 {
        if registry.metrics().active_waiters() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(registry.metrics().active_waiters(), 1);

    let outcome = registry
        .cancel(&cancel)
        .expect("matching cancellation should validate");
    assert_eq!(outcome.fully_matched(), 1);
    assert_eq!(outcome.cancelled(), 1);
    assert_eq!(outcome.partially_matched(), 0);
    let repeated = registry
        .cancel(&cancel)
        .expect("repeated cancellation should validate");
    assert_eq!(repeated.already_cancelled(), 1);
    let (mut request, result) = task.await.expect("hydration task should join");
    assert!(matches!(result, Ok(WindowsFetchDataPreparation::Cancelled)));
    assert!(matches!(
        request
            .prepare_registered_transfer(
                &coordinator,
                ContentRevision::from_slice(b"replacement")
                    .expect("revision fixture should be non-empty"),
                Alignment::ONE,
                &registry,
            )
            .await,
        Ok(WindowsFetchDataPreparation::Cancelled)
    ));
    assert_eq!(registry.metrics().active_waiters(), 0);
    assert_eq!(registry.metrics().registered_waiters(), 1);
    assert_eq!(registry.metrics().cancelled_waiters(), 1);
    assert_eq!(registry.metrics().already_cancelled_waiters(), 1);
    assert!(backend.requests().len() <= 1);
}

#[tokio::test]
async fn registered_transfer_partial_cancel_retains_bytes_outside_cancel_range() {
    let backend = Arc::new(BlockingContentBackend::new());
    let coordinator = HydrationCoordinator::new(backend.clone() as Arc<dyn CloudContentBackend>);
    let registry = WindowsFetchDataWaiterRegistry::new();
    let mut request = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 8192).expect("range should be valid"),
    );
    let cancel = WindowsCancelFetchDataSnapshot::new(
        info_snapshot(),
        WindowsCancelFetchDataFlags::IO_TIMEOUT,
        WindowsCallbackRange::exact(0, 4096).expect("cancel range should be valid"),
    );
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let task = tokio::spawn({
        let registry = registry.clone();
        async move {
            request
                .prepare_registered_transfer(&coordinator, revision, Alignment::ONE, &registry)
                .await
        }
    });
    for _ in 0..100 {
        if registry.metrics().active_waiters() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(registry.metrics().active_waiters(), 1);

    let outcome = registry
        .cancel(&cancel)
        .expect("partial cancellation should validate");
    assert_eq!(outcome.fully_matched(), 0);
    assert_eq!(outcome.partially_matched(), 1);
    assert_eq!(outcome.cancelled(), 0);
    assert_eq!(registry.metrics().active_waiters(), 1);

    backend.release();
    let preparation = task
        .await
        .expect("hydration task should join")
        .expect("retained waiter should complete");
    let WindowsFetchDataPreparation::Transfer(preparation) = preparation else {
        panic!("partial cancellation must retain a transfer");
    };
    assert_eq!(preparation.length(), 8192);
    assert_eq!(registry.metrics().active_waiters(), 0);
    assert_eq!(registry.metrics().partially_matched_waiters(), 1);
}

#[tokio::test]
async fn cancellation_correlation_includes_session_generation() {
    let backend = Arc::new(BlockingContentBackend::new());
    let coordinator = HydrationCoordinator::new(backend.clone() as Arc<dyn CloudContentBackend>);
    let registry = WindowsFetchDataWaiterRegistry::new();
    let mut request = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    );
    let cancel = WindowsCancelFetchDataSnapshot::new(
        info_snapshot_with_generation(generation(8)),
        WindowsCancelFetchDataFlags::IO_ABORTED,
        WindowsCallbackRange::exact(0, 4096).expect("cancel range should be valid"),
    );
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let task = tokio::spawn({
        let registry = registry.clone();
        async move {
            request
                .prepare_registered_transfer(&coordinator, revision, Alignment::ONE, &registry)
                .await
        }
    });
    for _ in 0..100 {
        if registry.metrics().active_waiters() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        registry.cancel(&cancel).expect("range should validate"),
        Default::default()
    );
    assert_eq!(registry.metrics().unmatched_cancellations(), 1);
    backend.release();
    assert!(matches!(
        task.await.expect("hydration task should join"),
        Ok(WindowsFetchDataPreparation::Transfer(_))
    ));
}

#[tokio::test]
async fn subset_cancel_only_releases_fully_covered_waiters_with_same_correlation() {
    let backend = Arc::new(BlockingContentBackend::new());
    let coordinator = HydrationCoordinator::new(backend.clone() as Arc<dyn CloudContentBackend>);
    let registry = WindowsFetchDataWaiterRegistry::new();
    let mut first = fetch_request(
        8192,
        WindowsCallbackRange::exact(0, 4096).expect("first range should be valid"),
    );
    let mut second = fetch_request(
        8192,
        WindowsCallbackRange::exact(4096, 4096).expect("second range should be valid"),
    );
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let first_task = tokio::spawn({
        let coordinator = coordinator.clone();
        let registry = registry.clone();
        let revision = revision.clone();
        async move {
            first
                .prepare_registered_transfer(&coordinator, revision, Alignment::ONE, &registry)
                .await
        }
    });
    let second_task = tokio::spawn({
        let registry = registry.clone();
        async move {
            second
                .prepare_registered_transfer(&coordinator, revision, Alignment::ONE, &registry)
                .await
        }
    });
    for _ in 0..100 {
        if registry.metrics().active_waiters() == 2 && backend.requests().len() == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(registry.metrics().active_waiters(), 2);
    assert_eq!(backend.requests().len(), 2);

    let cancel = WindowsCancelFetchDataSnapshot::new(
        info_snapshot(),
        WindowsCancelFetchDataFlags::IO_ABORTED,
        WindowsCallbackRange::exact(0, 4096).expect("cancel range should be valid"),
    );
    let outcome = registry
        .cancel(&cancel)
        .expect("subset cancellation should validate");
    assert_eq!(outcome.fully_matched(), 1);
    assert_eq!(outcome.partially_matched(), 0);
    assert_eq!(outcome.cancelled(), 1);

    backend.release();
    assert!(matches!(
        first_task.await.expect("first hydration task should join"),
        Ok(WindowsFetchDataPreparation::Cancelled)
    ));
    assert!(matches!(
        second_task
            .await
            .expect("second hydration task should join"),
        Ok(WindowsFetchDataPreparation::Transfer(_))
    ));
    assert_eq!(registry.metrics().active_waiters(), 0);
    assert_eq!(registry.metrics().registered_waiters(), 2);
    assert_eq!(registry.metrics().fully_matched_waiters(), 1);
}

#[tokio::test]
async fn watchdog_timeout_is_distinct_from_platform_cancellation_and_does_not_reregister() {
    let backend = Arc::new(BlockingContentBackend::new());
    let coordinator = HydrationCoordinator::new(backend.clone() as Arc<dyn CloudContentBackend>);
    let config = WindowsFetchDataWatchdogConfig::new(Duration::from_secs(1))
        .expect("watchdog fixture should be valid");
    let registry = WindowsFetchDataWaiterRegistry::with_watchdog_config(config);
    let mut request = fetch_request(
        4096,
        WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
    );
    let revision =
        ContentRevision::from_slice(b"revision").expect("revision fixture should be non-empty");
    let started = Instant::now();
    let task = tokio::spawn({
        let coordinator = coordinator.clone();
        let registry = registry.clone();
        let revision = revision.clone();
        async move {
            request
                .prepare_registered_transfer(
                    &coordinator,
                    revision.clone(),
                    Alignment::ONE,
                    &registry,
                )
                .await
                .map(|outcome| (request, revision, outcome))
        }
    });
    for _ in 0..100 {
        if registry.metrics().active_waiters() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(registry.metrics().active_waiters(), 1);
    assert_eq!(
        registry
            .report_progress(
                &info_snapshot_with_size(4096),
                WindowsFetchDataProgress::new(4096, 1024)
                    .expect("progress fixture should be valid"),
                started + Duration::from_millis(500),
            )
            .expect("progress should refresh the matching waiter"),
        1
    );
    assert_eq!(
        registry
            .poll_watchdog(started + Duration::from_secs(1))
            .cancelled(),
        0
    );
    assert_eq!(
        registry
            .poll_watchdog(started + Duration::from_secs(2))
            .cancelled(),
        1
    );
    let (mut request, revision, outcome) = task
        .await
        .expect("watchdog task should join")
        .expect("watchdog preparation should return a typed outcome");
    assert!(matches!(outcome, WindowsFetchDataPreparation::TimedOut));
    assert!(matches!(
        request
            .prepare_registered_transfer(&coordinator, revision, Alignment::ONE, &registry)
            .await,
        Ok(WindowsFetchDataPreparation::TimedOut)
    ));
    assert_eq!(registry.metrics().registered_waiters(), 1);
    assert_eq!(registry.metrics().watchdog_timeouts(), 1);
    assert_eq!(registry.metrics().progress_callbacks(), 1);
    assert_eq!(registry.metrics().progress_updates(), 1);
}

#[test]
fn cancellation_registry_rejects_empty_eof_and_beyond_file_ranges() {
    let registry = WindowsFetchDataWaiterRegistry::new();
    let cases = [
        (
            WindowsCancelFetchDataSnapshot::new(
                info_snapshot_with_size(4096),
                WindowsCancelFetchDataFlags::default(),
                WindowsCallbackRange::to_end(4096).expect("EOF range should construct"),
            ),
            "cancellation range must not be empty",
        ),
        (
            WindowsCancelFetchDataSnapshot::new(
                info_snapshot_with_size(4096),
                WindowsCancelFetchDataFlags::default(),
                WindowsCallbackRange::to_end(8192).expect("EOF range should construct"),
            ),
            "cancellation range offset exceeds callback file size",
        ),
        (
            WindowsCancelFetchDataSnapshot::new(
                info_snapshot_with_size(4096),
                WindowsCancelFetchDataFlags::default(),
                WindowsCallbackRange::exact(0, 8192).expect("range should construct"),
            ),
            "cancellation range exceeds callback file size",
        ),
    ];
    for (snapshot, reason) in cases {
        assert!(matches!(
            registry.cancel(&snapshot),
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot { reason: actual })
                if actual == reason
        ));
    }
    assert_eq!(registry.metrics().cancellation_callbacks(), 0);
}

#[test]
fn cancel_request_snapshot_can_be_inspected_without_reapplying_native_cancellation() {
    let session = WindowsConnectionSession::new(generation(7));
    let lease = session
        .begin_callback(generation(7))
        .expect("cancellation fixture should acquire a lease");
    let request = WindowsCallbackRequest::cancel_fetch_data(
        WindowsCancelFetchDataSnapshot::new(
            info_snapshot(),
            WindowsCancelFetchDataFlags::IO_TIMEOUT,
            WindowsCallbackRange::exact(0, 4096).expect("range should be valid"),
        ),
        lease,
    )
    .expect("cancellation fixture should construct");
    let WindowsCallbackRequest::CancelFetchData(request) = request else {
        panic!("fixture should create a cancellation request");
    };
    let registry = WindowsFetchDataWaiterRegistry::new();
    let outcome = registry
        .cancel(request.snapshot())
        .expect("empty registry cancellation should still validate");
    assert!(!outcome.matched());
    assert_eq!(outcome.fully_matched(), 0);
    assert_eq!(outcome.partially_matched(), 0);
    assert_eq!(outcome.cancelled(), 0);
    assert_eq!(outcome.already_cancelled(), 0);
    assert_eq!(outcome.already_completed(), 0);
    assert_eq!(registry.metrics().cancellation_callbacks(), 1);
    assert_eq!(registry.metrics().unmatched_cancellations(), 1);
    assert_eq!(registry.metrics().fully_matched_waiters(), 0);
    assert_eq!(registry.metrics().partially_matched_waiters(), 0);
    assert_eq!(registry.metrics().cancelled_waiters(), 0);
    assert_eq!(registry.metrics().already_cancelled_waiters(), 0);
    assert_eq!(registry.metrics().completion_races(), 0);
    assert!(format!("{registry:?}").contains("active_waiters"));
}

#[test]
fn preflight_and_completed_notifications_keep_owned_paths_and_generation_leases() {
    let session = WindowsConnectionSession::new(generation(7));
    let preflight = WindowsCallbackRequest::preflight(
        WindowsPreflightSnapshot::Rename {
            info: info_snapshot(),
            flags: u32::MAX,
            target_path: Some(PathBuf::from(r"\renamed.txt")),
        },
        session
            .begin_callback(generation(7))
            .expect("preflight fixture should acquire a lease"),
    )
    .expect("preflight fixture should bind generation");
    assert_eq!(preflight.generation(), generation(7));
    let WindowsCallbackRequest::Preflight(preflight) = preflight else {
        panic!("fixture should create a preflight request");
    };
    assert!(matches!(
        preflight.snapshot(),
        WindowsPreflightSnapshot::Rename {
            flags: u32::MAX,
            target_path: Some(path),
            ..
        } if path == &PathBuf::from(r"\renamed.txt")
    ));

    let observation = WindowsCallbackRequest::observation(
        WindowsObservedNotification::RenameCompleted {
            info: info_snapshot(),
            flags: 0x8000_0000,
            source_path: Some(PathBuf::from(r"\old.txt")),
        },
        session
            .begin_callback(generation(7))
            .expect("observation fixture should acquire a lease"),
    )
    .expect("observation fixture should bind generation");
    assert!(matches!(
        observation,
        WindowsCallbackRequest::Observation {
            notification: WindowsObservedNotification::RenameCompleted {
                flags: 0x8000_0000,
                source_path: Some(path),
                ..
            },
            ..
        } if path == std::path::Path::new(r"\old.txt")
    ));
}

#[test]
fn callback_ranges_cover_exact_eof_and_all_invalid_boundaries() {
    let signed_max = i64::MAX as u64;
    let exact = WindowsCallbackRange::exact(signed_max - 4, 4)
        .expect("range ending at i64::MAX should be valid");
    assert_eq!(exact.offset(), signed_max - 4);
    assert_eq!(exact.length(), WindowsCallbackRangeLength::Exact(4));
    assert_eq!(exact.end_exclusive(), Some(signed_max));

    let to_end =
        WindowsCallbackRange::from_cfapi(i64::MAX, -1).expect("CF_EOF should be preserved");
    assert_eq!(to_end.offset(), i64::MAX as u64);
    assert_eq!(to_end.length(), WindowsCallbackRangeLength::ToEnd);
    assert_eq!(to_end.end_exclusive(), None);

    assert!(matches!(
        WindowsCallbackRange::exact(0, 0),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "callback range length must be non-zero"
        })
    ));
    assert!(matches!(
        WindowsCallbackRange::exact(signed_max, 1),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "callback range end exceeds signed CFAPI boundary"
        })
    ));
    assert!(matches!(
        WindowsCallbackRange::exact(signed_max + 1, 1),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "callback range offset exceeds signed CFAPI boundary"
        })
    ));
    assert!(matches!(
        WindowsCallbackRange::exact(0, signed_max + 1),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "callback range length exceeds signed CFAPI boundary"
        })
    ));
    assert!(matches!(
        WindowsCallbackRange::to_end(signed_max + 1),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "callback range offset exceeds signed CFAPI boundary"
        })
    ));
    assert!(matches!(
        WindowsCallbackRange::from_cfapi(-1, 1),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "callback range offset must be non-negative"
        })
    ));
    for length in [i64::MIN, -2, 0] {
        assert!(matches!(
            WindowsCallbackRange::from_cfapi(0, length),
            Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
                reason: "callback range length must be positive or CF_EOF"
            })
        ));
    }
}

#[test]
fn callback_flag_bitsets_preserve_known_and_future_bits() {
    let fetch = WindowsFetchDataFlags::from_bits_retain(0x8000_0003);
    assert_eq!(fetch.bits(), 0x8000_0003);
    assert!(fetch.contains(WindowsFetchDataFlags::RECOVERY));
    assert!(fetch.contains(WindowsFetchDataFlags::EXPLICIT_HYDRATION));

    let cancel = WindowsCancelFetchDataFlags::from_bits_retain(0x4000_0003);
    assert_eq!(cancel.bits(), 0x4000_0003);
    assert!(cancel.contains(WindowsCancelFetchDataFlags::IO_TIMEOUT));
    assert!(cancel.contains(WindowsCancelFetchDataFlags::IO_ABORTED));
}

#[test]
fn recovery_flag_and_restart_replacement_preserve_explicit_native_state() {
    let snapshot = WindowsFetchDataSnapshot::new(
        info_snapshot_with_size(4096),
        WindowsFetchDataFlags::RECOVERY,
        WindowsCallbackRange::exact(0, 4096).expect("range fixture should be valid"),
        None,
        0,
        0,
    );
    assert!(snapshot.is_recovery());

    let item = aster_forge_cloud_files_core::CloudItem::file(
        key("namespace", "root", "item"),
        CloudItemId::new("root-item").expect("parent fixture should be valid"),
        "recovered.bin",
        aster_forge_cloud_files_core::MetadataRevision::from_slice(b"metadata-v2")
            .expect("metadata revision should be valid"),
        aster_forge_cloud_files_core::CloudContentMetadata::new(
            ContentRevision::from_slice(b"content-v2").expect("content revision should be valid"),
            None,
            4096,
        ),
    )
    .expect("item fixture should be valid");
    let metadata = aster_forge_cloud_files_windows::WindowsPlaceholderMetadata::from_item(
        &item,
        aster_forge_cloud_files_windows::WindowsFileTimes::default(),
    )
    .expect("replacement metadata should be valid");
    let identity = WindowsFileIdentity::encode(item.key()).expect("identity should encode");
    let restart = WindowsRestartHydration::new()
        .with_metadata(metadata)
        .with_identity(identity.clone())
        .mark_in_sync();
    assert_eq!(restart.metadata(), Some(metadata));
    assert_eq!(restart.identity(), Some(&identity));
    assert!(restart.should_mark_in_sync());
}

#[test]
fn callback_info_owns_every_pointer_backed_value_and_exposes_exact_scalars() {
    let info = info_snapshot();
    assert_eq!(info.generation(), generation(7));
    assert_eq!(info.connection_key().get(), i64::MIN);
    assert_eq!(info.transfer_key().get(), -42);
    assert_eq!(info.request_key().get(), i64::MAX);
    assert_eq!(
        info.volume_guid_name(),
        Some(std::ffi::OsStr::new(r"\\?\Volume{fixture}"))
    );
    assert_eq!(info.volume_dos_name(), Some(std::ffi::OsStr::new("X:")));
    assert_eq!(info.volume_serial_number(), u32::MAX);
    assert_eq!(info.sync_root_file_id(), -17);
    assert_eq!(
        info.sync_root_identity()
            .decode()
            .expect("scope should decode"),
        scope("namespace", "root")
    );
    assert_eq!(info.file_id(), i64::MAX);
    assert_eq!(info.file_size(), i64::MAX as u64);
    assert_eq!(
        info.file_identity().decode().expect("item should decode"),
        key("namespace", "root", "item")
    );
    assert_eq!(
        info.normalized_path(),
        Some(std::path::Path::new(r"\folder\file.txt"))
    );
    assert_eq!(info.priority_hint(), CFAPI_MAX_PRIORITY_HINT);

    let process = info.process().expect("process snapshot should be present");
    assert_eq!(process.process_id, u32::MAX);
    assert_eq!(
        process.image_path.as_deref(),
        Some(std::path::Path::new(r"C:\fixture.exe"))
    );
    assert_eq!(
        process.package_name.as_deref(),
        Some(std::ffi::OsStr::new("package"))
    );
    assert_eq!(
        process.application_id.as_deref(),
        Some(std::ffi::OsStr::new("application"))
    );
    assert_eq!(
        process.command_line.as_deref(),
        Some(std::ffi::OsStr::new("fixture.exe --test"))
    );
    assert_eq!(process.session_id, u32::MAX);

    let debug = format!("{info:?}");
    assert!(!debug.contains("namespace"));
    assert!(!debug.contains("item"));
    assert!(!debug.contains("fixture.exe"));
    assert!(!debug.contains("folder"));
}

#[test]
fn callback_info_rejects_priority_size_and_cross_scope_identity() {
    let make = |file_size, priority, file_identity| {
        WindowsCallbackInfoSnapshot::new(
            generation(1),
            WindowsConnectionKey::new(1),
            WindowsTransferKey::new(2),
            WindowsRequestKey::new(3),
            None,
            None,
            0,
            4,
            WindowsSyncRootIdentity::encode(&scope("namespace", "root"))
                .expect("sync root should encode"),
            5,
            file_size,
            file_identity,
            None,
            priority,
            None,
        )
    };

    let matching = WindowsFileIdentity::encode(&key("namespace", "root", "item"))
        .expect("matching identity should encode");
    assert!(matches!(
        make(0, CFAPI_MAX_PRIORITY_HINT + 1, matching.clone()),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "priority hint exceeds CFAPI maximum"
        })
    ));
    assert!(matches!(
        make(i64::MAX as u64 + 1, 0, matching),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "callback file size exceeds signed CFAPI boundary"
        })
    ));
    assert!(matches!(
        make(
            0,
            0,
            WindowsFileIdentity::encode(&key("namespace", "other-root", "item"))
                .expect("cross-scope identity should encode")
        ),
        Err(WindowsCloudFilesError::InvalidCallbackSnapshot {
            reason: "file identity belongs to another sync-root scope"
        })
    ));
}

#[test]
fn fetch_and_cancel_snapshots_preserve_all_callback_specific_fields() {
    let required = WindowsCallbackRange::exact(4, 8).expect("required range should be valid");
    let optional = WindowsCallbackRange::to_end(0).expect("to-end range should be valid");
    let fetch = WindowsFetchDataSnapshot::new(
        info_snapshot(),
        WindowsFetchDataFlags::from_bits_retain(0x83),
        required,
        Some(optional),
        i64::MIN,
        u32::MAX,
    );
    assert_eq!(fetch.info().generation(), generation(7));
    assert_eq!(fetch.flags().bits(), 0x83);
    assert_eq!(fetch.required_range(), required);
    assert_eq!(fetch.optional_range(), Some(optional));
    assert_eq!(fetch.last_dehydration_time(), i64::MIN);
    assert_eq!(fetch.last_dehydration_reason(), u32::MAX);

    let cancel = WindowsCancelFetchDataSnapshot::new(
        info_snapshot(),
        WindowsCancelFetchDataFlags::from_bits_retain(0x43),
        required,
    );
    assert_eq!(cancel.info().generation(), generation(7));
    assert_eq!(cancel.flags().bits(), 0x43);
    assert_eq!(cancel.range(), required);
}

#[test]
fn session_transition_matrix_rejects_stale_and_post_close_ingress() {
    let session = WindowsConnectionSession::new(generation(9));
    assert_eq!(session.state(), SessionState::Accepting);
    assert_eq!(session.active_callbacks(), 0);
    assert!(matches!(
        session.mark_disconnected(),
        Err(WindowsCloudFilesError::InvalidConnectionTransition {
            from: SessionState::Accepting,
            to: SessionState::Draining
        })
    ));
    assert!(matches!(
        session.begin_callback(generation(8)),
        Err(WindowsCloudFilesError::StaleConnectionGeneration {
            expected: 9,
            actual: 8
        })
    ));
    assert_eq!(session.active_callbacks(), 0);

    let first = session
        .begin_callback(generation(9))
        .expect("active generation should accept callback");
    let second = session
        .begin_callback(generation(9))
        .expect("active generation should accept concurrent callback");
    assert_eq!(first.generation(), generation(9));
    assert_eq!(session.active_callbacks(), 2);

    assert!(session.begin_closing());
    assert!(!session.begin_closing());
    assert_eq!(session.state(), SessionState::Closing);
    assert!(matches!(
        session.begin_callback(generation(9)),
        Err(WindowsCloudFilesError::ConnectionNotAccepting {
            state: SessionState::Closing
        })
    ));

    session
        .mark_disconnected()
        .expect("closing session should begin draining");
    assert_eq!(session.state(), SessionState::Draining);
    assert!(first.accepts_completion(&session));
    first.release();
    assert_eq!(session.state(), SessionState::Draining);
    assert_eq!(session.active_callbacks(), 1);
    drop(second);
    assert_eq!(session.state(), SessionState::Closed);
    assert_eq!(session.active_callbacks(), 0);
    assert!(session.mark_disconnected().is_ok());
    assert!(!session.begin_closing());
    assert!(matches!(
        session.begin_callback(generation(9)),
        Err(WindowsCloudFilesError::ConnectionNotAccepting {
            state: SessionState::Closed
        })
    ));
}

#[test]
fn disconnect_without_inflight_callbacks_closes_immediately() {
    let session = WindowsConnectionSession::new(generation(1));
    assert!(session.begin_closing());
    session
        .mark_disconnected()
        .expect("closing session should disconnect");
    assert_eq!(session.state(), SessionState::Closed);
    assert_eq!(session.active_callbacks(), 0);
}

#[test]
fn request_owns_the_lease_until_worker_releases_or_drops_it() {
    let session = WindowsConnectionSession::new(generation(7));
    let lease = session
        .begin_callback(generation(7))
        .expect("callback should be accepted");
    let request = WindowsCallbackRequest::fetch_data(
        WindowsFetchDataSnapshot::new(
            info_snapshot(),
            WindowsFetchDataFlags::default(),
            WindowsCallbackRange::exact(0, 1).expect("range should be valid"),
            None,
            0,
            0,
        ),
        lease,
    )
    .expect("snapshot and lease generations should match");
    assert_eq!(request.generation(), generation(7));
    let WindowsCallbackRequest::FetchData(fetch) = &request else {
        panic!("fetch constructor must create the fetch variant");
    };
    assert_eq!(fetch.snapshot().required_range().offset(), 0);
    assert!(!fetch.terminal_attempted());
    assert!(fetch.lease().accepts_completion(&session));
    assert_eq!(session.active_callbacks(), 1);
    assert!(session.begin_closing());
    session
        .mark_disconnected()
        .expect("session should start draining");
    assert_eq!(session.state(), SessionState::Draining);
    drop(request);
    assert_eq!(session.state(), SessionState::Closed);
}

#[test]
fn request_constructor_rejects_snapshot_and_lease_generation_mismatch() {
    let session = WindowsConnectionSession::new(generation(8));
    let lease = session
        .begin_callback(generation(8))
        .expect("generation eight should accept its callback");
    let result = WindowsCallbackRequest::fetch_data(
        WindowsFetchDataSnapshot::new(
            info_snapshot(),
            WindowsFetchDataFlags::default(),
            WindowsCallbackRange::exact(0, 1).expect("range should be valid"),
            None,
            0,
            0,
        ),
        lease,
    );
    assert!(matches!(
        result,
        Err(WindowsCloudFilesError::StaleConnectionGeneration {
            expected: 8,
            actual: 7
        })
    ));
    assert_eq!(session.active_callbacks(), 0);
}

#[test]
fn cancel_request_constructor_binds_snapshot_accessors_and_lease() {
    let session = WindowsConnectionSession::new(generation(7));
    assert_eq!(session.generation(), generation(7));
    let lease = session
        .begin_callback(generation(7))
        .expect("generation seven should accept cancellation");
    assert!(format!("{lease:?}").contains("generation"));
    let range = WindowsCallbackRange::exact(4, 4).expect("range should be valid");
    let request = WindowsCallbackRequest::cancel_fetch_data(
        WindowsCancelFetchDataSnapshot::new(
            info_snapshot(),
            WindowsCancelFetchDataFlags::IO_ABORTED,
            range,
        ),
        lease,
    )
    .expect("snapshot and lease generations should match");
    assert_eq!(request.info().generation(), generation(7));
    let WindowsCallbackRequest::CancelFetchData(cancel) = &request else {
        panic!("cancel constructor must create the cancellation variant");
    };
    assert_eq!(cancel.snapshot().range(), range);
    assert_eq!(
        cancel.snapshot().flags(),
        WindowsCancelFetchDataFlags::IO_ABORTED
    );
    assert!(cancel.lease().accepts_completion(&session));
    drop(request);
    assert_eq!(session.active_callbacks(), 0);
}

#[test]
fn closing_fence_is_shared_across_threads_and_rejects_all_late_callbacks() {
    let session = Arc::new(WindowsConnectionSession::new(generation(11)));
    assert!(session.begin_closing());

    let handles = (0..32)
        .map(|_| {
            let session = session.clone();
            thread::spawn(move || session.begin_callback(generation(11)))
        })
        .collect::<Vec<_>>();
    for handle in handles {
        assert!(matches!(
            handle.join().expect("callback thread should not panic"),
            Err(WindowsCloudFilesError::ConnectionNotAccepting {
                state: SessionState::Closing
            })
        ));
    }
    assert_eq!(session.active_callbacks(), 0);
    session
        .mark_disconnected()
        .expect("session should close after native disconnect");
    assert_eq!(session.state(), SessionState::Closed);
}

#[test]
fn old_generation_completion_never_becomes_current_for_reconnected_session() {
    let old = WindowsConnectionSession::new(generation(20));
    let old_lease = old
        .begin_callback(generation(20))
        .expect("old session should accept work before closing");
    assert!(old.begin_closing());
    old.mark_disconnected()
        .expect("old session should drain after disconnect");

    let current = WindowsConnectionSession::new(generation(21));
    assert!(old_lease.accepts_completion(&old));
    assert!(!old_lease.accepts_completion(&current));

    drop(old_lease);
    assert_eq!(old.state(), SessionState::Closed);
    assert_eq!(current.state(), SessionState::Accepting);
}

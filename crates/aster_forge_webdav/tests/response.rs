use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavBodyError, DavCapabilityDeclaration,
    DavCompatibilityCapabilities, DavComplianceClasses, DavConditionalPlanError, DavDownloadBody,
    DavDownloadOpenError, DavDownloadPlanError, DavDownloadSource, DavLockingCapability,
    DavMetaData, DavMethod, DavMethodSet, DavMultiRangeLimits, DavMultiRangePolicy,
    DavOpenedDownload, DavPatchBodyPolicy, DavPatchCapability, DavPatchFormat, DavPath,
    DavRangeLimitBehavior, DavRequestHead, DavRequestOrigin, DavResourceState, DavResponseBody,
    DavVersioningCapability, DavWriteCapabilities, DavWritePrecondition, DavXmlError,
    backend_error_response, body_error_response, conditional_plan_error_response,
    method_not_allowed_response, open_download, options_response, plan_capabilities,
    plan_download_response, plan_download_response_with_multi_range, propfind_xml_error_response,
    protocol_error_response, range_not_satisfiable_response,
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use http::StatusCode;
use http::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH, IF_RANGE, LAST_MODIFIED, RANGE,
};
use http::{HeaderMap, HeaderValue, Uri};

fn representation_time() -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(784_111_777)
}

fn full_download_body(expected_length: u64) -> DavDownloadBody {
    DavDownloadBody::Full { expected_length }
}

fn multi_range_policy(
    limits: DavMultiRangeLimits,
    coalesce_gap_bytes: u64,
    limit_behavior: DavRangeLimitBehavior,
) -> DavMultiRangePolicy {
    DavMultiRangePolicy::new(limits, coalesce_gap_bytes, limit_behavior)
}

fn generous_multi_range_policy() -> DavMultiRangePolicy {
    multi_range_policy(
        DavMultiRangeLimits::new(4096, 16, 16, u64::MAX, 16),
        0,
        DavRangeLimitBehavior::RangeNotSatisfiable,
    )
}

struct TestDownloadSource {
    full: Result<u64, DavBackendErrorKind>,
    range: Result<u64, DavBackendErrorKind>,
    calls: AtomicUsize,
}

struct TestDownloadMetadata;

struct MultipartStreamSpec {
    declared_length: u64,
    chunks: VecDeque<Result<Bytes, DavBackendErrorKind>>,
}

impl MultipartStreamSpec {
    fn bytes(declared_length: u64, chunks: &[&'static [u8]]) -> Self {
        Self {
            declared_length,
            chunks: chunks
                .iter()
                .map(|chunk| Ok(Bytes::from_static(chunk)))
                .collect(),
        }
    }

    fn with_error(declared_length: u64, chunks: Vec<Result<Bytes, DavBackendErrorKind>>) -> Self {
        Self {
            declared_length,
            chunks: chunks.into(),
        }
    }
}

struct MultipartDownloadSource {
    opens: Mutex<VecDeque<Result<MultipartStreamSpec, DavBackendErrorKind>>>,
    ranges: Mutex<Vec<aster_forge_utils::http_range::HttpByteRange>>,
    dropped_streams: Arc<AtomicUsize>,
}

impl MultipartDownloadSource {
    fn new(opens: Vec<Result<MultipartStreamSpec, DavBackendErrorKind>>) -> Self {
        Self {
            opens: Mutex::new(opens.into()),
            ranges: Mutex::new(Vec::new()),
            dropped_streams: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn opened_ranges(&self) -> Vec<aster_forge_utils::http_range::HttpByteRange> {
        self.ranges.lock().expect("range calls lock").clone()
    }
}

struct DropTrackedStream {
    chunks: VecDeque<Result<Bytes, DavBackendErrorKind>>,
    dropped_streams: Arc<AtomicUsize>,
}

impl Stream for DropTrackedStream {
    type Item = Result<Bytes, DavBackendError>;

    fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(
            self.get_mut()
                .chunks
                .pop_front()
                .map(|item| item.map_err(DavBackendError::new)),
        )
    }
}

impl Drop for DropTrackedStream {
    fn drop(&mut self) {
        self.dropped_streams.fetch_add(1, Ordering::SeqCst);
    }
}

impl DavMetaData for TestDownloadMetadata {
    fn len(&self) -> u64 {
        0
    }

    fn modified(&self) -> Result<SystemTime, aster_forge_webdav::FsError> {
        Ok(UNIX_EPOCH)
    }

    fn is_dir(&self) -> bool {
        false
    }

    fn etag(&self) -> Option<String> {
        None
    }

    fn created(&self) -> Result<SystemTime, aster_forge_webdav::FsError> {
        Ok(UNIX_EPOCH)
    }
}

impl TestDownloadSource {
    fn opened(length: u64) -> DavOpenedDownload {
        DavOpenedDownload::new(Box::pin(futures::stream::empty()), length)
    }
}

impl DavDownloadSource for TestDownloadSource {
    type Metadata = TestDownloadMetadata;

    async fn metadata<'a>(&'a self, _path: &'a DavPath) -> Result<Self::Metadata, DavBackendError> {
        Err(DavBackendError::new(DavBackendErrorKind::Internal))
    }

    async fn open_full<'a>(
        &'a self,
        _path: &'a DavPath,
    ) -> Result<DavOpenedDownload, DavBackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.full.map(Self::opened).map_err(DavBackendError::new)
    }

    async fn open_range<'a>(
        &'a self,
        _path: &'a DavPath,
        _range: aster_forge_utils::http_range::HttpByteRange,
    ) -> Result<DavOpenedDownload, DavBackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.range.map(Self::opened).map_err(DavBackendError::new)
    }
}

impl DavDownloadSource for MultipartDownloadSource {
    type Metadata = TestDownloadMetadata;

    async fn metadata<'a>(&'a self, _path: &'a DavPath) -> Result<Self::Metadata, DavBackendError> {
        Err(DavBackendError::new(DavBackendErrorKind::Internal))
    }

    async fn open_full<'a>(
        &'a self,
        _path: &'a DavPath,
    ) -> Result<DavOpenedDownload, DavBackendError> {
        Err(DavBackendError::new(DavBackendErrorKind::Unsupported))
    }

    async fn open_range<'a>(
        &'a self,
        _path: &'a DavPath,
        range: aster_forge_utils::http_range::HttpByteRange,
    ) -> Result<DavOpenedDownload, DavBackendError> {
        self.ranges.lock().expect("range calls lock").push(range);
        let open = self
            .opens
            .lock()
            .expect("open plans lock")
            .pop_front()
            .expect("one open plan per selected range")
            .map_err(DavBackendError::new)?;
        let stream = DropTrackedStream {
            chunks: open.chunks,
            dropped_streams: Arc::clone(&self.dropped_streams),
        };
        Ok(DavOpenedDownload::new(
            Box::pin(stream),
            open.declared_length,
        ))
    }
}

fn full_capability_snapshot() -> aster_forge_webdav::DavCapabilitySnapshot {
    static PATCH_FORMATS: [DavPatchFormat; 1] = [DavPatchFormat {
        media_type: "application/merge-patch+json",
        body_policy: DavPatchBodyPolicy::Bounded { maximum: 4096 },
        precondition: DavWritePrecondition::RequireStrongIfMatch,
    }];
    plan_capabilities(DavCapabilityDeclaration {
        resource: DavResourceState::File,
        methods: DavMethodSet::from_methods(&DavMethod::ALL),
        locking: DavLockingCapability::Class2,
        versioning: DavVersioningCapability::Core,
        compatibility: DavCompatibilityCapabilities {
            ms_author_via: true,
        },
        writes: DavWriteCapabilities {
            patch: DavPatchCapability::Formats(&PATCH_FORMATS),
            ..DavWriteCapabilities::default()
        },
        compliance: DavComplianceClasses { class1: true },
    })
    .expect("valid complete capability declaration")
}

#[test]
fn options_and_method_not_allowed_share_the_canonical_method_set() {
    let snapshot = full_capability_snapshot();
    let options = options_response(&snapshot);
    assert_eq!(options.status, StatusCode::OK);
    assert_eq!(
        options.headers.get(ALLOW).unwrap(),
        "OPTIONS, GET, HEAD, PUT, PATCH, DELETE, MKCOL, COPY, MOVE, PROPFIND, PROPPATCH, LOCK, UNLOCK, REPORT, VERSION-CONTROL"
    );
    assert_eq!(options.headers.get("DAV").unwrap(), "1, 2, version-control");
    assert_eq!(options.headers.get("MS-Author-Via").unwrap(), "DAV");
    assert_eq!(
        options.headers.get("Accept-Patch").unwrap(),
        "application/merge-patch+json"
    );

    let rejected = method_not_allowed_response(&snapshot);
    assert_eq!(rejected.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        rejected.headers.get(ALLOW).unwrap(),
        snapshot.allow_header()
    );
    assert_eq!(rejected.headers.get(CACHE_CONTROL).unwrap(), "no-store");
}

#[test]
fn body_policy_errors_preserve_status_body_and_cache_contracts() {
    let read = body_error_response(DavBodyError::ReadFailed);
    assert_eq!(read.status, StatusCode::BAD_REQUEST);
    assert_eq!(read.headers.get(CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(
        read.headers.get(CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );
    assert!(matches!(read.body, DavResponseBody::Bytes(_)));

    let too_large = body_error_response(DavBodyError::XmlTooLarge);
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(matches!(too_large.body, DavResponseBody::Bytes(_)));

    let not_allowed = body_error_response(DavBodyError::BodyNotAllowed);
    assert_eq!(not_allowed.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(not_allowed.headers.get(CONTENT_TYPE).is_none());
    assert!(matches!(not_allowed.body, DavResponseBody::Empty));
}

#[test]
fn xml_and_conditional_representation_errors_have_stable_responses() {
    for error in [DavXmlError::TooDeep, DavXmlError::Malformed] {
        let response = propfind_xml_error_response(error).expect("XML error response");
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(response.headers.get(CACHE_CONTROL).unwrap(), "no-store");
        let DavResponseBody::Bytes(body) = response.body else {
            panic!("invalid XML should have a text body");
        };
        assert_eq!(body.as_ref(), b"Invalid XML body");
    }

    let response = conditional_plan_error_response(&DavConditionalPlanError::InvalidRepresentation);
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.headers.get(CACHE_CONTROL).unwrap(), "no-store");
    assert!(matches!(response.body, DavResponseBody::Empty));
}

#[test]
fn full_get_plan_contains_complete_response_and_storage_contract() {
    let plan = plan_download_response(
        &HeaderMap::new(),
        false,
        20,
        "application/octet-stream",
        Some("etag-1"),
        representation_time(),
    )
    .expect("full GET should plan");

    assert_eq!(plan.response.status, StatusCode::OK);
    assert_eq!(plan.body, full_download_body(20));
    assert_eq!(plan.response.headers.get(CONTENT_LENGTH).unwrap(), "20");
    assert_eq!(
        plan.response.headers.get(CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
    assert_eq!(plan.response.headers.get(ACCEPT_RANGES).unwrap(), "bytes");
    assert_eq!(
        plan.response.headers.get(CONTENT_ENCODING).unwrap(),
        "identity"
    );
    assert_eq!(plan.response.headers.get(ETAG).unwrap(), "\"etag-1\"");
    assert_eq!(
        plan.response.headers.get(LAST_MODIFIED).unwrap(),
        "Sun, 06 Nov 1994 08:49:37 GMT"
    );
    assert!(matches!(plan.response.body, DavResponseBody::Empty));
}

#[test]
fn ranged_get_plan_selects_exact_storage_offset_and_response_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(RANGE, HeaderValue::from_static("  BYTES=5-99"));
    let plan = plan_download_response(
        &headers,
        false,
        20,
        "video/mp4",
        None,
        representation_time(),
    )
    .expect("range GET should plan");

    assert_eq!(plan.response.status, StatusCode::PARTIAL_CONTENT);
    let DavDownloadBody::Range(range) = plan.body else {
        panic!("range GET should select a storage range");
    };
    assert_eq!((range.start(), range.length()), (5, 15));
    assert_eq!(plan.response.headers.get(CONTENT_LENGTH).unwrap(), "15");
    assert_eq!(
        plan.response.headers.get(CONTENT_RANGE).unwrap(),
        "bytes 5-19/20"
    );
}

#[test]
fn bounded_multi_range_plan_preserves_order_and_rfc_header_contract() {
    let mut headers = HeaderMap::new();
    headers.insert(RANGE, HeaderValue::from_static("bytes=10-12, 50-, -5, 0-1"));
    let plan = plan_download_response_with_multi_range(
        &headers,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
        generous_multi_range_policy(),
    )
    .expect("bounded multi-range should plan");

    assert_eq!(plan.response.status, StatusCode::PARTIAL_CONTENT);
    assert!(plan.response.headers.get(CONTENT_RANGE).is_none());
    assert!(
        plan.response
            .headers
            .get(CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("multipart/byteranges; boundary=aster-forge-")
    );
    let DavDownloadBody::Multipart(multipart) = &plan.body else {
        panic!("multiple requested ranges should select multipart");
    };
    assert_eq!(multipart.requested_range_count(), 4);
    assert_eq!(multipart.selected_length(), 10);
    assert_eq!(
        multipart
            .segments()
            .iter()
            .map(|segment| segment.range())
            .collect::<Vec<_>>(),
        vec![
            aster_forge_utils::http_range::HttpByteRange::new(10, 12, 20).unwrap(),
            aster_forge_utils::http_range::HttpByteRange::new(15, 19, 20).unwrap(),
            aster_forge_utils::http_range::HttpByteRange::new(0, 1, 20).unwrap(),
        ]
    );
    assert_eq!(
        plan.response.headers.get(CONTENT_LENGTH).unwrap(),
        multipart.expected_length().to_string().as_str()
    );
}

#[test]
fn multi_range_policy_keeps_one_raw_range_on_the_single_part_path() {
    let mut headers = HeaderMap::new();
    headers.insert(RANGE, HeaderValue::from_static("bytes=5-9"));
    let plan = plan_download_response_with_multi_range(
        &headers,
        false,
        20,
        "application/octet-stream",
        None,
        representation_time(),
        generous_multi_range_policy(),
    )
    .expect("single range should plan");

    let DavDownloadBody::Range(range) = plan.body else {
        panic!("one raw range must not generate multipart");
    };
    assert_eq!((range.start(), range.end()), (5, 9));
    assert_eq!(
        plan.response.headers.get(CONTENT_RANGE).unwrap(),
        "bytes 5-9/20"
    );
    assert_eq!(
        plan.response.headers.get(CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
}

#[test]
fn multi_range_coalescing_handles_overlap_adjacency_nearby_gaps_and_request_order() {
    let cases = [
        (
            "bytes=100-109,0-9,8-100,200-204,205-209,212-214",
            0,
            vec![(0, 109), (200, 209), (212, 214)],
        ),
        (
            "bytes=100-109,0-9,8-100,200-204,205-209,212-214",
            2,
            vec![(0, 109), (200, 214)],
        ),
    ];
    for (raw, gap, expected) in cases {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_str(raw).expect("range header"));
        let plan = plan_download_response_with_multi_range(
            &headers,
            false,
            300,
            "application/octet-stream",
            None,
            representation_time(),
            multi_range_policy(
                DavMultiRangeLimits::new(4096, 16, 16, 300, 16),
                gap,
                DavRangeLimitBehavior::RangeNotSatisfiable,
            ),
        )
        .expect("coalesced range plan");
        let DavDownloadBody::Multipart(multipart) = plan.body else {
            panic!("multiple ranges remain a multipart response");
        };
        assert_eq!(
            multipart
                .segments()
                .iter()
                .map(|segment| {
                    let range = segment.range();
                    (range.start(), range.end())
                })
                .collect::<Vec<_>>(),
            expected,
            "gap={gap}"
        );
    }
}

#[test]
fn multi_range_limits_are_exact_and_select_typed_ignore_or_reject_behavior() {
    let raw = "bytes=0-1,10-11";
    let mut headers = HeaderMap::new();
    headers.insert(RANGE, HeaderValue::from_static(raw));
    let exact = DavMultiRangeLimits::new(raw.len(), 2, 2, 4, 2);
    let plan = plan_download_response_with_multi_range(
        &headers,
        false,
        20,
        "text/plain",
        None,
        representation_time(),
        multi_range_policy(exact, 0, DavRangeLimitBehavior::RangeNotSatisfiable),
    )
    .expect("exact hard limits should pass");
    assert_eq!(plan.response.status, StatusCode::PARTIAL_CONTENT);

    let rejected_limits = [
        DavMultiRangeLimits::new(raw.len() - 1, 2, 2, 4, 2),
        DavMultiRangeLimits::new(raw.len(), 1, 2, 4, 2),
        DavMultiRangeLimits::new(raw.len(), 2, 1, 4, 2),
        DavMultiRangeLimits::new(raw.len(), 2, 2, 3, 2),
        DavMultiRangeLimits::new(raw.len(), 2, 2, 4, 1),
    ];
    for limits in rejected_limits {
        let plan = plan_download_response_with_multi_range(
            &headers,
            false,
            20,
            "text/plain",
            None,
            representation_time(),
            multi_range_policy(limits, 0, DavRangeLimitBehavior::RangeNotSatisfiable),
        )
        .expect("limit rejection should be a response plan");
        assert_eq!(plan.response.status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(plan.body, DavDownloadBody::Empty);
    }

    let ignored = plan_download_response_with_multi_range(
        &headers,
        false,
        20,
        "text/plain",
        None,
        representation_time(),
        multi_range_policy(
            DavMultiRangeLimits::new(raw.len(), 1, 1, 1, 1),
            0,
            DavRangeLimitBehavior::IgnoreRange,
        ),
    )
    .expect("limit ignore should plan a complete response");
    assert_eq!(ignored.response.status, StatusCode::OK);
    assert_eq!(ignored.body, full_download_body(20));
}

#[test]
fn multi_range_rejects_duplicate_or_malformed_fields_and_ignores_unknown_units() {
    let mut duplicate = HeaderMap::new();
    duplicate.append(RANGE, HeaderValue::from_static("bytes=0-1"));
    duplicate.append(RANGE, HeaderValue::from_static("bytes=10-11"));
    let duplicate = plan_download_response_with_multi_range(
        &duplicate,
        false,
        20,
        "text/plain",
        None,
        representation_time(),
        generous_multi_range_policy(),
    )
    .expect("duplicate fields should plan a rejection");
    assert_eq!(duplicate.response.status, StatusCode::RANGE_NOT_SATISFIABLE);

    for (raw, expected) in [
        ("bytes=0-1,broken", StatusCode::RANGE_NOT_SATISFIABLE),
        ("bytes=100-200,300-400", StatusCode::RANGE_NOT_SATISFIABLE),
        ("items=0-1,10-11", StatusCode::OK),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_str(raw).expect("range header"));
        let plan = plan_download_response_with_multi_range(
            &headers,
            false,
            20,
            "text/plain",
            None,
            representation_time(),
            generous_multi_range_policy(),
        )
        .expect("range result should plan");
        assert_eq!(plan.response.status, expected, "{raw}");
    }
}

#[test]
fn head_ignores_even_an_unsatisfiable_range_and_never_selects_a_body() {
    let mut headers = HeaderMap::new();
    headers.insert(RANGE, HeaderValue::from_static("bytes=20-99"));
    let plan = plan_download_response(
        &headers,
        true,
        20,
        "text/plain",
        None,
        representation_time(),
    )
    .expect("HEAD should ignore Range");

    assert_eq!(plan.response.status, StatusCode::OK);
    assert_eq!(plan.body, DavDownloadBody::Empty);
    assert_eq!(plan.response.headers.get(CONTENT_LENGTH).unwrap(), "20");
    assert!(plan.response.headers.get(CONTENT_RANGE).is_none());
}

#[test]
fn malformed_unsatisfiable_and_empty_ranges_return_complete_416_shells() {
    for (raw, content_length) in [("bytes=-0", 20), ("bytes=20-", 20), ("bytes=0-0", 0)] {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static(raw));
        let plan = plan_download_response(
            &headers,
            false,
            content_length,
            "application/octet-stream",
            None,
            representation_time(),
        )
        .expect("invalid range should produce a response plan");

        assert_eq!(plan.response.status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(plan.body, DavDownloadBody::Empty);
        assert_eq!(
            plan.response
                .headers
                .get(CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("bytes */{content_length}")
        );
        assert_eq!(plan.response.headers.get(ACCEPT_RANGES).unwrap(), "bytes");
        assert_eq!(
            plan.response.headers.get(LAST_MODIFIED).unwrap(),
            "Sun, 06 Nov 1994 08:49:37 GMT"
        );
        assert_eq!(
            plan.response.headers.get(CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }
}

#[test]
fn conditional_outcomes_skip_range_evaluation() {
    let mut not_modified = HeaderMap::new();
    not_modified.insert(IF_NONE_MATCH, HeaderValue::from_static("\"etag-1\""));
    not_modified.insert(RANGE, HeaderValue::from_static("bytes=99-100"));
    let plan = plan_download_response(
        &not_modified,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    )
    .expect("304 skips Range");
    assert_eq!(plan.response.status, StatusCode::NOT_MODIFIED);
    assert!(plan.response.headers.get(CONTENT_RANGE).is_none());

    let mut failed = HeaderMap::new();
    failed.insert(IF_MATCH, HeaderValue::from_static("\"other\""));
    failed.insert(RANGE, HeaderValue::from_static("bytes=99-100"));
    let plan = plan_download_response(
        &failed,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    )
    .expect("412 skips Range");
    assert_eq!(plan.response.status, StatusCode::PRECONDITION_FAILED);
    assert!(plan.response.headers.get(CONTENT_RANGE).is_none());
}

#[test]
fn unsupported_and_multiple_ranges_fall_back_to_the_complete_representation() {
    for raw in ["items=0-1", "bytes=0-1,3-4"] {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static(raw));
        let plan = plan_download_response(
            &headers,
            false,
            20,
            "application/octet-stream",
            None,
            representation_time(),
        )
        .expect("unsupported range form should be ignored");

        assert_eq!(plan.response.status, StatusCode::OK);
        assert_eq!(plan.body, full_download_body(20));
        assert_eq!(plan.response.headers.get(CONTENT_LENGTH).unwrap(), "20");
        assert!(plan.response.headers.get(CONTENT_RANGE).is_none());
    }
}

#[test]
fn if_range_requires_a_matching_strong_validator() {
    for (if_range, expected_status, expected_body) in [
        (
            "\"etag-1\"",
            StatusCode::PARTIAL_CONTENT,
            DavDownloadBody::Range(
                aster_forge_utils::http_range::HttpByteRange::new(0, 1, 20).unwrap(),
            ),
        ),
        ("\"other\"", StatusCode::OK, full_download_body(20)),
        ("W/\"etag-1\"", StatusCode::OK, full_download_body(20)),
        ("not-a-validator", StatusCode::OK, full_download_body(20)),
        (
            "Sun, 06 Nov 1994 08:49:37 GMT",
            StatusCode::PARTIAL_CONTENT,
            DavDownloadBody::Range(
                aster_forge_utils::http_range::HttpByteRange::new(0, 1, 20).unwrap(),
            ),
        ),
        (
            "Sun, 06 Nov 1994 08:49:36 GMT",
            StatusCode::OK,
            full_download_body(20),
        ),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-1"));
        headers.insert(
            IF_RANGE,
            HeaderValue::from_str(if_range).expect("valid test header"),
        );
        let plan = plan_download_response(
            &headers,
            false,
            20,
            "application/octet-stream",
            Some("etag-1"),
            representation_time(),
        )
        .expect("If-Range should plan");

        assert_eq!(plan.response.status, expected_status, "{if_range}");
        assert_eq!(plan.body, expected_body, "{if_range}");
    }
}

#[test]
fn conditional_downloads_plan_distinct_304_and_412_responses() {
    let mut not_modified = HeaderMap::new();
    not_modified.insert(IF_NONE_MATCH, HeaderValue::from_static("\"etag-1\""));
    let plan = plan_download_response(
        &not_modified,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    )
    .expect("matching If-None-Match should plan 304");
    assert_eq!(plan.response.status, StatusCode::NOT_MODIFIED);
    assert_eq!(plan.body, DavDownloadBody::Empty);
    assert_eq!(plan.response.headers.get(ETAG).unwrap(), "\"etag-1\"");
    assert!(plan.response.headers.get(CONTENT_LENGTH).is_none());

    let mut failed = HeaderMap::new();
    failed.insert(IF_MATCH, HeaderValue::from_static("\"other\""));
    let plan = plan_download_response(
        &failed,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    )
    .expect("mismatched If-Match should produce a response plan");
    assert_eq!(plan.response.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(plan.body, DavDownloadBody::Empty);
    assert_eq!(plan.response.headers.get(ETAG).unwrap(), "\"etag-1\"");
    assert_eq!(
        plan.response.headers.get(CACHE_CONTROL).unwrap(),
        "no-store"
    );
}

#[test]
fn invalid_product_metadata_is_not_misclassified_as_a_request_error() {
    let error = match plan_download_response(
        &HeaderMap::new(),
        false,
        20,
        "text/plain\ninvalid",
        None,
        representation_time(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("invalid content type should fail response planning"),
    };
    assert_eq!(error, DavDownloadPlanError::InvalidRepresentation);

    let plan = plan_download_response(
        &HeaderMap::new(),
        false,
        20,
        "text/plain",
        Some("etag\ninvalid"),
        representation_time(),
    )
    .expect("unconditional download should skip an unrenderable ETag");
    assert_eq!(plan.response.status, StatusCode::OK);
    assert!(plan.response.headers.get(ETAG).is_none());

    let mut conditional = HeaderMap::new();
    conditional.insert(IF_MATCH, HeaderValue::from_static("\"current\""));
    let error = match plan_download_response(
        &conditional,
        false,
        20,
        "text/plain",
        Some("etag\ninvalid"),
        representation_time(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("If-Match requires a representable product ETag"),
    };
    assert_eq!(error, DavDownloadPlanError::InvalidRepresentation);
}

#[test]
fn malformed_download_conditionals_remain_protocol_errors() {
    let mut headers = HeaderMap::new();
    headers.insert(IF_MATCH, HeaderValue::from_static("bare-etag"));
    let error = match plan_download_response(
        &headers,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("malformed If-Match should remain a request error"),
    };
    assert!(matches!(error, DavDownloadPlanError::Protocol(_)));
}

#[test]
fn non_utf8_range_fields_and_strong_if_range_boundaries_are_explicit() {
    let mut non_utf8_range = HeaderMap::new();
    non_utf8_range.insert(
        RANGE,
        HeaderValue::from_bytes(&[0xff]).expect("opaque Range value"),
    );
    let plan = plan_download_response(
        &non_utf8_range,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    )
    .expect("opaque Range produces a 416 plan");
    assert_eq!(plan.response.status, StatusCode::RANGE_NOT_SATISFIABLE);

    let mut non_utf8_if_range = HeaderMap::new();
    non_utf8_if_range.insert(RANGE, HeaderValue::from_static("bytes=0-1"));
    non_utf8_if_range.insert(
        IF_RANGE,
        HeaderValue::from_bytes(&[0xff]).expect("opaque If-Range value"),
    );
    let plan = plan_download_response(
        &non_utf8_if_range,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    )
    .expect("opaque If-Range is ignored");
    assert_eq!(plan.response.status, StatusCode::OK);
    assert_eq!(plan.body, full_download_body(20));

    for (current, candidate) in [
        (Some("etag-1"), "\"a\"b\""),
        (None, "\"etag-1\""),
        (Some("W/\"etag-1\""), "\"etag-1\""),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-1"));
        headers.insert(
            IF_RANGE,
            HeaderValue::from_str(candidate).expect("valid If-Range test value"),
        );
        let plan = plan_download_response(
            &headers,
            false,
            20,
            "text/plain",
            current,
            representation_time(),
        )
        .expect("non-matching strong If-Range falls back to full response");
        assert_eq!(plan.response.status, StatusCode::OK, "{candidate}");
        assert_eq!(plan.body, full_download_body(20), "{candidate}");
    }
}

#[test]
fn direct_416_builder_handles_zero_and_maximum_representation_lengths() {
    for content_length in [0, u64::MAX] {
        let response = range_not_satisfiable_response(content_length);
        assert_eq!(response.status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response
                .headers
                .get(CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("bytes */{content_length}")
        );
    }
}

#[test]
fn protocol_and_backend_failures_are_mapped_by_forge() {
    let error = DavRequestHead::parse_target(
        &"/outside".parse::<Uri>().expect("URI"),
        "/webdav",
        &DavRequestOrigin {
            scheme: "https".to_owned(),
            host: "dav.example".to_owned(),
        },
    )
    .expect_err("outside mount should fail");
    let protocol = protocol_error_response(&error);
    assert_eq!(protocol.status, StatusCode::BAD_REQUEST);
    assert_eq!(protocol.headers.get(CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(
        protocol.headers.get(CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );

    for (kind, expected) in [
        (DavBackendErrorKind::NotFound, StatusCode::NOT_FOUND),
        (DavBackendErrorKind::Forbidden, StatusCode::FORBIDDEN),
        (DavBackendErrorKind::Conflict, StatusCode::CONFLICT),
        (DavBackendErrorKind::AlreadyExists, StatusCode::CONFLICT),
        (
            DavBackendErrorKind::InsufficientStorage,
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        (
            DavBackendErrorKind::PayloadTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (DavBackendErrorKind::Locked, StatusCode::LOCKED),
        (DavBackendErrorKind::InvalidInput, StatusCode::BAD_REQUEST),
        (
            DavBackendErrorKind::Unsupported,
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            DavBackendErrorKind::Internal,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let response = backend_error_response(&DavBackendError::new(kind));
        assert_eq!(response.status, expected);
        assert_eq!(response.headers.get(CACHE_CONTROL).unwrap(), "no-store");
    }
}

#[test]
fn planned_download_opens_full_and_range_sources_with_exact_lengths() {
    futures::executor::block_on(async {
        let path = DavPath::new("/video.mp4").expect("path");
        let source = TestDownloadSource {
            full: Ok(20),
            range: Ok(15),
            calls: AtomicUsize::new(0),
        };
        assert!(matches!(
            source.metadata(&path).await,
            Err(DavBackendError {
                kind: DavBackendErrorKind::Internal,
            })
        ));
        let full = open_download(&source, &path, full_download_body(20))
            .await
            .expect("full source should open")
            .expect("full response has a body");
        assert_eq!(full.expected_length, 20);

        let range = aster_forge_utils::http_range::HttpByteRange::new(5, 19, 20).expect("range");
        let ranged = open_download(&source, &path, DavDownloadBody::Range(range))
            .await
            .expect("range source should open")
            .expect("range response has a body");
        assert_eq!(ranged.expected_length, 15);
        assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    });
}

#[test]
fn planned_download_rejects_backend_failures_and_length_drift_without_opening_empty_body() {
    futures::executor::block_on(async {
        let path = DavPath::new("/video.mp4").expect("path");
        let backend = TestDownloadSource {
            full: Err(DavBackendErrorKind::Forbidden),
            range: Ok(15),
            calls: AtomicUsize::new(0),
        };
        assert!(matches!(
            open_download(&backend, &path, full_download_body(20)).await,
            Err(DavDownloadOpenError::Backend(DavBackendError {
                kind: DavBackendErrorKind::Forbidden,
            }))
        ));

        let mismatch = TestDownloadSource {
            full: Ok(19),
            range: Ok(14),
            calls: AtomicUsize::new(0),
        };
        assert!(matches!(
            open_download(&mismatch, &path, full_download_body(20)).await,
            Err(DavDownloadOpenError::LengthMismatch {
                planned: 20,
                opened: 19,
            })
        ));
        let range = aster_forge_utils::http_range::HttpByteRange::new(5, 19, 20).expect("range");
        assert!(matches!(
            open_download(&mismatch, &path, DavDownloadBody::Range(range)).await,
            Err(DavDownloadOpenError::LengthMismatch {
                planned: 15,
                opened: 14,
            })
        ));

        let empty = TestDownloadSource {
            full: Err(DavBackendErrorKind::Internal),
            range: Err(DavBackendErrorKind::Internal),
            calls: AtomicUsize::new(0),
        };
        assert!(
            open_download(&empty, &path, DavDownloadBody::Empty)
                .await
                .expect("empty response should not open a source")
                .is_none()
        );
        assert_eq!(empty.calls.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn multipart_download_opens_each_final_segment_once_and_streams_only_framing_and_bytes() {
    futures::executor::block_on(async {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-2,10-11"));
        let plan = plan_download_response_with_multi_range(
            &headers,
            false,
            20,
            "text/plain",
            None,
            representation_time(),
            generous_multi_range_policy(),
        )
        .expect("multipart plan");
        let content_type = plan
            .response
            .headers
            .get(CONTENT_TYPE)
            .expect("multipart content type")
            .to_str()
            .expect("content type text");
        let boundary = content_type
            .strip_prefix("multipart/byteranges; boundary=")
            .expect("boundary parameter");
        assert!(!boundary.is_empty());

        let source = MultipartDownloadSource::new(vec![
            Ok(MultipartStreamSpec::bytes(3, &[b"a", b"bc"])),
            Ok(MultipartStreamSpec::bytes(2, &[b"de"])),
        ]);
        let path = DavPath::new("/video.mp4").expect("path");
        let opened = open_download(&source, &path, plan.body)
            .await
            .expect("multipart backend opens")
            .expect("multipart body");
        assert_eq!(
            opened.expected_length,
            plan.response
                .headers
                .get(CONTENT_LENGTH)
                .unwrap()
                .to_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        );
        assert_eq!(
            source.opened_ranges(),
            vec![
                aster_forge_utils::http_range::HttpByteRange::new(0, 2, 20).unwrap(),
                aster_forge_utils::http_range::HttpByteRange::new(10, 11, 20).unwrap(),
            ]
        );

        let body = opened
            .stream
            .map(|item| item.expect("multipart stream should complete"))
            .collect::<Vec<_>>()
            .await;
        let mut expected = Vec::new();
        expected.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Type: text/plain\r\nContent-Range: bytes 0-2/20\r\n\r\n"
            )
            .as_bytes(),
        );
        expected.extend_from_slice(b"abc");
        expected.extend_from_slice(format!(
            "\r\n--{boundary}\r\nContent-Type: text/plain\r\nContent-Range: bytes 10-11/20\r\n\r\n"
        )
        .as_bytes());
        expected.extend_from_slice(b"de");
        expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let actual = body
            .into_iter()
            .flat_map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    });
}

#[test]
fn multipart_backend_open_failures_happen_before_a_response_stream_is_returned() {
    futures::executor::block_on(async {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-1,10-11"));
        let plan = plan_download_response_with_multi_range(
            &headers,
            false,
            20,
            "application/octet-stream",
            None,
            representation_time(),
            generous_multi_range_policy(),
        )
        .expect("multipart plan");
        let source = MultipartDownloadSource::new(vec![
            Ok(MultipartStreamSpec::bytes(2, &[b"ab"])),
            Err(DavBackendErrorKind::Forbidden),
        ]);
        let path = DavPath::new("/video.mp4").expect("path");
        assert!(matches!(
            open_download(&source, &path, plan.body).await,
            Err(DavDownloadOpenError::Backend(DavBackendError {
                kind: DavBackendErrorKind::Forbidden,
            }))
        ));
        assert_eq!(source.opened_ranges().len(), 2);
        assert_eq!(source.dropped_streams.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn multipart_length_drift_and_stream_failures_never_pad_or_emit_a_closing_boundary() {
    futures::executor::block_on(async {
        let path = DavPath::new("/video.mp4").expect("path");
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-2,10-11"));

        let mismatch_plan = plan_download_response_with_multi_range(
            &headers,
            false,
            20,
            "text/plain",
            None,
            representation_time(),
            generous_multi_range_policy(),
        )
        .expect("multipart plan");
        let mismatch_source = MultipartDownloadSource::new(vec![
            Ok(MultipartStreamSpec::bytes(3, &[b"abc"])),
            Ok(MultipartStreamSpec::bytes(1, &[b"d"])),
        ]);
        assert!(matches!(
            open_download(&mismatch_source, &path, mismatch_plan.body).await,
            Err(DavDownloadOpenError::LengthMismatch {
                planned: 2,
                opened: 1,
            })
        ));
        assert_eq!(mismatch_source.dropped_streams.load(Ordering::SeqCst), 2);

        for spec in [
            MultipartStreamSpec::bytes(3, &[b"ab"]),
            MultipartStreamSpec::with_error(
                3,
                vec![
                    Ok(Bytes::from_static(b"ab")),
                    Err(DavBackendErrorKind::Forbidden),
                ],
            ),
            MultipartStreamSpec::with_error(3, vec![Ok(Bytes::from_static(b"abcd"))]),
        ] {
            let plan = plan_download_response_with_multi_range(
                &headers,
                false,
                20,
                "text/plain",
                None,
                representation_time(),
                generous_multi_range_policy(),
            )
            .expect("multipart plan");
            let source = MultipartDownloadSource::new(vec![
                Ok(spec),
                Ok(MultipartStreamSpec::bytes(2, &[b"de"])),
            ]);
            let opened = open_download(&source, &path, plan.body)
                .await
                .expect("backend opens")
                .expect("stream body");
            let chunks = opened.stream.collect::<Vec<_>>().await;
            assert!(chunks.iter().any(Result::is_err));
            let body = chunks
                .into_iter()
                .filter_map(Result::ok)
                .flat_map(|chunk| chunk.to_vec())
                .collect::<Vec<_>>();
            assert!(!body.windows(4).any(|window| window == b"\r\n--"));
        }
    });
}

#[test]
fn multipart_stream_drop_cancels_remaining_backend_streams_without_more_reads() {
    futures::executor::block_on(async {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-2,10-11"));
        let plan = plan_download_response_with_multi_range(
            &headers,
            false,
            20,
            "text/plain",
            None,
            representation_time(),
            generous_multi_range_policy(),
        )
        .expect("multipart plan");
        let source = MultipartDownloadSource::new(vec![
            Ok(MultipartStreamSpec::bytes(3, &[b"abc"])),
            Ok(MultipartStreamSpec::bytes(2, &[b"de"])),
        ]);
        let path = DavPath::new("/video.mp4").expect("path");
        let opened = open_download(&source, &path, plan.body)
            .await
            .expect("backend opens")
            .expect("body");
        let mut stream = opened.stream;
        let _ = stream.next().await.expect("first framing");
        let _ = stream.next().await.expect("first body");
        drop(stream);
        assert_eq!(source.dropped_streams.load(Ordering::SeqCst), 2);
    });
}

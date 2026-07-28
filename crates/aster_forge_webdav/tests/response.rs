use std::time::{Duration, UNIX_EPOCH};

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavBodyError, DavCapabilityDeclaration,
    DavCompatibilityCapabilities, DavComplianceClasses, DavConditionalPlanError, DavDownloadBody,
    DavDownloadPlanError, DavLockingCapability, DavMethod, DavMethodSet, DavRequestHead,
    DavRequestOrigin, DavResourceState, DavResponseBody, DavVersioningCapability, DavXmlError,
    backend_error_response, body_error_response, conditional_plan_error_response,
    method_not_allowed_response, options_response, plan_capabilities, plan_download_response,
    propfind_xml_error_response, protocol_error_response, range_not_satisfiable_response,
};
use http::StatusCode;
use http::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH, IF_RANGE, LAST_MODIFIED, RANGE,
};
use http::{HeaderMap, HeaderValue, Uri};

fn representation_time() -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(784_111_777)
}

fn full_capability_snapshot() -> aster_forge_webdav::DavCapabilitySnapshot {
    plan_capabilities(DavCapabilityDeclaration {
        resource: DavResourceState::File,
        methods: DavMethodSet::from_methods(&DavMethod::ALL),
        locking: DavLockingCapability::Class2,
        versioning: DavVersioningCapability::Core,
        compatibility: DavCompatibilityCapabilities {
            ms_author_via: true,
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
        "OPTIONS, GET, HEAD, PUT, DELETE, MKCOL, COPY, MOVE, PROPFIND, PROPPATCH, LOCK, UNLOCK, REPORT, VERSION-CONTROL"
    );
    assert_eq!(options.headers.get("DAV").unwrap(), "1, 2, version-control");
    assert_eq!(options.headers.get("MS-Author-Via").unwrap(), "DAV");

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
    assert_eq!(plan.body, DavDownloadBody::Full);
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
        assert_eq!(plan.body, DavDownloadBody::Full);
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
        ("\"other\"", StatusCode::OK, DavDownloadBody::Full),
        ("W/\"etag-1\"", StatusCode::OK, DavDownloadBody::Full),
        ("not-a-validator", StatusCode::OK, DavDownloadBody::Full),
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
            DavDownloadBody::Full,
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
    assert_eq!(plan.body, DavDownloadBody::Full);

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
        assert_eq!(plan.body, DavDownloadBody::Full, "{candidate}");
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

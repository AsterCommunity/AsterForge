use std::time::{Duration, UNIX_EPOCH};

use aster_forge_webdav::{
    DAV_ALLOW_HEADER, DavBodyError, DavDownloadBody, DavDownloadPlanError, DavProtocolErrorKind,
    DavResponseBody, body_error_response, method_not_allowed_response, options_response,
    plan_download_response, range_not_satisfiable_response,
};
use http::StatusCode;
use http::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH, LAST_MODIFIED, RANGE,
};
use http::{HeaderMap, HeaderValue};

fn representation_time() -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(784_111_777)
}

#[test]
fn options_and_method_not_allowed_share_the_canonical_method_set() {
    let options = options_response();
    assert_eq!(options.status, StatusCode::OK);
    assert_eq!(options.headers.get(ALLOW).unwrap(), DAV_ALLOW_HEADER);
    assert_eq!(options.headers.get("DAV").unwrap(), "1, 2, version-control");
    assert_eq!(options.headers.get("MS-Author-Via").unwrap(), "DAV");

    let rejected = method_not_allowed_response();
    assert_eq!(rejected.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(rejected.headers.get(ALLOW).unwrap(), DAV_ALLOW_HEADER);
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
    headers.insert(RANGE, HeaderValue::from_static("bytes=5-99"));
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
    for (raw, content_length) in [
        ("items=0-1", 20),
        ("bytes=0-1,3-4", 20),
        ("bytes=-0", 20),
        ("bytes=20-", 20),
        ("bytes=0-0", 0),
    ] {
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
            plan.response.headers.get(CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }
}

#[test]
fn conditional_downloads_plan_304_or_propagate_precondition_failure() {
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
    let error = match plan_download_response(
        &failed,
        false,
        20,
        "text/plain",
        Some("etag-1"),
        representation_time(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("mismatched If-Match should fail"),
    };
    let DavDownloadPlanError::Protocol(error) = error else {
        panic!("precondition failure should remain a protocol error");
    };
    assert_eq!(error.kind(), DavProtocolErrorKind::PreconditionFailed);
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

    let error = match plan_download_response(
        &HeaderMap::new(),
        false,
        20,
        "text/plain",
        Some("etag\ninvalid"),
        representation_time(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("invalid ETag should fail response planning"),
    };
    assert_eq!(error, DavDownloadPlanError::InvalidRepresentation);
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

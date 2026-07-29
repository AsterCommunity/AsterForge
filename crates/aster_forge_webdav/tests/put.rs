use aster_forge_webdav::{
    DavCapabilityDeclaration, DavCapabilitySnapshot, DavMethod, DavMethodSet,
    DavPartialPutCapability, DavPath, DavPrivateUpdateRangeCapability, DavProtocolErrorKind,
    DavPutPlanError, DavPutResourceState, DavPutWritePlan, DavResourceState, DavWriteCapabilities,
    DavWritePrecondition, plan_capabilities, plan_put_request, put_plan_error_response,
    put_success_response,
};
use http::header::{
    CONTENT_LENGTH, CONTENT_LOCATION, CONTENT_RANGE, ETAG, IF_MATCH, IF_NONE_MATCH,
    IF_UNMODIFIED_SINCE,
};
use http::{HeaderMap, HeaderValue, StatusCode};

fn put_snapshot(writes: DavWriteCapabilities) -> DavCapabilitySnapshot {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Put]),
    );
    declaration.writes = writes;
    plan_capabilities(declaration).expect("valid PUT capability snapshot")
}

fn ordinary_put_snapshot() -> DavCapabilitySnapshot {
    put_snapshot(DavWriteCapabilities::default())
}

fn partial_put_snapshot(precondition: DavWritePrecondition) -> DavCapabilitySnapshot {
    put_snapshot(DavWriteCapabilities {
        partial_put: DavPartialPutCapability::ContentRangeBytes { precondition },
        ..DavWriteCapabilities::default()
    })
}

fn existing(etag: Option<&str>) -> DavPutResourceState<'_> {
    DavPutResourceState::File {
        etag,
        last_modified: None,
    }
}

#[test]
fn ordinary_put_selects_replace_create_modes_and_expected_length_precedence() {
    let snapshot = ordinary_put_snapshot();
    let mut headers = HeaderMap::new();
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("4"));
    headers.insert("X-Expected-Entity-Length", HeaderValue::from_static("8"));
    let plan =
        plan_put_request(&snapshot, &headers, DavPutResourceState::Missing).expect("PUT plan");
    assert!(!plan.resource_existed);
    assert!(plan.create);
    assert!(plan.create_new);
    assert_eq!(plan.content_length_hint, Some(8));
    assert_eq!(plan.write, DavPutWritePlan::Replace);

    headers.insert(
        "X-Expected-Entity-Length",
        HeaderValue::from_static("invalid"),
    );
    let plan =
        plan_put_request(&snapshot, &headers, DavPutResourceState::Missing).expect("fallback plan");
    assert_eq!(plan.content_length_hint, Some(4));
}

#[test]
fn put_requires_the_method_and_rejects_undeclared_partial_write_headers() {
    let disabled = plan_capabilities(DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options]),
    ))
    .expect("snapshot without PUT");
    assert!(matches!(
        plan_put_request(&disabled, &HeaderMap::new(), existing(None)),
        Err(DavPutPlanError::MethodNotAllowed)
    ));
    assert_eq!(
        put_plan_error_response(&DavPutPlanError::MethodNotAllowed).status,
        StatusCode::METHOD_NOT_ALLOWED
    );

    let snapshot = ordinary_put_snapshot();
    for (name, value) in [
        (CONTENT_RANGE, "bytes 0-3/10"),
        (
            http::header::HeaderName::from_static("x-update-range"),
            "0-3",
        ),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_static(value));
        let error = plan_put_request(&snapshot, &headers, existing(Some("v1")))
            .expect_err("undeclared partial write must be rejected");
        let response = put_plan_error_response(&error);
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
    }
}

#[test]
fn partial_put_parses_exact_inclusive_ranges_without_allocating_a_body() {
    let snapshot = partial_put_snapshot(DavWritePrecondition::Optional);
    for (raw, offset, length, complete_length) in [
        ("bytes 0-0/1", 0, 1, Some(1)),
        ("bytes 0-3/10", 0, 4, Some(10)),
        ("bytes 4-9/*", 4, 6, None),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, HeaderValue::from_static(raw));
        let plan =
            plan_put_request(&snapshot, &headers, existing(Some("v1"))).expect("valid partial PUT");
        assert_eq!(plan.content_length_hint, Some(length));
        assert!(plan.resource_existed);
        assert!(matches!(
            plan.write,
            DavPutWritePlan::Partial(partial)
                if partial.offset == offset
                    && partial.length == length
                    && partial.complete_length == complete_length
        ));
    }
}

#[test]
fn partial_put_rejects_every_invalid_content_range_boundary() {
    let snapshot = partial_put_snapshot(DavWritePrecondition::Optional);
    for raw in [
        "bytes */10",
        "bytes 4-3/10",
        "bytes 0-9/9",
        "bytes 0-9/8",
        "bytes 0-18446744073709551615/*",
        "items 0-3/10",
        "bytes 0-3",
        "bytes 0-3/18446744073709551616",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_RANGE,
            HeaderValue::from_str(raw).expect("header-safe invalid range"),
        );
        let error = plan_put_request(&snapshot, &headers, existing(Some("v1")))
            .expect_err("invalid Content-Range must fail");
        assert!(matches!(
            error,
            DavPutPlanError::Protocol(ref protocol)
                if protocol.kind() == DavProtocolErrorKind::BadRequest
        ));
    }

    let mut repeated = HeaderMap::new();
    repeated.append(CONTENT_RANGE, HeaderValue::from_static("bytes 0-1/4"));
    repeated.append(CONTENT_RANGE, HeaderValue::from_static("bytes 2-3/4"));
    assert!(matches!(
        plan_put_request(&snapshot, &repeated, existing(Some("v1"))),
        Err(DavPutPlanError::Protocol(_))
    ));

    let mut non_utf8 = HeaderMap::new();
    non_utf8.insert(
        CONTENT_RANGE,
        HeaderValue::from_bytes(b"bytes 0-1/\xff").expect("opaque header bytes"),
    );
    assert!(matches!(
        plan_put_request(&snapshot, &non_utf8, existing(Some("v1"))),
        Err(DavPutPlanError::Protocol(_))
    ));
}

#[test]
fn partial_put_validates_declared_body_length_and_existing_target() {
    let snapshot = partial_put_snapshot(DavWritePrecondition::Optional);
    let mut mismatch = HeaderMap::new();
    mismatch.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 2-5/10"));
    mismatch.insert(CONTENT_LENGTH, HeaderValue::from_static("3"));
    assert!(matches!(
        plan_put_request(&snapshot, &mismatch, existing(Some("v1"))),
        Err(DavPutPlanError::Protocol(_))
    ));

    mismatch.insert(CONTENT_LENGTH, HeaderValue::from_static("4"));
    assert!(
        plan_put_request(&snapshot, &mismatch, existing(Some("v1"))).is_ok(),
        "exact body length is accepted"
    );

    let mut invalid_length = HeaderMap::new();
    invalid_length.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 2-5/10"));
    invalid_length.insert(CONTENT_LENGTH, HeaderValue::from_static("invalid"));
    assert!(matches!(
        plan_put_request(&snapshot, &invalid_length, existing(Some("v1"))),
        Err(DavPutPlanError::Protocol(_))
    ));

    let mut range = HeaderMap::new();
    range.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 0-0/*"));
    let error = plan_put_request(&snapshot, &range, DavPutResourceState::Missing)
        .expect_err("partial PUT needs a current representation");
    assert!(matches!(error, DavPutPlanError::PartialTargetMissing));
    assert_eq!(put_plan_error_response(&error).status, StatusCode::CONFLICT);
}

#[test]
fn partial_put_can_require_a_matching_strong_if_match() {
    let snapshot = partial_put_snapshot(DavWritePrecondition::RequireStrongIfMatch);
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 0-0/2"));

    for raw in [None, Some("*"), Some("W/\"v1\"")] {
        match raw {
            Some(raw) => {
                headers.insert(IF_MATCH, HeaderValue::from_str(raw).expect("If-Match"));
            }
            None => {
                headers.remove(IF_MATCH);
            }
        }
        let error = plan_put_request(&snapshot, &headers, existing(Some("v1")))
            .expect_err("strong If-Match is required");
        assert!(matches!(error, DavPutPlanError::PreconditionRequired));
        assert_eq!(
            put_plan_error_response(&error).status,
            StatusCode::PRECONDITION_REQUIRED
        );
    }

    headers.insert(IF_MATCH, HeaderValue::from_static("\"other\""));
    assert!(matches!(
        plan_put_request(&snapshot, &headers, existing(Some("v1"))),
        Err(DavPutPlanError::PreconditionFailed(_))
    ));

    headers.insert(IF_MATCH, HeaderValue::from_static("\"v1\""));
    assert!(plan_put_request(&snapshot, &headers, existing(Some("v1"))).is_ok());

    headers.insert(IF_MATCH, HeaderValue::from_static("bare-etag"));
    assert!(matches!(
        plan_put_request(&snapshot, &headers, existing(Some("v1"))),
        Err(DavPutPlanError::Protocol(_))
    ));
}

#[test]
fn private_update_range_is_separate_and_rejects_ambiguous_headers() {
    let snapshot = put_snapshot(DavWriteCapabilities {
        private_update_range: DavPrivateUpdateRangeCapability::XUpdateRange {
            precondition: DavWritePrecondition::Optional,
        },
        ..DavWriteCapabilities::default()
    });
    let mut headers = HeaderMap::new();
    headers.insert("X-Update-Range", HeaderValue::from_static("bytes=0-3"));
    let plan =
        plan_put_request(&snapshot, &headers, existing(Some("v1"))).expect("private range plan");
    assert!(matches!(
        plan.write,
        DavPutWritePlan::PrivateUpdateRange { ref value } if value == "bytes=0-3"
    ));

    headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 0-3/10"));
    assert!(matches!(
        plan_put_request(&snapshot, &headers, existing(Some("v1"))),
        Err(DavPutPlanError::Protocol(_))
    ));

    for value in [
        HeaderValue::from_static("  "),
        HeaderValue::from_bytes(b"\xff").unwrap(),
    ] {
        let mut invalid = HeaderMap::new();
        invalid.insert("X-Update-Range", value);
        assert!(matches!(
            plan_put_request(&snapshot, &invalid, existing(Some("v1"))),
            Err(DavPutPlanError::Protocol(_))
        ));
    }

    let mut repeated = HeaderMap::new();
    repeated.append("X-Update-Range", HeaderValue::from_static("first"));
    repeated.append("X-Update-Range", HeaderValue::from_static("second"));
    assert!(matches!(
        plan_put_request(&snapshot, &repeated, existing(Some("v1"))),
        Err(DavPutPlanError::Protocol(_))
    ));
}

#[test]
fn put_plan_enforces_etag_preconditions_and_collection_target() {
    let snapshot = ordinary_put_snapshot();
    let mut headers = HeaderMap::new();
    headers.insert(IF_MATCH, HeaderValue::from_static("*"));
    let error = plan_put_request(&snapshot, &headers, DavPutResourceState::Missing)
        .expect_err("If-Match star requires an existing resource");
    assert!(matches!(error, DavPutPlanError::PreconditionFailed(_)));

    let mut headers = HeaderMap::new();
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
    let error = plan_put_request(
        &snapshot,
        &headers,
        DavPutResourceState::File {
            etag: Some("a"),
            last_modified: Some(std::time::SystemTime::UNIX_EPOCH),
        },
    )
    .expect_err("If-None-Match star rejects an existing resource");
    assert!(matches!(error, DavPutPlanError::PreconditionFailed(_)));

    let mut collection_headers = HeaderMap::new();
    collection_headers.insert(CONTENT_RANGE, HeaderValue::from_static("invalid"));
    assert!(matches!(
        plan_put_request(
            &snapshot,
            &collection_headers,
            DavPutResourceState::Collection
        ),
        Err(DavPutPlanError::CollectionTarget)
    ));
    let response = put_plan_error_response(&DavPutPlanError::CollectionTarget);
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
}

#[test]
fn put_success_response_selects_created_or_no_content() {
    let snapshot = ordinary_put_snapshot();
    let path = DavPath::new("/space file.txt").expect("path");
    let created_plan = plan_put_request(&snapshot, &HeaderMap::new(), DavPutResourceState::Missing)
        .expect("created plan");
    let created = put_success_response(&created_plan, "/webdav", &path).expect("created response");
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(
        created.headers.get(CONTENT_LOCATION).unwrap(),
        "/webdav/space%20file.txt"
    );

    let replaced_plan =
        plan_put_request(&snapshot, &HeaderMap::new(), existing(None)).expect("replace plan");
    let replaced =
        put_success_response(&replaced_plan, "/webdav", &path).expect("replace response");
    assert_eq!(replaced.status, StatusCode::NO_CONTENT);
    assert!(replaced.headers.get(CONTENT_LOCATION).is_none());
}

#[test]
fn put_plan_applies_if_unmodified_since_and_retains_validators_on_412() {
    let snapshot = ordinary_put_snapshot();
    let mut headers = HeaderMap::new();
    headers.insert(
        IF_UNMODIFIED_SINCE,
        HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
    );
    let error = plan_put_request(
        &snapshot,
        &headers,
        DavPutResourceState::File {
            etag: Some("v1"),
            last_modified: Some(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
            ),
        },
    )
    .expect_err("stale PUT should fail");
    let response = put_plan_error_response(&error);
    assert_eq!(response.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(response.headers.get(ETAG).expect("ETag"), "\"v1\"");
    assert_eq!(
        response
            .headers
            .get("Last-Modified")
            .expect("Last-Modified"),
        "Thu, 01 Jan 1970 00:00:01 GMT"
    );
}

#[test]
fn put_plan_preserves_protocol_and_representation_error_boundaries() {
    let snapshot = ordinary_put_snapshot();
    let mut malformed = HeaderMap::new();
    malformed.insert(IF_MATCH, HeaderValue::from_static("bare-etag"));
    let protocol_error = plan_put_request(&snapshot, &malformed, DavPutResourceState::Missing)
        .expect_err("malformed If-Match should fail");
    assert!(matches!(
        &protocol_error,
        DavPutPlanError::Protocol(error)
            if error.kind() == DavProtocolErrorKind::BadRequest
    ));
    let response = put_plan_error_response(&protocol_error);
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");

    let invalid_state = DavPutResourceState::File {
        etag: Some("invalid\netag"),
        last_modified: None,
    };
    let plan = plan_put_request(&snapshot, &HeaderMap::new(), invalid_state)
        .expect("unconditional PUT should ignore an unrenderable ETag");
    assert!(plan.resource_existed);

    let mut conditional = HeaderMap::new();
    conditional.insert(IF_MATCH, HeaderValue::from_static("\"current\""));
    let representation_error = plan_put_request(&snapshot, &conditional, invalid_state)
        .expect_err("If-Match requires a representable product ETag");
    assert!(matches!(
        representation_error,
        DavPutPlanError::InvalidRepresentation
    ));
    let response = put_plan_error_response(&representation_error);
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
}

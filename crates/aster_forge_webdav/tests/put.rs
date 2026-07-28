use aster_forge_webdav::{
    DavPath, DavProtocolErrorKind, DavPutPlanError, DavPutResourceState, plan_put_request,
    put_plan_error_response, put_success_response,
};
use http::header::{
    CONTENT_LENGTH, CONTENT_LOCATION, ETAG, IF_MATCH, IF_NONE_MATCH, IF_UNMODIFIED_SINCE,
};
use http::{HeaderMap, HeaderValue, StatusCode};

#[test]
fn put_plan_selects_create_modes_and_expected_length_precedence() {
    let mut headers = HeaderMap::new();
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("4"));
    headers.insert("X-Expected-Entity-Length", HeaderValue::from_static("8"));
    let plan = plan_put_request(&headers, DavPutResourceState::Missing).expect("PUT plan");
    assert!(!plan.resource_existed);
    assert!(plan.create);
    assert!(plan.create_new);
    assert_eq!(plan.content_length_hint, Some(8));

    headers.insert(
        "X-Expected-Entity-Length",
        HeaderValue::from_static("invalid"),
    );
    let plan = plan_put_request(&headers, DavPutResourceState::Missing).expect("fallback plan");
    assert_eq!(plan.content_length_hint, Some(4));
}

#[test]
fn put_plan_enforces_etag_preconditions_and_collection_target() {
    let mut headers = HeaderMap::new();
    headers.insert(IF_MATCH, HeaderValue::from_static("*"));
    let error = plan_put_request(&headers, DavPutResourceState::Missing)
        .expect_err("If-Match star requires an existing resource");
    assert!(matches!(error, DavPutPlanError::PreconditionFailed(_)));

    let mut headers = HeaderMap::new();
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
    let error = plan_put_request(
        &headers,
        DavPutResourceState::File {
            etag: Some("a"),
            last_modified: Some(std::time::SystemTime::UNIX_EPOCH),
        },
    )
    .expect_err("If-None-Match star rejects an existing resource");
    assert!(matches!(error, DavPutPlanError::PreconditionFailed(_)));

    assert!(matches!(
        plan_put_request(&HeaderMap::new(), DavPutResourceState::Collection),
        Err(DavPutPlanError::CollectionTarget)
    ));
    let response = put_plan_error_response(&DavPutPlanError::CollectionTarget);
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
}

#[test]
fn put_success_response_selects_created_or_no_content() {
    let path = DavPath::new("/space file.txt").expect("path");
    let created_plan =
        plan_put_request(&HeaderMap::new(), DavPutResourceState::Missing).expect("created plan");
    let created = put_success_response(&created_plan, "/webdav", &path).expect("created response");
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(
        created.headers.get(CONTENT_LOCATION).unwrap(),
        "/webdav/space%20file.txt"
    );

    let replaced_plan = plan_put_request(
        &HeaderMap::new(),
        DavPutResourceState::File {
            etag: None,
            last_modified: None,
        },
    )
    .expect("replace plan");
    let replaced =
        put_success_response(&replaced_plan, "/webdav", &path).expect("replace response");
    assert_eq!(replaced.status, StatusCode::NO_CONTENT);
    assert!(replaced.headers.get(CONTENT_LOCATION).is_none());
}

#[test]
fn put_plan_applies_if_unmodified_since_and_retains_validators_on_412() {
    let mut headers = HeaderMap::new();
    headers.insert(
        IF_UNMODIFIED_SINCE,
        HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
    );
    let error = plan_put_request(
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
    let mut malformed = HeaderMap::new();
    malformed.insert(IF_MATCH, HeaderValue::from_static("bare-etag"));
    let protocol_error = plan_put_request(&malformed, DavPutResourceState::Missing)
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
    let plan = plan_put_request(&HeaderMap::new(), invalid_state)
        .expect("unconditional PUT should ignore an unrenderable ETag");
    assert!(plan.resource_existed);

    let mut conditional = HeaderMap::new();
    conditional.insert(IF_MATCH, HeaderValue::from_static("\"current\""));
    let representation_error = plan_put_request(&conditional, invalid_state)
        .expect_err("If-Match requires a representable product ETag");
    assert!(matches!(
        representation_error,
        DavPutPlanError::InvalidRepresentation
    ));
    let response = put_plan_error_response(&representation_error);
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
}

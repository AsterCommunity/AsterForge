use aster_forge_webdav::{
    DavPath, DavProtocolErrorKind, DavPutPlanError, DavPutResourceState, plan_put_request,
    put_plan_error_response, put_success_response,
};
use http::header::{CONTENT_LENGTH, CONTENT_LOCATION, IF_MATCH, IF_NONE_MATCH};
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
    assert!(matches!(
        error,
        DavPutPlanError::Protocol(error)
            if error.kind() == DavProtocolErrorKind::PreconditionFailed
    ));

    let mut headers = HeaderMap::new();
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
    let error = plan_put_request(&headers, DavPutResourceState::File { etag: Some("a") })
        .expect_err("If-None-Match star rejects an existing resource");
    assert!(matches!(error, DavPutPlanError::Protocol(_)));

    assert_eq!(
        plan_put_request(&HeaderMap::new(), DavPutResourceState::Collection),
        Err(DavPutPlanError::CollectionTarget)
    );
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

    let replaced_plan =
        plan_put_request(&HeaderMap::new(), DavPutResourceState::File { etag: None })
            .expect("replace plan");
    let replaced =
        put_success_response(&replaced_plan, "/webdav", &path).expect("replace response");
    assert_eq!(replaced.status, StatusCode::NO_CONTENT);
    assert!(replaced.headers.get(CONTENT_LOCATION).is_none());
}

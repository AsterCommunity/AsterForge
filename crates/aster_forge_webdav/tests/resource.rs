use aster_forge_webdav::{
    DavCopyMoveMethod, DavMutationFailure, DavMutationPlanError, DavPath, DavResourceKind,
    DavResponseBody, Depth, collection_created_response, delete_success_response,
    is_descendant_path, mutation_multistatus_response, mutation_plan_error_response,
    mutation_success_response, plan_copy_move_request, resource_identity_path, same_resource_path,
    validate_collection_create_target, validate_delete_target,
};
use http::StatusCode;

#[test]
fn resource_identity_and_descendant_rules_use_path_boundaries() {
    assert_eq!(resource_identity_path("/"), "/");
    assert_eq!(resource_identity_path("/docs///"), "/docs");
    assert!(same_resource_path("/docs", "/docs/"));
    assert!(!same_resource_path("/docs", "/docs-2"));
    assert!(is_descendant_path("/docs/", "/docs/sub/file.txt"));
    assert!(!is_descendant_path("/docs", "/docs-2/file.txt"));
    assert!(!is_descendant_path("/docs", "/docs"));
    assert!(!is_descendant_path("/", "/docs"));
}

#[test]
fn mutation_success_selects_created_or_no_content_with_no_store() {
    let created = mutation_success_response(false);
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.headers.get("Cache-Control").unwrap(), "no-store");

    let replaced = mutation_success_response(true);
    assert_eq!(replaced.status, StatusCode::NO_CONTENT);
    assert_eq!(replaced.headers.get("Cache-Control").unwrap(), "no-store");
}

#[test]
fn partial_failures_compose_locked_dav_error_and_plain_status_items() {
    let response = mutation_multistatus_response(
        "/webdav",
        &[
            DavMutationFailure::locked(
                DavPath::new("/locked.txt").expect("path"),
                DavPath::new("/parent/").expect("lock path"),
            ),
            DavMutationFailure::status(
                DavPath::new("/missing.txt").expect("path"),
                StatusCode::NOT_FOUND.as_u16(),
            ),
        ],
    )
    .expect("multistatus response");
    assert_eq!(response.status, StatusCode::MULTI_STATUS);
    assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
    let DavResponseBody::Bytes(body) = response.body else {
        panic!("multistatus should have an XML body");
    };
    let xml = String::from_utf8(body.to_vec()).expect("UTF-8 XML");
    assert!(xml.contains("lock-token-submitted"), "{xml}");
    assert!(xml.contains("/webdav/parent/"), "{xml}");
    assert!(xml.contains("HTTP/1.1 404 Not Found"), "{xml}");
}

#[test]
fn resource_shape_planner_enforces_depth_path_and_overwrite_rules() {
    assert_eq!(
        validate_delete_target(DavResourceKind::Collection, Depth::Zero),
        Err(DavMutationPlanError::BadRequest)
    );
    validate_delete_target(DavResourceKind::File, Depth::Zero).expect("file DELETE ignores Depth");

    let cases = [
        (
            DavCopyMoveMethod::Move,
            Depth::Zero,
            "/docs",
            "/archive",
            true,
            DavMutationPlanError::BadRequest,
        ),
        (
            DavCopyMoveMethod::Copy,
            Depth::One,
            "/docs",
            "/archive",
            true,
            DavMutationPlanError::BadRequest,
        ),
        (
            DavCopyMoveMethod::Copy,
            Depth::Infinity,
            "/docs",
            "/docs/sub",
            true,
            DavMutationPlanError::Forbidden,
        ),
        (
            DavCopyMoveMethod::Copy,
            Depth::Infinity,
            "/docs",
            "/archive",
            false,
            DavMutationPlanError::PreconditionFailed,
        ),
    ];
    for (method, depth, source, destination, overwrite, expected) in cases {
        assert_eq!(
            plan_copy_move_request(
                method,
                depth,
                DavResourceKind::Collection,
                Some(DavResourceKind::Collection),
                source,
                destination,
                overwrite,
            ),
            Err(expected)
        );
        assert_eq!(
            mutation_plan_error_response(expected)
                .headers
                .get("Cache-Control")
                .unwrap(),
            "no-store"
        );
    }

    let shallow = plan_copy_move_request(
        DavCopyMoveMethod::Copy,
        Depth::Zero,
        DavResourceKind::Collection,
        None,
        "/docs",
        "/archive",
        true,
    )
    .expect("shallow copy");
    assert!(!shallow.recursive_collection);
    assert!(!shallow.destination_deep);
}

#[test]
fn mkcol_and_delete_success_responses_are_protocol_owned() {
    assert_eq!(
        validate_collection_create_target("/"),
        Err(DavMutationPlanError::MethodNotAllowed)
    );
    validate_collection_create_target("/space folder/").expect("non-root MKCOL target");
    assert_eq!(
        mutation_plan_error_response(DavMutationPlanError::Conflict).status,
        StatusCode::CONFLICT
    );
    let path = DavPath::new("/space folder/").expect("path");
    let created = collection_created_response("/webdav", &path).expect("MKCOL response");
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(
        created.headers.get("Content-Location").unwrap(),
        "/webdav/space%20folder/"
    );
    assert_eq!(delete_success_response().status, StatusCode::NO_CONTENT);
}

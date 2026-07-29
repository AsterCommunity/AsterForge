use aster_forge_webdav::{
    DavBodyPolicy, DavCapabilityDeclaration, DavCapabilitySnapshot, DavConditionalResource,
    DavMethod, DavMethodSet, DavPatchBodyPolicy, DavPatchCapability, DavPatchFormat,
    DavPatchPlanError, DavResourceState, DavWriteCapabilities, DavWritePrecondition,
    patch_plan_error_response, plan_capabilities, plan_patch_request,
};
use http::header::{CONTENT_TYPE, ETAG, IF_MATCH};
use http::{HeaderMap, HeaderValue, StatusCode};

static PATCH_FORMATS: [DavPatchFormat; 3] = [
    DavPatchFormat {
        media_type: "application/merge-patch+json",
        body_policy: DavPatchBodyPolicy::Bounded { maximum: 4096 },
        precondition: DavWritePrecondition::RequireStrongIfMatch,
    },
    DavPatchFormat {
        media_type: "application/example-patch; version=1",
        body_policy: DavPatchBodyPolicy::Stream,
        precondition: DavWritePrecondition::Optional,
    },
    DavPatchFormat {
        media_type: "application/empty-patch",
        body_policy: DavPatchBodyPolicy::Bounded { maximum: 0 },
        precondition: DavWritePrecondition::Optional,
    },
];

fn patch_snapshot() -> DavCapabilitySnapshot {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Patch]),
    );
    declaration.writes = DavWriteCapabilities {
        patch: DavPatchCapability::Formats(&PATCH_FORMATS),
        ..DavWriteCapabilities::default()
    };
    plan_capabilities(declaration).expect("valid PATCH snapshot")
}

fn mapped(etag: Option<&str>) -> DavConditionalResource<'_> {
    DavConditionalResource {
        exists: true,
        etag,
        last_modified: None,
    }
}

#[test]
fn patch_requires_capability_before_media_type_dispatch() {
    let snapshot = plan_capabilities(DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options]),
    ))
    .expect("snapshot without PATCH");
    let error = plan_patch_request(&snapshot, &HeaderMap::new(), mapped(None))
        .expect_err("PATCH method must be enabled");
    assert!(matches!(error, DavPatchPlanError::MethodNotAllowed));
    let response = patch_plan_error_response(&error, &snapshot);
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers.get("Allow").unwrap(), "OPTIONS");
    assert!(response.headers.get("Accept-Patch").is_none());
}

#[test]
fn patch_rejects_missing_malformed_repeated_and_unsupported_content_type() {
    let snapshot = patch_snapshot();
    let mut cases = vec![HeaderMap::new()];

    for value in [
        HeaderValue::from_static("not a media type"),
        HeaderValue::from_static("application/json"),
        // Complete MIME equality is intentional: an undeclared charset is a different format.
        HeaderValue::from_static("application/merge-patch+json; charset=utf-8"),
        HeaderValue::from_bytes(b"application/\xff").expect("opaque header bytes"),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, value);
        cases.push(headers);
    }

    let mut repeated = HeaderMap::new();
    repeated.append(
        CONTENT_TYPE,
        HeaderValue::from_static("application/merge-patch+json"),
    );
    repeated.append(
        CONTENT_TYPE,
        HeaderValue::from_static("application/example-patch; version=1"),
    );
    cases.push(repeated);

    for headers in cases {
        let error = plan_patch_request(&snapshot, &headers, mapped(Some("v1")))
            .expect_err("unsupported patch document must fail");
        assert!(matches!(error, DavPatchPlanError::UnsupportedMediaType));
        let response = patch_plan_error_response(&error, &snapshot);
        assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(
            response.headers.get("Accept-Patch").unwrap(),
            snapshot.accept_patch_header().unwrap()
        );
        assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");
    }
}

#[test]
fn patch_dispatches_structured_media_types_and_exact_parameters() {
    let snapshot = patch_snapshot();

    let mut bounded = HeaderMap::new();
    bounded.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("APPLICATION/MERGE-PATCH+JSON"),
    );
    bounded.insert(IF_MATCH, HeaderValue::from_static("\"v1\""));
    let plan = plan_patch_request(&snapshot, &bounded, mapped(Some("v1")))
        .expect("case-insensitive MIME names should match");
    assert_eq!(plan.format, &PATCH_FORMATS[0]);
    assert_eq!(plan.body_policy, DavBodyPolicy::Bounded { maximum: 4096 });
    assert!(plan.resource_existed);

    let mut streamed = HeaderMap::new();
    streamed.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/example-patch; VERSION=1"),
    );
    let plan = plan_patch_request(&snapshot, &streamed, DavConditionalResource::missing())
        .expect("declared MIME parameter should dispatch");
    assert_eq!(plan.format, &PATCH_FORMATS[1]);
    assert_eq!(plan.body_policy, DavBodyPolicy::Stream);
    assert!(!plan.resource_existed);

    let mut zero = HeaderMap::new();
    zero.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/empty-patch"),
    );
    assert_eq!(
        plan_patch_request(&snapshot, &zero, mapped(None))
            .expect("zero-limit format selection")
            .body_policy,
        DavBodyPolicy::Bounded { maximum: 0 }
    );
}

#[test]
fn patch_format_can_require_a_matching_strong_if_match() {
    let snapshot = patch_snapshot();
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/merge-patch+json"),
    );

    for raw in [None, Some("*"), Some("W/\"v1\"")] {
        match raw {
            Some(raw) => {
                headers.insert(IF_MATCH, HeaderValue::from_str(raw).expect("If-Match"));
            }
            None => {
                headers.remove(IF_MATCH);
            }
        }
        let error = plan_patch_request(&snapshot, &headers, mapped(Some("v1")))
            .expect_err("strong If-Match is required");
        assert!(matches!(error, DavPatchPlanError::PreconditionRequired));
        assert_eq!(
            patch_plan_error_response(&error, &snapshot).status,
            StatusCode::PRECONDITION_REQUIRED
        );
    }

    headers.insert(IF_MATCH, HeaderValue::from_static("\"other\""));
    let error = plan_patch_request(&snapshot, &headers, mapped(Some("v1")))
        .expect_err("mismatching strong tag must fail");
    assert!(matches!(error, DavPatchPlanError::PreconditionFailed(_)));
    let response = patch_plan_error_response(&error, &snapshot);
    assert_eq!(response.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(response.headers.get(ETAG).unwrap(), "\"v1\"");

    headers.insert(IF_MATCH, HeaderValue::from_static("\"v1\""));
    assert!(plan_patch_request(&snapshot, &headers, mapped(Some("v1"))).is_ok());
}

#[test]
fn patch_preserves_protocol_and_representation_error_boundaries() {
    let snapshot = patch_snapshot();
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/merge-patch+json"),
    );
    headers.insert(IF_MATCH, HeaderValue::from_static("bare-etag"));
    let malformed = plan_patch_request(&snapshot, &headers, mapped(Some("v1")))
        .expect_err("malformed If-Match");
    assert!(matches!(malformed, DavPatchPlanError::Protocol(_)));
    assert_eq!(
        patch_plan_error_response(&malformed, &snapshot).status,
        StatusCode::BAD_REQUEST
    );

    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/example-patch; version=1"),
    );
    let optional_malformed = plan_patch_request(&snapshot, &headers, mapped(Some("v1")))
        .expect_err("optional format still validates a supplied If-Match");
    assert!(matches!(optional_malformed, DavPatchPlanError::Protocol(_)));

    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/merge-patch+json"),
    );
    headers.insert(IF_MATCH, HeaderValue::from_static("\"v1\""));
    let invalid = plan_patch_request(&snapshot, &headers, mapped(Some("invalid\netag")))
        .expect_err("unrepresentable current ETag");
    assert!(matches!(invalid, DavPatchPlanError::InvalidRepresentation));
    assert_eq!(
        patch_plan_error_response(&invalid, &snapshot).status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

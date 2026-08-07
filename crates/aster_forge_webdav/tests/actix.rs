#![cfg(feature = "actix")]

use std::pin::Pin;

use actix_web::http::{Method, StatusCode};
use actix_web::{FromRequest, web};
use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavBodyError, DavCancellationToken, DavCapabilityContext,
    DavCapabilityDeclaration, DavCapabilityProvider, DavCapabilityTarget,
    DavCompatibilityCapabilities, DavComplianceClasses, DavConditionalOutcome,
    DavConditionalResource, DavExtensionPackage, DavExtensionSet, DavLockingCapability, DavMethod,
    DavMethodSet, DavMultiStatusItem, DavMultiStatusLimits, DavMultiStatusSourceError,
    DavNonDavProfile, DavPath, DavRequestedProperty, DavResourceState, DavResponse,
    DavResponseBody, DavSearchCapabilities, DavVersionProperty, DavVersionReportItem,
    DavVersionTreeRequest, DavWriteCapabilities, Depth, multistatus_stream_response,
    multistatus_stream_response_with_cancellation, plan_capabilities, version_tree_response,
};
use bytes::Bytes;
use futures::StreamExt;

async fn payload_from_bytes(bytes: Bytes) -> web::Payload {
    let (request, mut payload) = actix_web::test::TestRequest::default()
        .set_payload(bytes)
        .to_http_parts();
    web::Payload::from_request(&request, &mut payload)
        .await
        .expect("test payload should extract")
}

async fn payload_from_chunks(chunks: &[&'static [u8]]) -> web::Payload {
    let stream = futures::stream::iter(
        chunks
            .iter()
            .map(|chunk| Ok(Bytes::from_static(chunk)))
            .collect::<Vec<Result<Bytes, actix_web::error::PayloadError>>>(),
    );
    let stream: Pin<
        Box<dyn futures::Stream<Item = Result<Bytes, actix_web::error::PayloadError>>>,
    > = Box::pin(stream);
    let mut payload = actix_web::dev::Payload::from(stream);
    let request = actix_web::test::TestRequest::default().to_http_request();
    web::Payload::from_request(&request, &mut payload)
        .await
        .expect("chunked test payload should extract")
}

#[actix_web::test]
async fn empty_body_policy_accepts_empty_and_rejects_the_first_nonempty_chunk() {
    let mut empty = payload_from_bytes(Bytes::new()).await;
    aster_forge_webdav::actix::ensure_empty_body(&mut empty)
        .await
        .expect("empty body should be accepted");

    let mut nonempty = payload_from_bytes(Bytes::from(vec![b'x'; 2 * 1024 * 1024])).await;
    assert_eq!(
        aster_forge_webdav::actix::ensure_empty_body(&mut nonempty).await,
        Err(DavBodyError::BodyNotAllowed)
    );
}

#[actix_web::test]
async fn bounded_body_accepts_the_exact_limit_and_rejects_one_byte_over() {
    let mut exact = payload_from_bytes(Bytes::from_static(b"1234")).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_body(&mut exact, 4)
            .await
            .expect("exact body limit should be accepted"),
        b"1234"
    );

    let mut over = payload_from_bytes(Bytes::from_static(b"12345")).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_body(&mut over, 4).await,
        Err(DavBodyError::BodyTooLarge)
    );

    let mut cumulative = payload_from_chunks(&[b"12", b"34", b"5"]).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_body(&mut cumulative, 4).await,
        Err(DavBodyError::BodyTooLarge)
    );

    let mut zero_empty = payload_from_bytes(Bytes::new()).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_body(&mut zero_empty, 0).await,
        Ok(Vec::new())
    );
    let mut zero_nonempty = payload_from_bytes(Bytes::from_static(b"x")).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_body(&mut zero_nonempty, 0).await,
        Err(DavBodyError::BodyTooLarge)
    );
}

#[actix_web::test]
async fn method_body_preparation_collects_xml_rejects_empty_policy_and_preserves_streams() {
    let snapshot = body_snapshot();
    for (method, body) in [
        (DavMethod::Propfind, b"<D:propfind/>".as_slice()),
        (
            DavMethod::VersionControl,
            b"<D:version-control/>".as_slice(),
        ),
    ] {
        let mut xml = payload_from_bytes(Bytes::copy_from_slice(body)).await;
        let policy = snapshot
            .body_policy(method, 64)
            .expect("allowed method")
            .expect("standard method body policy");
        let prepared = aster_forge_webdav::actix::prepare_request_body(policy, &mut xml)
            .await
            .expect("bounded XML body");
        assert_eq!(prepared.xml(), body);
        assert!(prepared.bytes().is_empty());
    }

    let mut forbidden = payload_from_bytes(Bytes::from_static(b"x")).await;
    assert!(matches!(
        aster_forge_webdav::actix::prepare_request_body(
            snapshot
                .body_policy(DavMethod::Options, 64)
                .expect("allowed method")
                .expect("OPTIONS body policy"),
            &mut forbidden,
        )
        .await,
        Err(DavBodyError::BodyNotAllowed)
    ));

    let mut stream = payload_from_bytes(Bytes::from_static(b"payload")).await;
    let prepared = aster_forge_webdav::actix::prepare_request_body(
        snapshot
            .body_policy(DavMethod::Put, 64)
            .expect("allowed method")
            .expect("PUT body policy"),
        &mut stream,
    )
    .await
    .expect("PUT stream policy");
    assert!(prepared.xml().is_empty());
    assert!(prepared.bytes().is_empty());
    assert_eq!(
        stream.next().await.expect("stream chunk").expect("chunk"),
        Bytes::from_static(b"payload")
    );

    let mut bounded = payload_from_bytes(Bytes::from_static(b"patch")).await;
    let prepared = aster_forge_webdav::actix::prepare_request_body(
        aster_forge_webdav::DavBodyPolicy::Bounded { maximum: 5 },
        &mut bounded,
    )
    .await
    .expect("bounded patch body");
    assert_eq!(prepared.bytes(), b"patch");
    assert!(prepared.xml().is_empty());

    let mut oversized = payload_from_bytes(Bytes::from_static(b"patch!")).await;
    assert!(matches!(
        aster_forge_webdav::actix::prepare_request_body(
            aster_forge_webdav::DavBodyPolicy::Bounded { maximum: 5 },
            &mut oversized,
        )
        .await,
        Err(DavBodyError::BodyTooLarge)
    ));
}

#[actix_web::test]
async fn actix_adapter_streams_multistatus_and_propagates_typed_stream_failures() {
    let response = multistatus_stream_response(
        futures::stream::iter([Ok(DavMultiStatusItem::status("/dav/a", 404))]),
        DavMultiStatusLimits::new(4096, 2, 2, 8),
    )
    .expect("stream response");
    let response = aster_forge_webdav::actix::into_response(response);
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    assert_eq!(
        response
            .headers()
            .get("Content-Type")
            .expect("content type"),
        "application/xml; charset=utf-8"
    );
    let body = actix_web::body::to_bytes(response.into_body())
        .await
        .expect("streamed body");
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 XML");
    assert!(body.contains("/dav/a"), "{body}");
    assert!(body.ends_with("</D:multistatus>"), "{body}");

    let response = multistatus_stream_response(
        futures::stream::iter([Err(DavMultiStatusSourceError::Cancelled)]),
        DavMultiStatusLimits::default(),
    )
    .expect("failure response shell");
    let response = aster_forge_webdav::actix::into_response(response);
    let error = actix_web::body::to_bytes(response.into_body())
        .await
        .expect_err("typed stream failure should terminate the Actix body");
    assert_eq!(error.to_string(), "WebDAV response stream failed");

    let mut response = DavResponse::empty(http::StatusCode::OK);
    response.body = DavResponseBody::Stream(Box::pin(futures::stream::iter([Err(
        DavBackendError::new(DavBackendErrorKind::Forbidden),
    )])));
    let response = aster_forge_webdav::actix::into_response(response);
    let error = actix_web::body::to_bytes(response.into_body())
        .await
        .expect_err("backend stream failure should terminate the Actix body");
    assert_eq!(error.to_string(), "WebDAV response stream failed");
}

#[actix_web::test]
async fn actix_adapter_preserves_bounded_deltav_status_headers_and_body() {
    let property = DavRequestedProperty {
        name: "version-name".to_owned(),
        namespace: Some("DAV:".to_owned()),
        prefix: Some("D".to_owned()),
    };
    let response = version_tree_response(
        &DavVersionTreeRequest {
            properties: Some(vec![property.clone()]),
            depth: Depth::Zero,
        },
        vec![DavVersionReportItem {
            href: "/versions/1".to_owned(),
            properties: vec![DavVersionProperty::text(property, "V1")],
        }],
    )
    .expect("bounded DeltaV response");
    let response = aster_forge_webdav::actix::into_response(response);
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    assert_eq!(
        response
            .headers()
            .get("Content-Type")
            .expect("content type"),
        "application/xml; charset=utf-8"
    );
    let body = actix_web::body::to_bytes(response.into_body())
        .await
        .expect("DeltaV body");
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 XML");
    assert!(body.contains("/versions/1"), "{body}");
    assert!(body.contains("V1"), "{body}");
}

#[test]
fn header_and_precondition_adapter_preserves_success_and_error_contracts() {
    let matching_none_match = actix_web::test::TestRequest::default()
        .insert_header(("If-None-Match", "\"etag-a\""))
        .to_http_request();
    assert_eq!(
        aster_forge_webdav::actix::plan_http_conditionals(
            matching_none_match.headers(),
            DavMethod::Get,
            DavConditionalResource {
                exists: true,
                etag: Some("etag-a"),
                last_modified: None,
            },
        )
        .expect("safe matching If-None-Match should be a valid precondition")
        .outcome,
        DavConditionalOutcome::NotModified
    );

    let mismatching_if_match = actix_web::test::TestRequest::default()
        .insert_header(("If-Match", "\"etag-b\""))
        .to_http_request();
    assert_eq!(
        aster_forge_webdav::actix::plan_http_conditionals(
            mismatching_if_match.headers(),
            DavMethod::Put,
            DavConditionalResource {
                exists: true,
                etag: Some("etag-a"),
                last_modified: None,
            },
        )
        .expect("valid mismatching If-Match should produce a plan")
        .outcome,
        DavConditionalOutcome::PreconditionFailed
    );

    let malformed_if_match = actix_web::test::TestRequest::default()
        .insert_header(("If-Match", "bare-etag"))
        .to_http_request();
    let response = aster_forge_webdav::actix::plan_http_conditionals(
        malformed_if_match.headers(),
        DavMethod::Put,
        DavConditionalResource {
            exists: true,
            etag: Some("etag-a"),
            last_modified: None,
        },
    )
    .expect_err("malformed If-Match should map to an Actix response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let converted = aster_forge_webdav::actix::converted_headers(malformed_if_match.headers())
        .expect("valid Actix headers should convert");
    assert_eq!(converted.get("If-Match").expect("If-Match"), "bare-etag");
}

#[test]
fn unknown_methods_still_parse_and_validate_the_request_target_first() {
    let unknown = actix_web::test::TestRequest::default()
        .method(Method::from_bytes(b"TARGET-METHOD").expect("unknown method"))
        .uri("/webdav/folder/file.txt")
        .to_http_request();
    let target = aster_forge_webdav::actix::request_target(&unknown, "/webdav")
        .expect("unknown method target should parse");
    assert_eq!(target.target.as_str(), "/folder/file.txt");
    assert!(
        aster_forge_webdav::actix::request_head(&unknown, "/webdav")
            .expect("target should parse")
            .is_none()
    );

    let outside = actix_web::test::TestRequest::default()
        .method(Method::from_bytes(b"TARGET-METHOD").expect("unknown method"))
        .uri("/outside/file.txt")
        .to_http_request();
    assert!(aster_forge_webdav::actix::request_head(&outside, "/webdav").is_err());
}

fn actix_snapshot() -> aster_forge_webdav::DavCapabilitySnapshot {
    plan_capabilities(DavCapabilityDeclaration {
        resource: DavResourceState::File,
        versioning: aster_forge_webdav::DavVersioningCapabilities::default(),
        methods: DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Get]),
        locking: DavLockingCapability::Disabled,
        extensions: aster_forge_webdav::DavExtensionSet::empty(),
        search: DavSearchCapabilities::default(),
        compatibility: DavCompatibilityCapabilities::default(),
        writes: DavWriteCapabilities::default(),
        compliance: DavComplianceClasses {
            class1: true,
            class3: false,
        },
    })
    .expect("valid Actix capability snapshot")
}

fn body_snapshot() -> aster_forge_webdav::DavCapabilitySnapshot {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[
            DavMethod::Options,
            DavMethod::Propfind,
            DavMethod::Put,
            DavMethod::Report,
            DavMethod::VersionControl,
        ]),
    );
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::VersionControl]);
    declaration.versioning = aster_forge_webdav::DavVersioningCapabilities {
        state: aster_forge_webdav::DavVersioningState::CheckedIn,
        ..aster_forge_webdav::DavVersioningCapabilities::default()
    };
    plan_capabilities(declaration).expect("body capability snapshot")
}

#[test]
fn actix_dispatch_gate_uses_snapshot_allow_for_known_and_unknown_methods() {
    let snapshot = actix_snapshot();
    let get = actix_web::test::TestRequest::get()
        .uri("/webdav/file.txt")
        .to_http_request();
    assert_eq!(
        aster_forge_webdav::actix::gate_request_method(&get, &snapshot)
            .expect("GET should be allowed"),
        DavMethod::Get
    );

    for request in [
        actix_web::test::TestRequest::put()
            .uri("/webdav/file.txt")
            .to_http_request(),
        actix_web::test::TestRequest::default()
            .method(Method::from_bytes(b"SEARCH").expect("extension method"))
            .uri("/webdav/file.txt")
            .to_http_request(),
    ] {
        let response = aster_forge_webdav::actix::gate_request_method(&request, &snapshot)
            .expect_err("method should be rejected");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response.headers().get("Allow").expect("Allow header"),
            "OPTIONS, GET, HEAD"
        );
    }
}

struct InvalidCapabilityProvider;

impl DavCapabilityProvider for InvalidCapabilityProvider {
    type Profile = DavNonDavProfile;

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        Ok(DavCapabilityDeclaration::new(
            DavResourceState::File,
            DavMethodSet::from_methods(&[DavMethod::Get]),
        ))
    }
}

struct ForbiddenCapabilityProvider;

impl DavCapabilityProvider for ForbiddenCapabilityProvider {
    type Profile = DavNonDavProfile;

    async fn capabilities(
        &self,
        _target: &DavCapabilityTarget,
        _context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError> {
        Err(DavBackendError::new(DavBackendErrorKind::Forbidden))
    }
}

#[actix_web::test]
async fn actix_capability_adapter_distinguishes_invalid_declarations_and_backend_failures() {
    let target = DavCapabilityTarget::new(DavPath::new("/file.txt").expect("path"), false);
    let context = DavCapabilityContext::default();

    let invalid = aster_forge_webdav::actix::capability_snapshot(
        &InvalidCapabilityProvider,
        &target,
        &context,
    )
    .await
    .expect_err("invalid declaration should map to a response");
    assert_eq!(invalid.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let forbidden = aster_forge_webdav::actix::capability_snapshot(
        &ForbiddenCapabilityProvider,
        &target,
        &context,
    )
    .await
    .expect_err("provider failure should map to a response");
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}

#[test]
fn actix_response_drop_propagates_multistatus_cancellation() {
    let cancellation = DavCancellationToken::new();
    let response = multistatus_stream_response_with_cancellation(
        futures::stream::pending(),
        DavMultiStatusLimits::default(),
        cancellation.clone(),
    )
    .expect("stream response");
    let response = aster_forge_webdav::actix::into_response(response);
    assert!(!cancellation.is_cancelled());

    drop(response);

    assert!(cancellation.is_cancelled());
}

#![cfg(feature = "actix")]

use actix_web::http::StatusCode;
use actix_web::{FromRequest, web};
use aster_forge_webdav::{DavBodyError, DavMethod, DavPrecondition};
use bytes::Bytes;
use futures::StreamExt;
use std::pin::Pin;

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
async fn bounded_xml_body_accepts_the_exact_limit_and_rejects_one_byte_over() {
    let mut exact = payload_from_bytes(Bytes::from_static(b"1234")).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_xml_body(&mut exact, 4)
            .await
            .expect("exact body limit should be accepted"),
        b"1234"
    );

    let mut over = payload_from_bytes(Bytes::from_static(b"12345")).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_xml_body(&mut over, 4).await,
        Err(DavBodyError::XmlTooLarge)
    );

    let mut cumulative = payload_from_chunks(&[b"12", b"34", b"5"]).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_xml_body(&mut cumulative, 4).await,
        Err(DavBodyError::XmlTooLarge)
    );

    let mut zero_empty = payload_from_bytes(Bytes::new()).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_xml_body(&mut zero_empty, 0).await,
        Ok(Vec::new())
    );
    let mut zero_nonempty = payload_from_bytes(Bytes::from_static(b"x")).await;
    assert_eq!(
        aster_forge_webdav::actix::collect_bounded_xml_body(&mut zero_nonempty, 0).await,
        Err(DavBodyError::XmlTooLarge)
    );
}

#[actix_web::test]
async fn method_body_preparation_collects_xml_rejects_empty_policy_and_preserves_streams() {
    for (method, body) in [
        (DavMethod::Propfind, b"<D:propfind/>".as_slice()),
        (
            DavMethod::VersionControl,
            b"<D:version-control/>".as_slice(),
        ),
    ] {
        let mut xml = payload_from_bytes(Bytes::copy_from_slice(body)).await;
        let prepared = aster_forge_webdav::actix::prepare_request_body(method, &mut xml, 64)
            .await
            .expect("bounded XML body");
        assert_eq!(prepared.xml(), body);
    }

    let mut forbidden = payload_from_bytes(Bytes::from_static(b"x")).await;
    assert!(matches!(
        aster_forge_webdav::actix::prepare_request_body(DavMethod::Options, &mut forbidden, 64,)
            .await,
        Err(DavBodyError::BodyNotAllowed)
    ));

    let mut stream = payload_from_bytes(Bytes::from_static(b"payload")).await;
    let prepared = aster_forge_webdav::actix::prepare_request_body(DavMethod::Put, &mut stream, 64)
        .await
        .expect("PUT stream policy");
    assert!(prepared.xml().is_empty());
    assert_eq!(
        stream.next().await.expect("stream chunk").expect("chunk"),
        Bytes::from_static(b"payload")
    );
}

#[test]
fn header_and_precondition_adapter_preserves_success_and_error_contracts() {
    let matching_none_match = actix_web::test::TestRequest::default()
        .insert_header(("If-None-Match", "\"etag-a\""))
        .to_http_request();
    assert_eq!(
        aster_forge_webdav::actix::evaluate_http_etag_preconditions(
            matching_none_match.headers(),
            true,
            Some("etag-a"),
            true,
        )
        .expect("safe matching If-None-Match should be a valid precondition"),
        DavPrecondition::NotModified
    );

    let mismatching_if_match = actix_web::test::TestRequest::default()
        .insert_header(("If-Match", "\"etag-b\""))
        .to_http_request();
    let response = aster_forge_webdav::actix::evaluate_http_etag_preconditions(
        mismatching_if_match.headers(),
        true,
        Some("etag-a"),
        false,
    )
    .expect_err("mismatching If-Match should fail");
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let converted = aster_forge_webdav::actix::converted_headers(mismatching_if_match.headers())
        .expect("valid Actix headers should convert");
    assert_eq!(converted.get("If-Match").expect("If-Match"), "\"etag-b\"");
}

#![cfg(feature = "actix")]

use actix_web::{FromRequest, web};
use aster_forge_webdav::{DavBodyError, DavMethod};
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
}

#[actix_web::test]
async fn method_body_preparation_collects_xml_rejects_empty_policy_and_preserves_streams() {
    let mut xml = payload_from_bytes(Bytes::from_static(b"<D:propfind/>")).await;
    let prepared =
        aster_forge_webdav::actix::prepare_request_body(DavMethod::Propfind, &mut xml, 64)
            .await
            .expect("PROPFIND body");
    assert_eq!(prepared.xml(), b"<D:propfind/>");

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

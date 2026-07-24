#![cfg(feature = "actix")]

use actix_web::{FromRequest, web};
use aster_forge_webdav::DavBodyError;
use bytes::Bytes;

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

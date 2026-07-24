use std::time::Duration;

use aster_forge_webdav::{
    DavLockInfo, DavLockPlan, DavLockPlanError, DavMethod, DavPath, DavProtocolErrorKind,
    DavRequestHead, DavRequestOrigin, DavResponseBody, Depth, IfHeader,
    lock_acquire_success_response, lock_conflict_response, lock_limit_response,
    lock_refresh_success_response, lock_xml_error_response, parse_if_header, plan_lock_request,
    unlock_success_response, unlock_token_mismatch_response,
};
use http::StatusCode;
use http::header::{HeaderMap, HeaderValue};

fn headers(name: &'static str, value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(name, HeaderValue::from_static(value));
    headers
}

fn request_head(depth: Depth, if_header: Option<IfHeader>) -> DavRequestHead {
    DavRequestHead {
        method: DavMethod::Lock,
        target: DavPath::new("/a.txt").expect("path"),
        origin: DavRequestOrigin {
            scheme: "https".to_owned(),
            host: "dav.example".to_owned(),
        },
        depth: Some(depth),
        overwrite: None,
        destination: None,
        if_header,
    }
}

#[test]
fn lock_plan_selects_acquire_with_parsed_body_depth_and_timeout() {
    let body = br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:shared/></D:lockscope><D:locktype><D:write/></D:locktype><D:owner>owner</D:owner></D:lockinfo>"#;
    let plan = plan_lock_request(
        &headers("Timeout", "Second-60"),
        body,
        &request_head(Depth::Infinity, None),
        "/webdav",
        Duration::from_secs(600),
    )
    .expect("acquire plan");
    assert!(matches!(
        plan,
        DavLockPlan::Acquire {
            timeout,
            shared: true,
            deep: true,
            owner: Some(_),
        } if timeout == Duration::from_secs(60)
    ));
}

#[test]
fn empty_lock_body_requires_exactly_one_scoped_refresh_token() {
    let if_header = parse_if_header(&headers(
        "If",
        "</webdav/a.txt> (<urn:uuid:a>) </webdav/b.txt> (<urn:uuid:b>)",
    ))
    .expect("If grammar")
    .expect("If header");
    let plan = plan_lock_request(
        &HeaderMap::new(),
        &[],
        &request_head(Depth::Zero, Some(if_header)),
        "/webdav",
        Duration::from_secs(600),
    )
    .expect("one path-scoped token should refresh");
    assert!(matches!(plan, DavLockPlan::Refresh { token, .. } if token == "urn:uuid:a"));

    let error = plan_lock_request(
        &HeaderMap::new(),
        &[],
        &request_head(Depth::Zero, None),
        "/webdav",
        Duration::from_secs(600),
    )
    .expect_err("missing refresh token should fail");
    assert!(matches!(
        error,
        DavLockPlanError::Protocol(error) if error.kind() == DavProtocolErrorKind::BadRequest
    ));
}

#[test]
fn lock_success_responses_own_status_xml_and_lock_token_header_contract() {
    let lock = DavLockInfo {
        token: "urn:uuid:lock".to_string(),
        path: DavPath::new("/a.txt").expect("path"),
        owner_xml: None,
        timeout_at: None,
        timeout: Some(Duration::from_secs(60)),
        shared: false,
        deep: false,
    };
    let response = lock_acquire_success_response(&lock, "/webdav", false).expect("LOCK response");
    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(
        response.headers.get("Lock-Token").unwrap(),
        "<urn:uuid:lock>"
    );
    assert_eq!(
        response.headers.get("Content-Type").unwrap(),
        "application/xml; charset=utf-8"
    );
    assert!(matches!(response.body, DavResponseBody::Bytes(_)));

    let refreshed = lock_refresh_success_response(&lock, "/webdav").expect("refresh response");
    assert_eq!(refreshed.status, StatusCode::OK);
    assert!(refreshed.headers.get("Lock-Token").is_none());

    let existing =
        lock_acquire_success_response(&lock, "/webdav", true).expect("existing resource response");
    assert_eq!(existing.status, StatusCode::OK);
    assert_eq!(
        existing.headers.get("Lock-Token").unwrap(),
        "<urn:uuid:lock>"
    );
}

#[test]
fn lock_and_unlock_failures_own_status_dav_error_and_cache_contract() {
    let path = DavPath::new("/folder/a b.txt").expect("path");
    let conflict = lock_conflict_response("/webdav", &path).expect("conflict response");
    assert_eq!(conflict.status, StatusCode::LOCKED);
    assert_eq!(conflict.headers.get("Cache-Control").unwrap(), "no-store");
    let DavResponseBody::Bytes(body) = conflict.body else {
        panic!("conflict should have XML body");
    };
    let xml = String::from_utf8(body.to_vec()).expect("UTF-8 XML");
    assert!(xml.contains("lock-token-submitted"), "{xml}");
    assert!(xml.contains("a%20b.txt"), "{xml}");

    let mismatch = unlock_token_mismatch_response().expect("mismatch response");
    assert_eq!(mismatch.status, StatusCode::CONFLICT);
    assert_eq!(mismatch.headers.get("Cache-Control").unwrap(), "no-store");
    let DavResponseBody::Bytes(body) = mismatch.body else {
        panic!("mismatch should have XML body");
    };
    let xml = String::from_utf8(body.to_vec()).expect("UTF-8 XML");
    assert!(xml.contains("lock-token-matches-request-uri"), "{xml}");
}

#[test]
fn unlock_success_and_lock_limit_are_explicitly_not_cacheable() {
    let success = unlock_success_response();
    assert_eq!(success.status, StatusCode::NO_CONTENT);
    assert_eq!(success.headers.get("Cache-Control").unwrap(), "no-store");

    let limit = lock_limit_response();
    assert_eq!(limit.status, StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(limit.headers.get("Cache-Control").unwrap(), "no-store");
    assert_eq!(
        limit.headers.get("Content-Type").unwrap(),
        "text/plain; charset=utf-8"
    );
}

#[test]
fn lock_xml_errors_are_mapped_by_the_protocol_layer() {
    let invalid = lock_xml_error_response(aster_forge_webdav::DavXmlError::InvalidGrammar)
        .expect("invalid LOCK response");
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let DavResponseBody::Bytes(body) = invalid.body else {
        panic!("invalid LOCK should have text body");
    };
    assert_eq!(body.as_ref(), b"Invalid LOCK body");

    let external = lock_xml_error_response(aster_forge_webdav::DavXmlError::ExternalEntity)
        .expect("external entity response");
    assert_eq!(external.status, StatusCode::FORBIDDEN);
    let DavResponseBody::Bytes(body) = external.body else {
        panic!("external entity should have XML body");
    };
    assert!(
        String::from_utf8(body.to_vec())
            .unwrap()
            .contains("no-external-entities")
    );
}

use aster_forge_webdav::{
    DavResourceKind, DavResponse, DavResponseBody, DavVersionTreeReportError, DavVersionXml,
    DavXmlError, validate_version_control_request, validate_version_tree_report,
    version_control_request_error_response, version_control_response,
    version_tree_non_file_response, version_tree_report_error_response, version_tree_response,
};
use http::StatusCode;

fn body_text(response: &DavResponse) -> String {
    let DavResponseBody::Bytes(body) = &response.body else {
        panic!("response should contain bytes");
    };
    String::from_utf8(body.to_vec()).expect("UTF-8 response")
}

#[test]
fn report_selector_accepts_only_dav_version_tree() {
    validate_version_tree_report(br#"<D:version-tree xmlns:D="DAV:"/>"#).expect("valid report");
    validate_version_tree_report(br#"<X:version-tree xmlns:X="DAV:"/>"#)
        .expect("prefix is lexical only");
    validate_version_tree_report(
        br#"<D:version-tree xmlns:D="DAV:" xmlns:X="urn:x">
              <D:future><D:prop><D:not-active/></D:prop></D:future>
              <D:prop><D:getetag><D:ignored>value</D:ignored></D:getetag></D:prop>
              <X:future/>
            </D:version-tree>"#,
    )
    .expect("unknown report extensions should be ignored");

    assert_eq!(
        validate_version_tree_report(br#"<D:expand-property xmlns:D="DAV:"/>"#),
        Err(DavVersionTreeReportError::Unsupported {
            name: "expand-property".to_owned(),
        })
    );
    assert_eq!(
        validate_version_tree_report(br#"<X:version-tree xmlns:X="urn:extension"/>"#),
        Err(DavVersionTreeReportError::Unsupported {
            name: "version-tree".to_owned(),
        })
    );
    for body in [
        br#"<D:version-tree xmlns:D="DAV:"><D:prop/><D:prop/></D:version-tree>"#.as_slice(),
        br#"<D:version-tree xmlns:D="DAV:">text</D:version-tree>"#,
        br#"<D:version-tree xmlns:D="DAV:"><D:prop><D:getetag>value</D:getetag></D:prop></D:version-tree>"#,
    ] {
        assert_eq!(
            validate_version_tree_report(body),
            Err(DavVersionTreeReportError::Xml(
                DavXmlError::InvalidGrammar
            ))
        );
    }
}

#[test]
fn version_tree_known_prop_order_is_irrelevant_across_extensions() {
    for body in [
        br#"<D:version-tree xmlns:D="DAV:" xmlns:X="urn:x">
              <D:prop><D:getetag/></D:prop><X:future/><D:future/>
            </D:version-tree>"#
            .as_slice(),
        br#"<D:version-tree xmlns:D="DAV:" xmlns:X="urn:x">
              <X:future/><D:future/><D:prop><D:getetag/></D:prop>
            </D:version-tree>"#,
        br#"<D:version-tree xmlns:D="DAV:" xmlns:X="urn:x">
              <X:before/><D:prop><D:getetag/></D:prop><X:after/>
            </D:version-tree>"#,
    ] {
        validate_version_tree_report(body)
            .expect("version-tree extension ordering should not affect the known prop control");
    }
}

#[test]
fn version_control_accepts_absent_or_any_safe_dav_body() {
    validate_version_control_request(b"").expect("absent body is valid");
    validate_version_control_request(br#"<D:version-control xmlns:D="DAV:"/>"#)
        .expect("empty DAV body is valid");
    validate_version_control_request(
        br#"<V:version-control xmlns:V="DAV:" xmlns:X="urn:x" X:flag="1">
              text<X:future><V:nested/></X:future><![CDATA[more]]>
            </V:version-control>"#,
    )
    .expect("RFC 3253 declares version-control as ANY");
}

#[test]
fn version_control_rejects_wrong_root_and_unsafe_or_malformed_xml() {
    assert_eq!(
        validate_version_control_request(br#"<X:version-control xmlns:X="urn:x"/>"#),
        Err(DavXmlError::InvalidGrammar)
    );
    assert_eq!(
        validate_version_control_request(b"   \r\n"),
        Err(DavXmlError::Malformed)
    );
    assert_eq!(
        validate_version_control_request(
            br#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///TARGET">]><D:version-control xmlns:D="DAV:">&e;</D:version-control>"#,
        ),
        Err(DavXmlError::ExternalEntity)
    );
}

#[test]
fn report_selector_preserves_xml_failure_categories() {
    assert_eq!(
        validate_version_tree_report(b"<D:version-tree"),
        Err(DavVersionTreeReportError::Xml(DavXmlError::Malformed))
    );
    assert_eq!(
        validate_version_tree_report(
            br#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]><D:version-tree xmlns:D="DAV:"/>"#,
        ),
        Err(DavVersionTreeReportError::Xml(
            DavXmlError::ExternalEntity
        ))
    );
}

#[test]
fn report_errors_select_plain_or_dav_xml_responses() {
    let invalid =
        version_tree_report_error_response(&DavVersionTreeReportError::Xml(DavXmlError::Malformed))
            .expect("invalid response");
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.headers.get("Cache-Control").unwrap(), "no-store");
    assert_eq!(body_text(&invalid), "Invalid XML body");

    let unsupported = version_tree_report_error_response(&DavVersionTreeReportError::Unsupported {
        name: "expand-property".to_owned(),
    })
    .expect("unsupported response");
    assert_eq!(unsupported.status, StatusCode::NOT_IMPLEMENTED);
    assert!(body_text(&unsupported).contains("expand-property"));

    let external = version_tree_report_error_response(&DavVersionTreeReportError::Xml(
        DavXmlError::ExternalEntity,
    ))
    .expect("external entity response");
    assert_eq!(external.status, StatusCode::FORBIDDEN);
    assert_eq!(
        external.headers.get("Content-Type").unwrap(),
        "application/xml; charset=utf-8"
    );
    assert!(body_text(&external).contains("no-external-entities"));

    let too_large =
        version_tree_report_error_response(&DavVersionTreeReportError::Xml(DavXmlError::TooLarge))
            .expect("too large response");
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);

    let version_control = version_control_request_error_response(DavXmlError::InvalidGrammar)
        .expect("version-control error response");
    assert_eq!(version_control.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        version_control.headers.get("Cache-Control").unwrap(),
        "no-store"
    );
}

#[test]
fn version_tree_response_composes_multistatus_properties() {
    let response = version_tree_response(vec![DavVersionXml {
        href: "/webdav/file.txt?v=1".to_owned(),
        version_name: "V1".to_owned(),
        creator: "alice".to_owned(),
        content_length: 42,
        last_modified: "Sat, 25 Jul 2026 00:00:00 GMT".to_owned(),
    }])
    .expect("version response");
    assert_eq!(response.status, StatusCode::MULTI_STATUS);
    assert!(response.headers.get("Cache-Control").is_none());
    let xml = body_text(&response);
    assert!(xml.contains("version-name"), "{xml}");
    assert!(xml.contains("V1"), "{xml}");
    assert!(xml.contains("42"), "{xml}");
}

#[test]
fn file_only_and_version_control_responses_select_protocol_status() {
    let report = version_tree_non_file_response();
    assert_eq!(report.status, StatusCode::CONFLICT);
    assert_eq!(report.headers.get("Cache-Control").unwrap(), "no-store");

    let file = version_control_response(DavResourceKind::File);
    assert_eq!(file.status, StatusCode::OK);
    assert!(file.headers.get("Cache-Control").is_none());

    let collection = version_control_response(DavResourceKind::Collection);
    assert_eq!(collection.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(collection.headers.get("Cache-Control").unwrap(), "no-store");
}

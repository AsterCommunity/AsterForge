use aster_forge_webdav::{
    DavCapabilityDeclaration, DavExtensionPackage, DavExtensionSet, DavMethod, DavMethodSet,
    DavReportErrorResponsePolicy, DavReportPlanError, DavReportType, DavResourceKind,
    DavResourceState, DavResponse, DavResponseBody, DavVersionXml, DavXmlError, plan_capabilities,
    plan_report_request, report_plan_error_response, validate_version_control_request,
    version_control_request_error_response, version_control_response,
    version_tree_non_file_response, version_tree_response,
};
use http::StatusCode;

fn body_text(response: &DavResponse) -> String {
    let DavResponseBody::Bytes(body) = &response.body else {
        panic!("response should contain bytes");
    };
    String::from_utf8(body.to_vec()).expect("UTF-8 response")
}

struct ReportResponsePolicy;

impl DavReportErrorResponsePolicy for ReportResponsePolicy {
    fn unknown_type(&self, namespace: Option<&str>, name: &str) -> DavResponse {
        DavResponse::bytes(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown {namespace:?} {name}"),
        )
    }

    fn not_available(&self, report: DavReportType) -> DavResponse {
        DavResponse::bytes(
            StatusCode::CONFLICT,
            format!("unavailable {}", report.local_name()),
        )
    }
}

fn report_snapshot(packages: &[DavExtensionPackage]) -> aster_forge_webdav::DavCapabilitySnapshot {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::Collection,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Report]),
    );
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(packages);
    plan_capabilities(declaration).expect("REPORT snapshot")
}

#[test]
fn report_selector_accepts_only_snapshot_discovered_dav_report_types() {
    let snapshot = report_snapshot(&[DavExtensionPackage::VersionControl]);
    assert_eq!(
        plan_report_request(&snapshot, br#"<D:version-tree xmlns:D="DAV:"/>"#)
            .expect("valid report"),
        DavReportType::VersionTree
    );
    assert_eq!(
        plan_report_request(&snapshot, br#"<X:version-tree xmlns:X="DAV:"/>"#)
            .expect("prefix is lexical only"),
        DavReportType::VersionTree
    );
    plan_report_request(
        &snapshot,
        br#"<D:version-tree xmlns:D="DAV:" xmlns:X="urn:x">
              <D:future><D:prop><D:not-active/></D:prop></D:future>
              <D:prop><D:getetag><D:ignored>value</D:ignored></D:getetag></D:prop>
              <X:future/>
            </D:version-tree>"#,
    )
    .expect("unknown report extensions should be ignored");

    assert_eq!(
        plan_report_request(&snapshot, br#"<D:expand-property xmlns:D="DAV:"/>"#),
        Ok(DavReportType::ExpandProperty)
    );
    assert_eq!(
        plan_report_request(&snapshot, br#"<D:sync-collection xmlns:D="DAV:"/>"#),
        Err(DavReportPlanError::NotAvailable {
            report: DavReportType::SyncCollection,
        })
    );
    let sync = report_snapshot(&[DavExtensionPackage::CollectionSync]);
    assert_eq!(
        plan_report_request(&sync, br#"<D:sync-collection xmlns:D="DAV:"/>"#),
        Ok(DavReportType::SyncCollection)
    );
    assert_eq!(
        plan_report_request(&snapshot, br#"<X:version-tree xmlns:X="urn:extension"/>"#,),
        Err(DavReportPlanError::UnknownType {
            namespace: Some("urn:extension".to_owned()),
            name: "version-tree".to_owned(),
        })
    );
    assert_eq!(
        plan_report_request(
            &snapshot,
            br#"<X:version-tree xmlns:X="urn:other-extension"/>"#,
        ),
        Err(DavReportPlanError::UnknownType {
            namespace: Some("urn:other-extension".to_owned()),
            name: "version-tree".to_owned(),
        })
    );
    assert_eq!(
        plan_report_request(&snapshot, br#"<D:future-report xmlns:D="DAV:"/>"#),
        Err(DavReportPlanError::UnknownType {
            namespace: Some("DAV:".to_owned()),
            name: "future-report".to_owned(),
        })
    );
    let namespaced =
        plan_report_request(&snapshot, br#"<X:version-tree xmlns:X="urn:extension"/>"#)
            .expect_err("extension namespace is not a DAV REPORT type");
    assert_eq!(
        namespaced.to_string(),
        r#"unknown WebDAV REPORT type "version-tree" in namespace Some("urn:extension")"#
    );
    let unqualified = plan_report_request(&snapshot, br"<version-tree/>")
        .expect_err("an unqualified root is not a DAV REPORT type");
    assert_eq!(
        unqualified.to_string(),
        r#"unknown WebDAV REPORT type "version-tree" in namespace None"#
    );
    for body in [
        br#"<D:version-tree xmlns:D="DAV:"><D:prop/><D:prop/></D:version-tree>"#.as_slice(),
        br#"<D:version-tree xmlns:D="DAV:">text</D:version-tree>"#,
        br#"<D:version-tree xmlns:D="DAV:"><D:prop><D:getetag>value</D:getetag></D:prop></D:version-tree>"#,
    ] {
        assert_eq!(
            plan_report_request(&snapshot, body),
            Err(DavReportPlanError::Xml(
                DavXmlError::InvalidGrammar
            ))
        );
    }
}

#[test]
fn version_tree_known_prop_order_is_irrelevant_across_extensions() {
    let snapshot = report_snapshot(&[DavExtensionPackage::VersionControl]);
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
        plan_report_request(&snapshot, body)
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
    let snapshot = report_snapshot(&[DavExtensionPackage::VersionControl]);
    assert_eq!(
        plan_report_request(&snapshot, b"<D:version-tree"),
        Err(DavReportPlanError::Xml(DavXmlError::Malformed))
    );
    assert_eq!(
        plan_report_request(
            &snapshot,
            br#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]><D:version-tree xmlns:D="DAV:"/>"#,
        ),
        Err(DavReportPlanError::Xml(
            DavXmlError::ExternalEntity
        ))
    );
}

#[test]
fn report_errors_select_plain_or_dav_xml_responses() {
    let invalid = report_plan_error_response(
        &DavReportPlanError::Xml(DavXmlError::Malformed),
        &ReportResponsePolicy,
    )
    .expect("invalid response");
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.headers.get("Cache-Control").unwrap(), "no-store");
    assert_eq!(body_text(&invalid), "Invalid XML body");

    let unsupported = report_plan_error_response(
        &DavReportPlanError::UnknownType {
            namespace: Some("urn:extension".to_owned()),
            name: "expand-property".to_owned(),
        },
        &ReportResponsePolicy,
    )
    .expect("unsupported response");
    assert_eq!(unsupported.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_text(&unsupported).contains("urn:extension"));
    assert!(body_text(&unsupported).contains("expand-property"));

    let unavailable = report_plan_error_response(
        &DavReportPlanError::NotAvailable {
            report: DavReportType::SyncCollection,
        },
        &ReportResponsePolicy,
    )
    .expect("unavailable response");
    assert_eq!(unavailable.status, StatusCode::CONFLICT);
    assert!(body_text(&unavailable).contains("sync-collection"));

    let external = report_plan_error_response(
        &DavReportPlanError::Xml(DavXmlError::ExternalEntity),
        &ReportResponsePolicy,
    )
    .expect("external entity response");
    assert_eq!(external.status, StatusCode::FORBIDDEN);
    assert_eq!(
        external.headers.get("Content-Type").unwrap(),
        "application/xml; charset=utf-8"
    );
    assert!(body_text(&external).contains("no-external-entities"));

    let too_large = report_plan_error_response(
        &DavReportPlanError::Xml(DavXmlError::TooLarge),
        &ReportResponsePolicy,
    )
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

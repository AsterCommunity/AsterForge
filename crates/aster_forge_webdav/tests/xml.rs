use std::io::{self, Cursor, Read};

use aster_forge_webdav::{
    DavCapabilityDeclaration, DavCapabilitySnapshot, DavExtensionPackage, DavExtensionSet,
    DavMethod, DavMethodSet, DavPropfindRequest, DavReportPlanError, DavReportType,
    DavResourceState, DavVersionControlPlanError, DavVersioningCapabilities, DavVersioningState,
    DavXmlElement, DavXmlError, DavXmlNode, parse_lock_request, parse_propfind_request,
    parse_proppatch_request, plan_capabilities, plan_report_request, plan_version_control_request,
};
use aster_forge_xml::{DEFAULT_XML_MAX_DEPTH, XmlSafetyPolicy};

struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("fixture read failure"))
    }
}

type RequestParser = fn(&[u8]) -> Result<(), DavXmlError>;

fn propfind_parser(body: &[u8]) -> Result<(), DavXmlError> {
    parse_propfind_request(body).map(|_| ())
}

fn proppatch_parser(body: &[u8]) -> Result<(), DavXmlError> {
    parse_proppatch_request(body).map(|_| ())
}

fn lock_parser(body: &[u8]) -> Result<(), DavXmlError> {
    parse_lock_request(body).map(|_| ())
}

fn report_snapshot() -> DavCapabilitySnapshot {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::Collection,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::Report]),
    );
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::VersionControl]);
    declaration.versioning = DavVersioningCapabilities {
        state: DavVersioningState::CheckedIn,
        ..DavVersioningCapabilities::default()
    };
    plan_capabilities(declaration).expect("REPORT snapshot fixture")
}

fn report_parser(body: &[u8]) -> Result<(), DavXmlError> {
    match plan_report_request(&report_snapshot(), body) {
        Ok(_) => Ok(()),
        Err(DavReportPlanError::Xml(error)) => Err(error),
        Err(
            DavReportPlanError::InvalidLimits
            | DavReportPlanError::UnknownType { .. }
            | DavReportPlanError::NotAvailable { .. },
        ) => Err(DavXmlError::InvalidGrammar),
    }
}

fn version_control_parser(body: &[u8]) -> Result<(), DavXmlError> {
    let mut declaration = DavCapabilityDeclaration::new(
        DavResourceState::File,
        DavMethodSet::from_methods(&[DavMethod::Options, DavMethod::VersionControl]),
    );
    declaration.compliance.class1 = true;
    declaration.extensions = DavExtensionSet::from_packages(&[DavExtensionPackage::VersionControl]);
    declaration.versioning = DavVersioningCapabilities {
        state: DavVersioningState::Versionable,
        ..DavVersioningCapabilities::default()
    };
    let snapshot = plan_capabilities(declaration).expect("VERSION-CONTROL snapshot fixture");
    match plan_version_control_request(&snapshot, body) {
        Ok(_) => Ok(()),
        Err(DavVersionControlPlanError::Xml(error)) => Err(error),
        Err(DavVersionControlPlanError::MethodNotAllowed) => Err(DavXmlError::InvalidGrammar),
    }
}

const XML_METHOD_PARSERS: [(&str, RequestParser); 5] = [
    ("PROPFIND", propfind_parser),
    ("PROPPATCH", proppatch_parser),
    ("LOCK", lock_parser),
    ("REPORT", report_parser),
    ("VERSION-CONTROL", version_control_parser),
];

fn ignored_subtree(total_document_depth: usize) -> String {
    assert!(total_document_depth >= 2);
    let mut subtree = String::from("<X:ignored>");
    for _ in 2..total_document_depth {
        subtree.push_str("<X:nested>");
    }
    for _ in 2..total_document_depth {
        subtree.push_str("</X:nested>");
    }
    subtree.push_str("</X:ignored>");
    subtree
}

fn request_with_ignored_subtree(method: &str, subtree: &str) -> String {
    match method {
        "PROPFIND" => format!(
            r#"<D:propfind xmlns:D="DAV:" xmlns:X="urn:x">{subtree}<D:allprop/></D:propfind>"#
        ),
        "PROPPATCH" => format!(
            r#"<D:propertyupdate xmlns:D="DAV:" xmlns:X="urn:x">{subtree}<D:set><D:prop><X:active/></D:prop></D:set></D:propertyupdate>"#
        ),
        "LOCK" => format!(
            r#"<D:lockinfo xmlns:D="DAV:" xmlns:X="urn:x">{subtree}<D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#
        ),
        "REPORT" => format!(
            r#"<D:version-tree xmlns:D="DAV:" xmlns:X="urn:x">{subtree}<D:prop><D:getetag/></D:prop></D:version-tree>"#
        ),
        "VERSION-CONTROL" => format!(
            r#"<D:version-control xmlns:D="DAV:" xmlns:X="urn:x">{subtree}</D:version-control>"#
        ),
        _ => panic!("complete XML method fixture table"),
    }
}

fn request_with_ignored_payload(method: &str, payload_bytes: usize) -> Vec<u8> {
    let marker = "PAYLOAD";
    let template =
        request_with_ignored_subtree(method, &format!("<X:ignored>{marker}</X:ignored>"));
    let marker_start = template
        .find(marker)
        .expect("ignored payload marker should exist");
    let mut request = Vec::with_capacity(template.len() - marker.len() + payload_bytes);
    request.extend_from_slice(&template.as_bytes()[..marker_start]);
    request.resize(request.len() + payload_bytes, b'x');
    request.extend_from_slice(&template.as_bytes()[marker_start + marker.len()..]);
    request
}

#[test]
fn xml_method_absent_empty_and_whitespace_body_semantics_are_explicit() {
    assert_eq!(
        parse_propfind_request(b"").unwrap(),
        DavPropfindRequest::AllProp {
            include: Vec::new()
        }
    );
    version_control_parser(b"").expect("VERSION-CONTROL body is optional");

    for (method, parser) in XML_METHOD_PARSERS {
        if method != "PROPFIND" && method != "VERSION-CONTROL" {
            assert_eq!(parser(b""), Err(DavXmlError::Malformed), "{method}");
        }
        assert_eq!(
            parser(b" \r\n\t"),
            Err(DavXmlError::Malformed),
            "whitespace is a present, malformed XML body for {method}"
        );
    }

    assert_eq!(
        parse_propfind_request(br#"<D:propfind xmlns:D="DAV:"/>"#),
        Err(DavXmlError::InvalidGrammar)
    );
    version_control_parser(br#"<D:version-control xmlns:D="DAV:"/>"#)
        .expect("present empty VERSION-CONTROL root is valid");
}

#[test]
fn every_xml_method_applies_safety_before_ignored_subtree_semantics() {
    for (method, parser) in XML_METHOD_PARSERS {
        let exact = request_with_ignored_subtree(method, &ignored_subtree(DEFAULT_XML_MAX_DEPTH));
        parser(exact.as_bytes())
            .unwrap_or_else(|error| panic!("{method} should accept exact max depth: {error}"));

        let over =
            request_with_ignored_subtree(method, &ignored_subtree(DEFAULT_XML_MAX_DEPTH + 1));
        assert_eq!(
            parser(over.as_bytes()),
            Err(DavXmlError::TooDeep),
            "{method} must reject one level over the shared depth limit"
        );

        let entity_subtree = "<X:ignored>&external;</X:ignored>";
        let with_entity = format!(
            "<!DOCTYPE root [<!ENTITY external SYSTEM \"file:///TARGET\">]>{}",
            request_with_ignored_subtree(method, entity_subtree)
        );
        assert_eq!(
            parser(with_entity.as_bytes()),
            Err(DavXmlError::ExternalEntity),
            "{method} must validate ignored subtrees before semantic filtering"
        );

        let valid = request_with_ignored_subtree(method, "<X:ignored/>");
        let malformed = &valid.as_bytes()[..valid.len() - 1];
        assert_eq!(
            parser(malformed),
            Err(DavXmlError::Malformed),
            "{method} must reject truncated documents"
        );

        let multiple_roots = format!(r#"{valid}<X:second xmlns:X="urn:x"/>"#);
        assert_eq!(
            parser(multiple_roots.as_bytes()),
            Err(DavXmlError::Malformed),
            "{method} must reject multiple document roots"
        );
    }
}

#[test]
fn every_xml_method_accepts_exact_input_limit_and_rejects_one_byte_over() {
    let maximum = XmlSafetyPolicy::untrusted().max_input_bytes;
    for (method, parser) in XML_METHOD_PARSERS {
        let empty = request_with_ignored_payload(method, 0);
        let payload_bytes = maximum
            .checked_sub(empty.len())
            .expect("fixture shell should fit below the XML input limit");
        let exact = request_with_ignored_payload(method, payload_bytes);
        assert_eq!(exact.len(), maximum, "{method}");
        parser(&exact).unwrap_or_else(|error| {
            panic!("{method} should accept the exact input limit: {error}")
        });

        let over = request_with_ignored_payload(method, payload_bytes + 1);
        assert_eq!(over.len(), maximum + 1, "{method}");
        assert_eq!(
            parser(&over),
            Err(DavXmlError::TooLarge),
            "{method} must reject one byte over the shared input limit"
        );
    }
}

#[test]
fn propfind_absent_body_and_namespace_forms() {
    assert_eq!(
        parse_propfind_request(b"").unwrap(),
        DavPropfindRequest::AllProp {
            include: Vec::new()
        }
    );

    for xml in [
        br#"<propfind xmlns="DAV:"><propname/></propfind>"#.as_slice(),
        br#"<D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#,
        br#"<d:propfind xmlns:d="DAV:"><d:propname/></d:propfind>"#,
    ] {
        assert_eq!(
            parse_propfind_request(xml).unwrap(),
            DavPropfindRequest::PropName
        );
    }
}

#[test]
fn propfind_qname_collisions_do_not_activate_dav_controls() {
    for xml in [
        br#"<propfind xmlns="urn:not-dav"><propname/></propfind>"#.as_slice(),
        br#"<X:propfind xmlns:X="urn:not-dav"><D:propname xmlns:D="DAV:"/></X:propfind>"#,
        br#"<D:propfind xmlns:D="DAV:" xmlns:X="urn:not-dav"><X:propname/></D:propfind>"#,
    ] {
        assert_eq!(
            parse_propfind_request(xml),
            Err(DavXmlError::InvalidGrammar)
        );
    }
}

#[test]
fn propfind_unknown_attributes_children_and_subtrees_are_ignored() {
    let request = parse_propfind_request(
        br#"<D:propfind xmlns:D="DAV:" xmlns:X="urn:ext" X:flag="1">
              <X:before><D:prop><D:getetag/></D:prop></X:before>
              <D:allprop X:mode="fast"><X:nested><D:propname/></X:nested></D:allprop>
              <X:after/>
            </D:propfind>"#,
    )
    .unwrap();
    assert_eq!(
        request,
        DavPropfindRequest::AllProp {
            include: Vec::new()
        }
    );
}

#[test]
fn propfind_rejects_character_data_and_property_values() {
    for xml in [
        br#"<D:propfind xmlns:D="DAV:">text<D:allprop/></D:propfind>"#.as_slice(),
        br#"<D:propfind xmlns:D="DAV:"><D:allprop>text</D:allprop></D:propfind>"#,
        br#"<D:propfind xmlns:D="DAV:"><D:prop><D:getetag>value</D:getetag></D:prop></D:propfind>"#,
    ] {
        assert_eq!(
            parse_propfind_request(xml),
            Err(DavXmlError::InvalidGrammar)
        );
    }

    parse_propfind_request(
        br#"<D:propfind xmlns:D="DAV:">
              <D:include><D:getetag><D:ignored>value</D:ignored></D:getetag></D:include>
              <D:allprop/>
            </D:propfind>"#,
    )
    .expect("unrecognized property-name children should be ignored with their subtrees");
}

#[test]
fn propfind_include_preserves_qnames_and_order() {
    let request = parse_propfind_request(
        br#"<D:propfind xmlns:D="DAV:" xmlns:A="urn:a">
              <D:allprop/>
              <D:include><D:getetag/><A:color/><plain/></D:include>
            </D:propfind>"#,
    )
    .unwrap();
    let DavPropfindRequest::AllProp { include } = request else {
        panic!("expected allprop");
    };
    assert_eq!(include.len(), 3);
    assert_eq!(include[0].name, "getetag");
    assert_eq!(include[0].namespace.as_deref(), Some("DAV:"));
    assert_eq!(include[1].name, "color");
    assert_eq!(include[1].namespace.as_deref(), Some("urn:a"));
    assert_eq!(include[2].name, "plain");
    assert_eq!(include[2].namespace, None);
}

#[test]
fn propfind_rejects_duplicates_and_mutually_exclusive_selectors() {
    for xml in [
        br#"<D:propfind xmlns:D="DAV:"><D:allprop/><D:allprop/></D:propfind>"#.as_slice(),
        br#"<D:propfind xmlns:D="DAV:"><D:propname/><D:prop/></D:propfind>"#,
        br#"<D:propfind xmlns:D="DAV:"><D:allprop/><D:include/><D:include/></D:propfind>"#,
        br#"<D:propfind xmlns:D="DAV:"><D:propname/><D:include/></D:propfind>"#,
        br#"<D:propfind xmlns:D="DAV:"/>"#,
    ] {
        assert_eq!(
            parse_propfind_request(xml),
            Err(DavXmlError::InvalidGrammar)
        );
    }
}

#[test]
fn all_xml_parsers_reject_empty_whitespace_declaration_and_multiple_roots() {
    for xml in [
        b"".as_slice(),
        b"   \r\n\t",
        br#"<?xml version="1.0"?>"#,
        b"<a/><b/>",
        b"<",
    ] {
        assert!(parse_proppatch_request(xml).is_err());
        assert!(parse_lock_request(xml).is_err());
        assert!(report_parser(xml).is_err());
    }
}

#[test]
fn safety_validation_precedes_unknown_subtree_extensibility() {
    for xml in [
        br#"<!DOCTYPE D:propfind [<!ENTITY x "boom">]><D:propfind xmlns:D="DAV:"><X:ignored xmlns:X="urn:x">&x;</X:ignored><D:allprop/></D:propfind>"#.as_slice(),
        br#"<!ENTITY x "boom"><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#,
    ] {
        assert_eq!(
            parse_propfind_request(xml),
            Err(DavXmlError::ExternalEntity)
        );
    }
}

#[test]
fn safety_depth_accepts_exact_limit_and_rejects_one_over_even_when_ignored() {
    fn nested(depth: usize) -> Vec<u8> {
        let mut xml = String::from("<D:propfind xmlns:D=\"DAV:\"><X:ignored xmlns:X=\"urn:x\">");
        for _ in 2..depth {
            xml.push_str("<X:x>");
        }
        for _ in 2..depth {
            xml.push_str("</X:x>");
        }
        xml.push_str("</X:ignored><D:allprop/></D:propfind>");
        xml.into_bytes()
    }

    assert!(parse_propfind_request(&nested(DEFAULT_XML_MAX_DEPTH)).is_ok());
    assert_eq!(
        parse_propfind_request(&nested(DEFAULT_XML_MAX_DEPTH + 1)),
        Err(DavXmlError::TooDeep)
    );
}

#[test]
fn proppatch_preserves_order_qnames_values_and_inherited_language() {
    let patches = parse_proppatch_request(
        r#"<D:propertyupdate xmlns:D="DAV:" xmlns:A="urn:a" xml:lang="zh-CN">
              <D:set><D:prop><A:color shade="深">蓝色<![CDATA[+青]]></A:color></D:prop></D:set>
              <D:remove xml:lang="en"><D:prop><A:obsolete/></D:prop></D:remove>
            </D:propertyupdate>"#
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(patches.len(), 2);
    assert!(patches[0].set);
    assert_eq!(patches[0].property.name, "color");
    assert_eq!(patches[0].property.namespace.as_deref(), Some("urn:a"));
    assert_eq!(
        patches[0].property.element.attributes.get("xml:lang"),
        Some(&"zh-CN".to_owned())
    );
    assert_eq!(
        patches[0].property.element.text().as_deref(),
        Some("蓝色+青")
    );
    assert!(!patches[1].set);
    assert_eq!(
        patches[1].property.element.attributes.get("xml:lang"),
        Some(&"en".to_owned())
    );
}

#[test]
fn proppatch_does_not_inherit_an_unqualified_lang_attribute() {
    let patches = parse_proppatch_request(
        br#"<D:propertyupdate xmlns:D="DAV:" xmlns:A="urn:a" lang="not-xml">
              <D:set><D:prop><A:color/></D:prop></D:set>
            </D:propertyupdate>"#,
    )
    .expect("unqualified lang is an ordinary application attribute");

    assert_eq!(patches.len(), 1);
    assert!(
        !patches[0]
            .property
            .element
            .attributes
            .contains_key("xml:lang")
    );
}

#[test]
fn proppatch_ignores_unknown_action_subtrees_but_rejects_known_grammar_errors() {
    let patches = parse_proppatch_request(
        br#"<D:propertyupdate xmlns:D="DAV:" xmlns:X="urn:x">
              <X:set><D:prop><X:not-active/></D:prop></X:set>
              <D:set>
                <X:ignored><D:prop/></X:ignored>
                <D:future><D:prop/></D:future>
                <D:prop><X:active/></D:prop>
              </D:set>
            </D:propertyupdate>"#,
    )
    .unwrap();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].property.name, "active");

    for xml in [
        br#"<D:propertyupdate xmlns:D="DAV:"><D:set/></D:propertyupdate>"#.as_slice(),
        br#"<D:propertyupdate xmlns:D="DAV:"><D:set><D:prop/><D:prop/></D:set></D:propertyupdate>"#,
        br#"<D:propertyupdate xmlns:D="DAV:"/>"#,
        br#"<X:propertyupdate xmlns:X="urn:x"><X:set><X:prop><X:a/></X:prop></X:set></X:propertyupdate>"#,
        br#"<D:propertyupdate xmlns:D="DAV:">text<D:set><D:prop><D:a/></D:prop></D:set></D:propertyupdate>"#,
        br#"<D:propertyupdate xmlns:D="DAV:"><D:remove><D:prop><D:a>value</D:a></D:prop></D:remove></D:propertyupdate>"#,
    ] {
        assert_eq!(
            parse_proppatch_request(xml),
            Err(DavXmlError::InvalidGrammar)
        );
    }

    parse_proppatch_request(
        br#"<D:propertyupdate xmlns:D="DAV:">
              <D:remove><D:prop><D:a><D:ignored>value</D:ignored></D:a></D:prop></D:remove>
            </D:propertyupdate>"#,
    )
    .expect("unrecognized remove-property children should be ignored with their subtrees");
}

#[test]
fn proppatch_preserves_known_action_order_across_extensions() {
    let patches = parse_proppatch_request(
        br#"<D:propertyupdate xmlns:D="DAV:" xmlns:X="urn:x">
              <D:remove><D:prop><X:first/></D:prop></D:remove>
              <D:future><D:set><D:prop><X:not-active/></D:prop></D:set></D:future>
              <D:set><D:prop><X:second>value</X:second></D:prop></D:set>
              <X:future/>
              <D:remove><D:prop><X:third/></D:prop></D:remove>
            </D:propertyupdate>"#,
    )
    .expect("valid ordered PROPPATCH");
    let operations = patches
        .iter()
        .map(|patch| (patch.set, patch.property.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        vec![(false, "first"), (true, "second"), (false, "third")]
    );
}

#[test]
fn lock_parses_exclusive_shared_owner_and_extensions() {
    for (scope, shared) in [("exclusive", false), ("shared", true)] {
        let xml = format!(
            r#"<D:lockinfo xmlns:D="DAV:" xmlns:X="urn:x">
                  <X:before><D:lockscope><D:shared/></D:lockscope></X:before>
                  <D:lockscope><X:ignored/><D:future/><D:{scope}/></D:lockscope>
                  <D:locktype><X:ignored/><D:future/><D:write/></D:locktype>
                  <D:owner><D:href>用户 &amp; owner</D:href></D:owner>
                </D:lockinfo>"#
        );
        let request = parse_lock_request(xml.as_bytes()).unwrap();
        assert_eq!(request.shared, shared);
        let owner = request.owner.unwrap();
        assert_eq!(owner.name, "owner");
        assert!(
            owner
                .to_bytes()
                .unwrap()
                .windows(4)
                .any(|part| part == b"&amp")
        );
    }
}

#[test]
fn lock_rejects_qname_collisions_missing_controls_and_duplicates() {
    for xml in [
        br#"<X:lockinfo xmlns:X="urn:x"><X:lockscope><X:exclusive/></X:lockscope><X:locktype><X:write/></X:locktype></X:lockinfo>"#.as_slice(),
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:future/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/><D:shared/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:lockscope><D:shared/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype><D:owner/><D:owner/></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:">text<D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive>text</D:exclusive></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
    ] {
        assert_eq!(parse_lock_request(xml), Err(DavXmlError::InvalidGrammar));
    }
}

#[test]
fn lock_known_control_order_is_irrelevant_but_nested_controls_stay_inactive() {
    for body in [
        br#"<D:lockinfo xmlns:D="DAV:" xmlns:X="urn:x">
              <D:owner><D:href>owner</D:href></D:owner>
              <D:locktype><D:write/></D:locktype>
              <X:ignored><D:lockscope><D:shared/></D:lockscope></X:ignored>
              <D:lockscope><D:exclusive/></D:lockscope>
            </D:lockinfo>"#
            .as_slice(),
        br#"<D:lockinfo xmlns:D="DAV:">
              <D:locktype><D:future/><D:write/></D:locktype>
              <D:lockscope><D:future/><D:shared/></D:lockscope>
            </D:lockinfo>"#,
    ] {
        parse_lock_request(body).expect("recognized LOCK controls may appear in either order");
    }
}

#[test]
fn report_planner_is_qname_aware() {
    let snapshot = report_snapshot();
    assert_eq!(
        plan_report_request(
            &snapshot,
            br#"<D:version-tree xmlns:D="DAV:" X:flag="1" xmlns:X="urn:x"/>"#,
        )
        .expect("DAV report")
        .report_type(),
        DavReportType::VersionTree
    );
    assert_eq!(
        plan_report_request(&snapshot, br#"<X:version-tree xmlns:X="DAV:"/>"#)
            .expect("expanded DAV QName")
            .report_type(),
        DavReportType::VersionTree,
        "the prefix is lexical; the expanded QName selects the REPORT"
    );
    assert_eq!(
        plan_report_request(&snapshot, br#"<X:version-tree xmlns:X="urn:x"/>"#),
        Err(DavReportPlanError::UnknownType {
            namespace: Some("urn:x".to_owned()),
            name: "version-tree".to_owned(),
        })
    );
}

#[test]
fn xml_boundary_round_trips_namespaces_comments_cdata_utf8_and_escaping() {
    let original = r#"<A:color xmlns:A="urn:a" quote="&quot; &amp; &lt;">蓝<![CDATA[+青]]><!--note--></A:color>"#;
    let element = DavXmlElement::parse(original.as_bytes()).unwrap();
    let bytes = element.to_bytes().unwrap();
    let reparsed = DavXmlElement::parse(&bytes).unwrap();
    assert_eq!(reparsed.name, "color");
    assert_eq!(reparsed.namespace.as_deref(), Some("urn:a"));
    assert_eq!(reparsed.text().as_deref(), Some("蓝+青"));
    assert_eq!(
        reparsed.attributes.get("quote").map(String::as_str),
        Some("\" & <")
    );
    assert!(
        reparsed
            .children
            .iter()
            .any(|node| matches!(node, DavXmlNode::Comment(_)))
    );
}

#[test]
fn xml_boundary_preserves_default_namespace_undeclaration() {
    let original = br#"<D:root xmlns:D="DAV:" xmlns="urn:default"><plain xmlns=""/></D:root>"#;
    let element = DavXmlElement::parse(original).unwrap();
    let bytes = element.to_bytes().unwrap();
    let reparsed = DavXmlElement::parse(&bytes).unwrap();
    let plain = reparsed.child_elements().next().unwrap();

    assert_eq!(plain.name, "plain");
    assert_eq!(plain.namespace, None);
}

#[test]
fn xml_boundary_round_trips_namespace_shadowing_and_namespaced_attributes() {
    let original = br#"<root xmlns="urn:root" xmlns:a="urn:a"><a:item a:id="7"><plain xmlns=""><a:leaf xmlns:a="urn:b"/></plain></a:item></root>"#;
    let element = DavXmlElement::parse(original).unwrap();
    let bytes = element.to_bytes().unwrap();
    let reparsed = DavXmlElement::parse(&bytes).unwrap();
    let item = reparsed.child_elements().next().unwrap();
    let plain = item.child_elements().next().unwrap();
    let leaf = plain.child_elements().next().unwrap();

    assert_eq!(reparsed.namespace.as_deref(), Some("urn:root"));
    assert_eq!(item.namespace.as_deref(), Some("urn:a"));
    assert_eq!(item.attributes.get("a:id").map(String::as_str), Some("7"));
    assert_eq!(plain.namespace, None);
    assert_eq!(leaf.namespace.as_deref(), Some("urn:b"));
}

#[test]
fn xml_reader_maps_io_invalid_encoding_and_size_boundaries() {
    assert_eq!(
        DavXmlElement::parse_reader(FailingReader),
        Err(DavXmlError::Malformed)
    );
    assert_eq!(
        DavXmlElement::parse_reader(Cursor::new(b"<root>\xff</root>")),
        Err(DavXmlError::Malformed)
    );
    assert_eq!(
        DavXmlElement::parse_reader(Cursor::new(b"<a/><b/>")),
        Err(DavXmlError::Malformed)
    );

    let max_input_bytes = XmlSafetyPolicy::untrusted().max_input_bytes;
    let mut exact = Vec::with_capacity(max_input_bytes);
    exact.extend_from_slice(b"<root>");
    exact.resize(max_input_bytes - b"</root>".len(), b'x');
    exact.extend_from_slice(b"</root>");
    assert_eq!(exact.len(), max_input_bytes);
    assert!(DavXmlElement::parse_reader(Cursor::new(&exact)).is_ok());

    exact.insert(b"<root>".len(), b'x');
    assert_eq!(exact.len(), max_input_bytes + 1);
    assert_eq!(
        DavXmlElement::parse_reader(Cursor::new(&exact)),
        Err(DavXmlError::TooLarge)
    );
}

#[test]
fn xml_writer_rejects_invalid_models_and_conflicting_namespaces() {
    let unbound_prefix = DavXmlElement::new("p:root");
    assert_eq!(unbound_prefix.to_bytes(), Err(DavXmlError::Malformed));

    let mut conflicting_namespace = DavXmlElement::dav("root");
    conflicting_namespace
        .attributes
        .insert("xmlns:D".to_owned(), "urn:not-dav".to_owned());
    assert_eq!(
        conflicting_namespace.to_bytes(),
        Err(DavXmlError::Malformed)
    );

    for node in [
        DavXmlNode::Text("bad\u{0}text".to_owned()),
        DavXmlNode::CData("bad]]>cdata".to_owned()),
        DavXmlNode::Comment("bad--comment".to_owned()),
        DavXmlNode::ProcessingInstruction("xml".to_owned(), None),
        DavXmlNode::ProcessingInstruction("work".to_owned(), Some("bad?>pi".to_owned())),
    ] {
        let mut element = DavXmlElement::dav("root");
        element.children.push(node);
        assert_eq!(element.to_bytes(), Err(DavXmlError::Malformed));
    }
}

#[test]
fn xml_writer_accepts_exact_depth_and_rejects_one_over() {
    fn nested(depth: usize) -> DavXmlElement {
        let mut element = DavXmlElement::dav("node");
        for _ in 1..depth {
            let mut parent = DavXmlElement::dav("node");
            parent.children.push(DavXmlNode::Element(element));
            element = parent;
        }
        element
    }

    assert!(nested(DEFAULT_XML_MAX_DEPTH).to_bytes().is_ok());
    assert_eq!(
        nested(DEFAULT_XML_MAX_DEPTH + 1).to_bytes(),
        Err(DavXmlError::TooDeep)
    );
}

#[test]
fn xml_writer_escapes_text_and_attributes() {
    let mut element = DavXmlElement::dav("href");
    element.namespaces.insert("D".to_owned(), "DAV:".to_owned());
    element
        .attributes
        .insert("data".to_owned(), "\"<&".to_owned());
    element
        .children
        .push(DavXmlNode::Text("/猫猫?a=1&b=<x>".to_owned()));
    let bytes = element.to_bytes().unwrap();
    let xml = String::from_utf8(bytes).unwrap();
    assert!(xml.contains("data=\"&quot;&lt;&amp;\""), "{xml}");
    assert!(xml.contains("/猫猫?a=1&amp;b=&lt;x&gt;"), "{xml}");
}

#[test]
fn large_bounded_property_payload_round_trips() {
    let payload = "猫&<tag>".repeat(32_768);
    let escaped = payload
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let xml = format!(r#"<A:value xmlns:A="urn:large">{escaped}</A:value>"#);
    let element = DavXmlElement::parse(xml.as_bytes()).unwrap();
    assert_eq!(element.text().as_deref(), Some(payload.as_str()));
    let reparsed = DavXmlElement::parse(&element.to_bytes().unwrap()).unwrap();
    assert_eq!(reparsed.text().as_deref(), Some(payload.as_str()));
}

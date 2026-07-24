use aster_forge_utils::xml::DEFAULT_XML_MAX_DEPTH;
use aster_forge_webdav::{
    DavPropfindRequest, DavXmlElement, DavXmlError, DavXmlNode, parse_lock_request,
    parse_propfind_request, parse_proppatch_request, parse_report_root,
};

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
        assert!(parse_report_root(xml).is_err());
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
fn proppatch_ignores_unknown_action_subtrees_but_rejects_known_grammar_errors() {
    let patches = parse_proppatch_request(
        br#"<D:propertyupdate xmlns:D="DAV:" xmlns:X="urn:x">
              <X:set><D:prop><X:not-active/></D:prop></X:set>
              <D:set><X:ignored><D:prop/></X:ignored><D:prop><X:active/></D:prop></D:set>
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
    ] {
        assert_eq!(
            parse_proppatch_request(xml),
            Err(DavXmlError::InvalidGrammar)
        );
    }
}

#[test]
fn lock_parses_exclusive_shared_owner_and_extensions() {
    for (scope, shared) in [("exclusive", false), ("shared", true)] {
        let xml = format!(
            r#"<D:lockinfo xmlns:D="DAV:" xmlns:X="urn:x">
                  <X:before><D:lockscope><D:shared/></D:lockscope></X:before>
                  <D:lockscope><X:ignored/><D:{scope}/></D:lockscope>
                  <D:locktype><X:ignored/><D:write/></D:locktype>
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
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/><D:shared/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/><D:other/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:lockscope><D:shared/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype><D:owner/><D:owner/></D:lockinfo>"#,
    ] {
        assert_eq!(parse_lock_request(xml), Err(DavXmlError::InvalidGrammar));
    }
}

#[test]
fn report_root_is_qname_aware() {
    let root = parse_report_root(br#"<D:version-tree xmlns:D="DAV:" X:flag="1" xmlns:X="urn:x"/>"#)
        .unwrap();
    assert_eq!(root.name, "version-tree");
    assert_eq!(root.namespace.as_deref(), Some("DAV:"));
    assert_eq!(root.prefix.as_deref(), Some("D"));

    let collision = parse_report_root(br#"<X:version-tree xmlns:X="urn:x"/>"#).unwrap();
    assert_eq!(collision.namespace.as_deref(), Some("urn:x"));
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

use std::time::Duration;

use aster_forge_webdav::{
    DavErrorCondition, DavLockXml, DavMultiStatusItem, DavMultiStatusLimits, DavPropStat,
    DavVersionXml, DavXmlElement, DavXmlNode, dav_dead_property_element, dav_element,
    dav_error_element, dav_lock_discovery_element, dav_lock_response_element,
    dav_multistatus_bytes, dav_property_child_element, dav_property_name_element,
    dav_property_text_element, dav_supported_lock_element, dav_version_multistatus_bytes,
};
use http::StatusCode;

fn xml(element: &DavXmlElement) -> String {
    String::from_utf8(element.to_bytes().unwrap()).unwrap()
}

#[test]
fn multistatus_preserves_response_propstat_and_property_order() {
    let output = String::from_utf8(
        dav_multistatus_bytes(
            vec![
                DavMultiStatusItem::properties(
                    "/webdav/a%20b.txt",
                    vec![
                        DavPropStat {
                            status: StatusCode::OK.as_u16(),
                            properties: vec![dav_element("displayname"), dav_element("getetag")],
                        },
                        DavPropStat {
                            status: StatusCode::NOT_FOUND.as_u16(),
                            properties: vec![dav_element("quota-used-bytes")],
                        },
                    ],
                ),
                DavMultiStatusItem::status("/webdav/locked", StatusCode::LOCKED.as_u16())
                    .with_error(DavErrorCondition::LockTokenSubmitted {
                        href: "/webdav/locked".to_owned(),
                    }),
            ],
            DavMultiStatusLimits::default(),
        )
        .expect("Multi-Status bytes"),
    )
    .expect("UTF-8 XML");
    assert!(output.contains("xmlns:D=\"DAV:\""), "{output}");
    assert!(output.contains("HTTP/1.1 200 OK"), "{output}");
    assert!(output.contains("HTTP/1.1 404 Not Found"), "{output}");
    assert!(output.contains("HTTP/1.1 423 Locked"), "{output}");
    assert!(output.contains("lock-token-submitted"), "{output}");
    assert!(
        output.find("displayname").unwrap() < output.find("getetag").unwrap(),
        "{output}"
    );
    DavXmlElement::parse(output.as_bytes()).unwrap();
}

#[test]
fn error_documents_cover_every_owned_condition_and_escape_hrefs() {
    let cases = [
        (
            DavErrorCondition::NoExternalEntities,
            "no-external-entities",
        ),
        (
            DavErrorCondition::LockTokenSubmitted {
                href: "/webdav/a?x=1&y=<2>".to_owned(),
            },
            "lock-token-submitted",
        ),
        (
            DavErrorCondition::LockTokenMatchesRequestUri,
            "lock-token-matches-request-uri",
        ),
        (
            DavErrorCondition::PropfindFiniteDepth,
            "propfind-finite-depth",
        ),
    ];
    for (condition, expected) in cases {
        let output = xml(&dav_error_element(&condition));
        assert!(output.contains(expected), "{output}");
        assert!(output.contains("xmlns:D=\"DAV:\""), "{output}");
        if matches!(condition, DavErrorCondition::LockTokenSubmitted { .. }) {
            assert!(output.contains("&amp;"), "{output}");
            assert!(output.contains("&lt;2&gt;"), "{output}");
        }
        DavXmlElement::parse(output.as_bytes()).unwrap();
    }
}

#[test]
fn supportedlock_has_exclusive_and_shared_write_entries() {
    let output = xml(&dav_supported_lock_element());
    assert_eq!(output.matches("<D:lockentry>").count(), 2, "{output}");
    assert!(output.contains("<D:exclusive"), "{output}");
    assert!(output.contains("<D:shared"), "{output}");
    assert_eq!(output.matches("<D:write").count(), 2, "{output}");
}

#[test]
fn lockdiscovery_covers_owner_timeout_scope_depth_token_and_root() {
    let mut owner = dav_element("owner");
    owner
        .children
        .push(DavXmlNode::Element(aster_forge_webdav::dav_text_element(
            "href",
            "用户 & owner",
        )));
    let locks = [
        DavLockXml {
            token: "urn:uuid:a b".to_owned(),
            owner: Some(owner),
            timeout: Some(Duration::from_secs(120)),
            shared: false,
            deep: true,
            root_href: "/webdav/a%20b".to_owned(),
        },
        DavLockXml {
            token: "urn:uuid:shared".to_owned(),
            owner: None,
            timeout: None,
            shared: true,
            deep: false,
            root_href: "/webdav/shared".to_owned(),
        },
    ];
    let output = xml(&dav_lock_discovery_element(&locks));
    assert!(output.contains("<D:exclusive"), "{output}");
    assert!(output.contains("<D:shared"), "{output}");
    assert!(output.contains("Second-120"), "{output}");
    assert!(output.contains("Infinite"), "{output}");
    assert!(output.contains("Infinity"), "{output}");
    assert!(output.contains("urn:uuid:a%20b"), "{output}");
    assert!(output.contains("用户 &amp; owner"), "{output}");
    let parsed = DavXmlElement::parse(output.as_bytes()).expect("lockdiscovery XML");
    let active_locks = parsed.child_elements().collect::<Vec<_>>();
    assert_eq!(active_locks.len(), 2);
    assert_eq!(
        active_locks[0]
            .child_elements()
            .map(|element| element.name.as_str())
            .collect::<Vec<_>>(),
        [
            "lockscope",
            "locktype",
            "depth",
            "owner",
            "timeout",
            "locktoken",
            "lockroot",
        ]
    );
    assert_eq!(
        active_locks[1]
            .child_elements()
            .map(|element| element.name.as_str())
            .collect::<Vec<_>>(),
        [
            "lockscope",
            "locktype",
            "depth",
            "timeout",
            "locktoken",
            "lockroot",
        ]
    );

    let response = xml(&dav_lock_response_element(&locks));
    assert!(response.contains("xmlns:D=\"DAV:\""), "{response}");
    DavXmlElement::parse(response.as_bytes()).unwrap();
}

#[test]
fn deltav_multistatus_escapes_values_and_keeps_protocol_property_order() {
    let output = String::from_utf8(
        dav_version_multistatus_bytes(
            vec![DavVersionXml {
                href: "/webdav/a?v=1&kind=<old>".to_owned(),
                version_name: "V1".to_owned(),
                creator: "猫 & owner".to_owned(),
                content_length: 42,
                last_modified: "Thu, 01 Jan 1970 00:00:00 GMT".to_owned(),
            }],
            DavMultiStatusLimits::default(),
        )
        .expect("DeltaV Multi-Status bytes"),
    )
    .expect("UTF-8 XML");
    assert!(output.contains("?v=1&amp;kind=&lt;old&gt;"), "{output}");
    assert!(output.contains("猫 &amp; owner"), "{output}");
    let names = [
        "version-name",
        "creator-displayname",
        "getcontentlength",
        "getlastmodified",
    ];
    for pair in names.windows(2) {
        assert!(
            output.find(pair[0]).unwrap() < output.find(pair[1]).unwrap(),
            "{output}"
        );
    }
    DavXmlElement::parse(output.as_bytes()).unwrap();
}

#[test]
fn property_builders_preserve_qnames_values_and_namespace_declarations() {
    let dav = aster_forge_webdav::DavRequestedProperty {
        name: "getetag".to_owned(),
        namespace: Some("DAV:".to_owned()),
        prefix: Some("Z".to_owned()),
    };
    let custom = aster_forge_webdav::DavRequestedProperty {
        name: "color".to_owned(),
        namespace: Some("urn:custom".to_owned()),
        prefix: None,
    };
    let plain = aster_forge_webdav::DavRequestedProperty {
        name: "plain".to_owned(),
        namespace: None,
        prefix: None,
    };
    let dav_xml = xml(&dav_property_text_element(&dav, "\"etag&1\""));
    assert!(dav_xml.contains("<Z:getetag"), "{dav_xml}");
    assert!(dav_xml.contains("xmlns:Z=\"DAV:\""), "{dav_xml}");
    assert!(dav_xml.contains("&amp;"), "{dav_xml}");

    let custom_xml = xml(&dav_property_child_element(
        &custom,
        dav_element("collection"),
    ));
    assert!(custom_xml.contains("<A:color"), "{custom_xml}");
    assert!(
        custom_xml.contains("xmlns:A=\"urn:custom\""),
        "{custom_xml}"
    );
    assert!(custom_xml.contains("<D:collection"), "{custom_xml}");
    assert!(xml(&dav_property_name_element(&plain)).contains("<plain"));

    for (reserved, expected_prefix) in [("xml", "A"), ("xmlns", "A")] {
        let requested = aster_forge_webdav::DavRequestedProperty {
            name: "color".to_owned(),
            namespace: Some("urn:custom".to_owned()),
            prefix: Some(reserved.to_owned()),
        };
        let output = xml(&dav_property_name_element(&requested));
        assert!(
            output.contains(&format!("<{expected_prefix}:color")),
            "{output}"
        );
        assert!(
            output.contains(&format!("xmlns:{expected_prefix}=\"urn:custom\"")),
            "{output}"
        );
        assert!(!output.contains(&format!("<{reserved}:color")), "{output}");
    }

    let reserved_dav = aster_forge_webdav::DavRequestedProperty {
        name: "getetag".to_owned(),
        namespace: Some("DAV:".to_owned()),
        prefix: Some("xml".to_owned()),
    };
    assert!(
        xml(&dav_property_name_element(&reserved_dav)).contains("<D:getetag"),
        "reserved DAV prefix should fall back to D"
    );
}

#[test]
fn dead_property_builder_preserves_valid_content_and_escapes_invalid_values() {
    let stored_name = aster_forge_webdav::DavRequestedProperty {
        name: "title".to_owned(),
        namespace: Some("urn:custom".to_owned()),
        prefix: Some("A".to_owned()),
    };
    let requested_name = aster_forge_webdav::DavRequestedProperty {
        name: "title".to_owned(),
        namespace: Some("urn:custom".to_owned()),
        prefix: Some("X".to_owned()),
    };
    let valid = dav_dead_property_element(
        &stored_name,
        Some(&requested_name),
        Some(
            r#"<A:title xmlns:A="urn:custom" xml:lang="zh">标题<B:b xmlns:B="urn:b">粗体</B:b></A:title>"#
                .as_bytes(),
        ),
    );
    let valid_xml = xml(&valid);
    assert!(valid_xml.contains("<X:title"), "{valid_xml}");
    assert!(valid_xml.contains("xmlns:X=\"urn:custom\""), "{valid_xml}");
    assert!(valid_xml.contains("xml:lang=\"zh\""), "{valid_xml}");
    assert!(valid_xml.contains("<B:b"), "{valid_xml}");

    let invalid = xml(&dav_dead_property_element(
        &stored_name,
        None,
        Some(b"<broken & value>"),
    ));
    assert!(invalid.contains("&lt;broken &amp; value&gt;"), "{invalid}");
    DavXmlElement::parse(invalid.as_bytes()).unwrap();
}

use std::io::{self, Write};

use aster_forge_xml::{
    BorrowedDocument, Error, NodeRef, OwnedDocument, ParseOptions, XmlSafetyError, XmlSafetyPolicy,
    validate_xml_input, xml_root_local_name,
};

#[test]
fn preserves_every_node_type_and_mixed_content_in_order() {
    let input = br#"<p>before<b key="a&amp;b"/><![CDATA[<raw>]]><!--note--><?work now?>after</p>"#;
    let document = BorrowedDocument::parse(input.as_slice()).expect("mixed document should parse");
    let nodes: Vec<_> = document.root().children().collect();

    assert!(matches!(nodes[0], NodeRef::Text("before")));
    assert!(matches!(nodes[1], NodeRef::Element(element) if element.name() == "b"));
    assert!(matches!(nodes[2], NodeRef::CData("<raw>")));
    assert!(matches!(nodes[3], NodeRef::Comment("note")));
    assert!(matches!(
        nodes[4],
        NodeRef::ProcessingInstruction(pi) if pi.target == "work" && pi.content == Some("now")
    ));
    assert!(matches!(nodes[5], NodeRef::Text("after")));
    assert_eq!(
        document
            .root()
            .get_child("b")
            .and_then(|b| b.attribute("key")),
        Some("a&b")
    );
}

#[test]
fn resolves_default_prefix_shadowing_and_attribute_namespaces() {
    let input = br#"<root xmlns="urn:one" xmlns:a="urn:attr" a:id="7"><child/><p:item xmlns:p="urn:two"><leaf xmlns="urn:three"/></p:item></root>"#;
    let document = BorrowedDocument::parse(input.as_slice()).expect("namespaces should parse");
    let root = document.root();

    assert_eq!(root.namespace(), Some("urn:one"));
    assert_eq!(root.attribute_ns("id", Some("urn:attr")), Some("7"));
    assert_eq!(root.attribute_ns("id", None), None);
    assert!(root.get_child_ns("child", "urn:one").is_some());
    let item = root
        .get_child_ns("item", "urn:two")
        .expect("prefixed namespace");
    assert_eq!(item.prefix(), Some("p"));
    assert!(item.get_child_ns("leaf", "urn:three").is_some());
}

#[test]
fn preserves_default_namespace_undeclaration() {
    let input = br#"<root xmlns="urn:root"><child xmlns=""><leaf/></child></root>"#;
    let document = BorrowedDocument::parse(input.as_slice()).expect("document should parse");
    let child = document.root().get_child("child").expect("child");

    assert_eq!(child.namespace(), None);
    assert_eq!(
        child.get_child("leaf").and_then(|leaf| leaf.namespace()),
        None
    );
    assert_eq!(child.raw_xml(), b"<child xmlns=\"\"><leaf/></child>");
}

#[test]
fn preserves_webdav_dead_property_and_lock_owner_subtrees_exactly() {
    let input = br#"<D:prop xmlns:D="DAV:" xmlns:x="urn:example"><x:color shade="dark">blue<em>!</em></x:color><D:lockowner><D:href>mailto:a@example.test</D:href></D:lockowner></D:prop>"#;
    let document = BorrowedDocument::parse(input.as_slice()).expect("WebDAV XML should parse");
    let root = document.root();
    let color = root
        .get_child_ns("color", "urn:example")
        .expect("dead property");
    let owner = root.get_child_ns("lockowner", "DAV:").expect("lock owner");

    assert_eq!(color.attribute("shade"), Some("dark"));
    assert_eq!(color.text().as_deref(), Some("blue"));
    assert_eq!(
        owner.raw_xml(),
        br#"<D:lockowner><D:href>mailto:a@example.test</D:href></D:lockowner>"#
    );
}

#[test]
fn validates_complete_single_root_and_declarations() {
    let policy = XmlSafetyPolicy::untrusted();
    assert_eq!(
        xml_root_local_name(br#"<D:prop xmlns:D="DAV:"/>"#, policy),
        Ok("prop".into())
    );
    for input in [
        b"<a/><b/>".as_slice(),
        b"<a/>garbage",
        b"<a><b></a>",
        b"<?xml version=\"1.0\"?><a/><?xml version=\"1.0\"?>",
    ] {
        assert_eq!(
            validate_xml_input(input, policy),
            Err(XmlSafetyError::Malformed)
        );
    }
    assert_eq!(
        validate_xml_input(b"<!DOCTYPE a [<!ENTITY x 'boom'>]><a>&x;</a>", policy),
        Err(XmlSafetyError::ExternalEntity)
    );
    assert!(validate_xml_input(b"<a><![CDATA[<!DOCTYPE harmless>]]></a>", policy).is_ok());
}

#[test]
fn applies_depth_and_element_limits_to_start_and_empty_elements() {
    let base = XmlSafetyPolicy::untrusted();
    let depth_policy = XmlSafetyPolicy {
        max_depth: 2,
        ..base
    };
    assert!(validate_xml_input(b"<a><b/></a>", depth_policy).is_ok());
    assert_eq!(
        validate_xml_input(b"<a><b><c/></b></a>", depth_policy),
        Err(XmlSafetyError::TooDeep)
    );

    let element_policy = XmlSafetyPolicy {
        max_elements: 3,
        ..base
    };
    assert!(validate_xml_input(b"<a><b><c/></b></a>", element_policy).is_ok());
    assert_eq!(
        validate_xml_input(b"<a><b/><c/><d/></a>", element_policy),
        Err(XmlSafetyError::TooManyElements)
    );
}

#[test]
fn applies_input_attribute_text_and_event_limits() {
    let base = XmlSafetyPolicy::untrusted();
    assert!(
        validate_xml_input(
            b"<root/>",
            XmlSafetyPolicy {
                max_input_bytes: 7,
                ..base
            }
        )
        .is_ok()
    );
    assert_eq!(
        validate_xml_input(
            b"<root/>",
            XmlSafetyPolicy {
                max_input_bytes: 6,
                ..base
            }
        ),
        Err(XmlSafetyError::InputTooLarge)
    );
    assert!(
        validate_xml_input(
            b"<root a='1' b='2'/>",
            XmlSafetyPolicy {
                max_attributes_per_element: 2,
                ..base
            }
        )
        .is_ok()
    );
    assert_eq!(
        validate_xml_input(
            b"<root a='1' b='2'/>",
            XmlSafetyPolicy {
                max_attributes_per_element: 1,
                ..base
            }
        ),
        Err(XmlSafetyError::TooManyAttributes)
    );
    assert!(
        validate_xml_input(
            b"<root>four</root>",
            XmlSafetyPolicy {
                max_text_bytes: 4,
                ..base
            }
        )
        .is_ok()
    );
    assert_eq!(
        validate_xml_input(
            b"<root>four</root>",
            XmlSafetyPolicy {
                max_text_bytes: 3,
                ..base
            }
        ),
        Err(XmlSafetyError::TextTooLarge)
    );
    assert!(
        validate_xml_input(
            b"<root><a/></root>",
            XmlSafetyPolicy {
                max_events: 3,
                ..base
            }
        )
        .is_ok()
    );
    assert_eq!(
        validate_xml_input(
            b"<root><a/></root>",
            XmlSafetyPolicy {
                max_events: 2,
                ..base
            }
        ),
        Err(XmlSafetyError::TooManyEvents)
    );
}

#[test]
fn rejects_zero_limits_and_invalid_utf8_in_all_textual_events() {
    let base = XmlSafetyPolicy::untrusted();
    for policy in [
        XmlSafetyPolicy {
            max_input_bytes: 0,
            ..base
        },
        XmlSafetyPolicy {
            max_depth: 0,
            ..base
        },
        XmlSafetyPolicy {
            max_elements: 0,
            ..base
        },
        XmlSafetyPolicy {
            max_attributes_per_element: 0,
            ..base
        },
        XmlSafetyPolicy {
            max_text_bytes: 0,
            ..base
        },
        XmlSafetyPolicy {
            max_events: 0,
            ..base
        },
    ] {
        assert_eq!(
            validate_xml_input(b"<root/>", policy),
            Err(XmlSafetyError::InvalidPolicy)
        );
    }
    for input in [
        b"<root>\xFF</root>".as_slice(),
        b"<!--\xFF--><root/>",
        b"<?\xFF value?><root/>",
    ] {
        assert_eq!(
            validate_xml_input(input, base),
            Err(XmlSafetyError::InvalidEncoding)
        );
    }
}

#[test]
fn owned_reader_enforces_the_exact_input_size_boundary() {
    let accepted = ParseOptions::new().max_size(7);
    assert!(OwnedDocument::from_reader_with_options(b"<root/>".as_slice(), &accepted).is_ok());

    let rejected = ParseOptions::new().max_size(6);
    assert!(matches!(
        OwnedDocument::from_reader_with_options(b"<root/>".as_slice(), &rejected),
        Err(Error::Safety(XmlSafetyError::InputTooLarge))
    ));
}

#[test]
fn rejects_invalid_or_unbound_namespace_prefixes() {
    let policy = XmlSafetyPolicy::untrusted();
    for input in [
        br#"<p:root/>"#.as_slice(),
        br#"<root p:id="1"/>"#,
        br#"<root xmlns:xml="urn:not-xml"/>"#,
        br#"<root xmlns:xmlns="urn:no"/>"#,
        br#"<root xmlns:1x="urn:no"/>"#,
        br#"<root xmlns:p=""/>"#,
    ] {
        assert_eq!(
            validate_xml_input(input, policy),
            Err(XmlSafetyError::Malformed)
        );
    }
    assert!(matches!(
        BorrowedDocument::parse(br#"<root xmlns:1x="urn:no"/>"#.as_slice()),
        Err(Error::Safety(XmlSafetyError::Malformed))
    ));
}

#[test]
fn arena_and_validator_classify_invalid_encoding_consistently() {
    let input = b"<root>\xFF</root>";

    assert_eq!(
        validate_xml_input(input, XmlSafetyPolicy::untrusted()),
        Err(XmlSafetyError::InvalidEncoding)
    );
    assert!(matches!(
        BorrowedDocument::parse(input.as_slice()),
        Err(Error::Safety(XmlSafetyError::InvalidEncoding))
    ));
}

#[test]
fn parses_and_drops_deep_trees_without_recursive_walkers() {
    const DEPTH: usize = 20_000;
    let mut input = "<n>".repeat(DEPTH);
    input.push_str(&"</n>".repeat(DEPTH));
    let options = ParseOptions::new()
        .max_size(input.len())
        .max_depth(DEPTH)
        .max_elements(DEPTH)
        .max_events(DEPTH * 2 + 1);
    let document = BorrowedDocument::parse_with_options(input.as_bytes(), &options)
        .expect("deep tree should parse");

    assert_eq!(document.node_count(), DEPTH);
    drop(document);
}

#[test]
fn trim_whitespace_is_explicit_and_source_backed_when_possible() {
    let input = b"<root>  before <child/> after  </root>";
    let document = BorrowedDocument::parse_with_options(
        input.as_slice(),
        &ParseOptions::new().trim_whitespace(true),
    )
    .expect("trimmed document");
    let nodes: Vec<_> = document.root().children().collect();

    assert!(matches!(nodes[0], NodeRef::Text("before")));
    assert!(matches!(nodes[2], NodeRef::Text("after")));
    assert_eq!(document.allocated_value_count(), 0);
}

#[test]
fn trim_whitespace_counts_original_decoded_text_against_the_limit() {
    let input = b"<root>  x  </root>";
    let policy = XmlSafetyPolicy {
        max_text_bytes: 4,
        ..XmlSafetyPolicy::untrusted()
    };
    let options = ParseOptions::new()
        .safety_policy(policy)
        .trim_whitespace(true);

    assert_eq!(
        validate_xml_input(input, policy),
        Err(XmlSafetyError::TextTooLarge)
    );
    assert!(matches!(
        BorrowedDocument::parse_with_options(input.as_slice(), &options),
        Err(Error::Safety(XmlSafetyError::TextTooLarge))
    ));
}

#[test]
fn original_writer_propagates_io_errors() {
    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("fixture failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let document = BorrowedDocument::parse(b"<root/>".as_slice()).expect("document");
    assert!(matches!(
        document.write_original(FailingWriter),
        Err(Error::Io(_))
    ));
}

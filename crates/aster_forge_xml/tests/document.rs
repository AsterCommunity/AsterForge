use std::sync::Arc;

use aster_forge_xml::{
    BorrowedDocument, NodeRef, ParseOptions, ValidatedXml, XmlDocument, XmlSafetyError,
};

fn inside(source: &[u8], value: &[u8]) -> bool {
    let source_start = source.as_ptr() as usize;
    let source_end = source_start + source.len();
    let value_start = value.as_ptr() as usize;
    let value_end = value_start + value.len();
    value_start >= source_start && value_end <= source_end
}

#[test]
fn plain_document_values_are_source_backed() {
    let source = br#"<D:root xmlns:D="DAV:" plain="value"><D:child>text</D:child></D:root>"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("document should parse");
    let root = document.root();
    let attribute = root.attributes().next().expect("plain attribute");
    let child = root.get_child_ns("child", "DAV:").expect("DAV child");
    let text = child.text().expect("child text");

    assert_eq!(document.allocated_value_count(), 0);
    assert!(inside(source, root.qualified_name().as_bytes()));
    assert!(inside(source, attribute.qualified_name().as_bytes()));
    assert!(inside(source, attribute.value().as_bytes()));
    assert!(inside(source, text.as_bytes()));
}

#[test]
fn decoded_entities_use_the_owned_value_pool_only_when_needed() {
    let source = br#"<root plain="value" escaped="a&amp;b">before&amp;after</root>"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("document should parse");
    let root = document.root();

    assert_eq!(root.attribute("plain"), Some("value"));
    assert_eq!(root.attribute("escaped"), Some("a&b"));
    assert_eq!(root.text().as_deref(), Some("before&after"));
    assert!(document.allocated_value_count() >= 2);
}

#[test]
fn namespace_scopes_shadow_and_undeclare_without_per_element_maps() {
    let source = br#"<root xmlns="urn:one" xmlns:p="urn:p1"><child><p:item xmlns:p="urn:p2"/><plain xmlns=""/></child></root>"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("document should parse");
    let root = document.root();
    let child = root
        .get_child_ns("child", "urn:one")
        .expect("default namespace child");
    let item = child
        .get_child_ns("item", "urn:p2")
        .expect("shadowed prefix");
    let plain = child.get_child("plain").expect("unqualified child");

    assert_eq!(root.namespace(), Some("urn:one"));
    assert_eq!(item.prefix(), Some("p"));
    assert_eq!(plain.namespace(), None);
}

#[test]
fn attributes_follow_xml_namespace_rules() {
    let source =
        br#"<root xmlns="urn:default" xmlns:p="urn:attrs" plain="a" p:value="b" xml:lang="en"/>"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("document should parse");
    let root = document.root();

    assert_eq!(root.attribute_ns("plain", None), Some("a"));
    assert_eq!(root.attribute_ns("value", Some("urn:attrs")), Some("b"));
    assert_eq!(
        root.attribute_ns("lang", Some("http://www.w3.org/XML/1998/namespace")),
        Some("en")
    );
}

#[test]
fn subtree_raw_xml_is_an_exact_source_slice() {
    let source = br#"<?xml version="1.0"?><root><owner><href>a&amp;b</href><!-- exact --></owner><tail/></root>"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("document should parse");
    let owner = document.root().get_child("owner").expect("owner subtree");

    assert_eq!(
        owner.raw_xml(),
        br#"<owner><href>a&amp;b</href><!-- exact --></owner>"#
    );
    assert!(inside(source, owner.raw_xml()));
}

#[test]
fn ordered_nodes_and_parent_links_are_preserved() {
    let source = br#"<root>before<child/><![CDATA[raw]]><!--note--><?work now?>after</root>"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("document should parse");
    let root = document.root();
    let nodes: Vec<_> = root.children().collect();

    assert!(matches!(nodes[0], NodeRef::Text("before")));
    assert!(matches!(nodes[1], NodeRef::Element(element) if element.name() == "child"));
    assert!(matches!(nodes[2], NodeRef::CData("raw")));
    assert!(matches!(nodes[3], NodeRef::Comment("note")));
    assert!(matches!(
        nodes[4],
        NodeRef::ProcessingInstruction(pi) if pi.target == "work" && pi.content == Some("now")
    ));
    assert!(matches!(nodes[5], NodeRef::Text("after")));
    let child = root.get_child("child").expect("child");
    assert_eq!(child.parent().map(|parent| parent.id()), Some(root.id()));
}

#[test]
fn validated_xml_owns_exact_bytes_and_clones_cheaply() {
    let source = br#"<owner xmlns="DAV:"><href>mailto:a@example.test</href></owner>"#.to_vec();
    let validated = ValidatedXml::new(source.clone()).expect("validated XML");
    let cloned = validated.clone();

    assert_eq!(validated.as_bytes(), source);
    assert_eq!(cloned.as_bytes(), source);
    assert_eq!(validated.document().root().namespace(), Some("DAV:"));
}

#[test]
fn original_write_preserves_the_complete_document() {
    let source = br#"<?xml version="1.0"?><!--before--><root><child/></root><!--after-->"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("document should parse");
    let mut output = Vec::new();

    document
        .write_original(&mut output)
        .expect("document should write");

    assert_eq!(output, source);
    assert_eq!(document.root().raw_xml(), b"<root><child/></root>");
}

#[test]
fn generic_owned_source_does_not_require_self_references() {
    let source: Arc<[u8]> = Arc::from(br#"<root><child/></root>"#.as_slice());
    let document = XmlDocument::parse(Arc::clone(&source)).expect("owned document");
    drop(source);

    assert_eq!(
        document.root().get_child("child").map(|child| child.name()),
        Some("child")
    );
}

#[test]
fn deep_arena_document_drops_without_tree_recursion() {
    const DEPTH: usize = 20_000;
    let mut source = "<n>".repeat(DEPTH);
    source.push_str(&"</n>".repeat(DEPTH));
    let options = ParseOptions::new()
        .max_size(source.len())
        .max_depth(DEPTH)
        .max_elements(DEPTH)
        .max_events(DEPTH * 2 + 1);
    let document = BorrowedDocument::parse_with_options(source.as_bytes(), &options)
        .expect("deep arena document");

    assert_eq!(document.node_count(), DEPTH);
    drop(document);
}

#[test]
fn arena_parser_keeps_safety_error_classification() {
    let error = BorrowedDocument::parse(b"<a/><b/>".as_slice()).expect_err("second root");
    assert!(matches!(
        error,
        aster_forge_xml::Error::Safety(XmlSafetyError::Malformed)
    ));
}

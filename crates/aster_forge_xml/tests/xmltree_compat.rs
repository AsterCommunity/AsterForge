use aster_forge_xml::{BorrowedDocument, ElementRef, NodeRef};
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug, PartialEq, Eq)]
enum NodeSnapshot {
    Element(ElementSnapshot),
    Text(String),
    CData(String),
    Comment(String),
    ProcessingInstruction(String, Option<String>),
}

#[derive(Debug, PartialEq, Eq)]
struct ElementSnapshot {
    prefix: Option<String>,
    namespace: Option<String>,
    name: String,
    attributes: std::collections::BTreeMap<String, String>,
    children: Vec<NodeSnapshot>,
}

fn forge_snapshot<S: AsRef<[u8]>>(element: ElementRef<'_, S>) -> ElementSnapshot {
    let mut children = Vec::new();
    for node in element.children() {
        let snapshot = match node {
            NodeRef::Element(element) => NodeSnapshot::Element(forge_snapshot(element)),
            NodeRef::Text(text) => NodeSnapshot::Text(text.to_owned()),
            NodeRef::CData(text) => NodeSnapshot::CData(text.to_owned()),
            NodeRef::Comment(text) => NodeSnapshot::Comment(text.to_owned()),
            NodeRef::ProcessingInstruction(pi) => NodeSnapshot::ProcessingInstruction(
                pi.target.to_owned(),
                pi.content.map(str::to_owned),
            ),
        };
        if let (Some(NodeSnapshot::Text(previous)), NodeSnapshot::Text(text)) =
            (children.last_mut(), &snapshot)
        {
            previous.push_str(text);
        } else {
            children.push(snapshot);
        }
    }
    ElementSnapshot {
        prefix: element.prefix().map(str::to_owned),
        namespace: element.namespace().map(str::to_owned),
        name: element.name().to_owned(),
        attributes: element
            .attributes()
            .map(|attribute| {
                (
                    attribute.qualified_name().to_owned(),
                    attribute.value().to_owned(),
                )
            })
            .collect(),
        children,
    }
}

fn xmltree_snapshot(element: &xmltree::Element) -> ElementSnapshot {
    ElementSnapshot {
        prefix: element.prefix.clone(),
        namespace: element.namespace.clone(),
        name: element.name.clone(),
        attributes: element
            .attributes
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        children: element
            .children
            .iter()
            .map(|node| match node {
                xmltree::XMLNode::Element(element) => {
                    NodeSnapshot::Element(xmltree_snapshot(element))
                }
                xmltree::XMLNode::Text(text) => NodeSnapshot::Text(text.clone()),
                xmltree::XMLNode::CData(text) => NodeSnapshot::CData(text.clone()),
                xmltree::XMLNode::Comment(text) => NodeSnapshot::Comment(text.clone()),
                xmltree::XMLNode::ProcessingInstruction(target, content) => {
                    NodeSnapshot::ProcessingInstruction(target.clone(), content.clone())
                }
            })
            .collect(),
    }
}

fn inside(source: &[u8], slice: &[u8]) -> bool {
    let source_start = source.as_ptr() as usize;
    let source_end = source_start + source.len();
    let slice_start = slice.as_ptr() as usize;
    let slice_end = slice_start + slice.len();
    slice_start >= source_start && slice_end <= source_end
}

#[test]
fn quick_xml_slice_events_borrow_plain_names_attributes_and_text() {
    let source = br#"<root plain="value">text</root>"#;
    let mut reader = Reader::from_reader(source.as_slice());

    let Event::Start(start) = reader.read_event().expect("start event") else {
        panic!("expected start event");
    };
    assert!(inside(source, start.name().as_ref()));
    let attribute = start
        .attributes()
        .next()
        .expect("attribute")
        .expect("valid attribute");
    assert!(inside(source, attribute.key.as_ref()));
    assert!(inside(source, attribute.value.as_ref()));

    let Event::Text(text) = reader.read_event().expect("text event") else {
        panic!("expected text event");
    };
    assert!(inside(source, text.as_ref()));
}

#[test]
fn forge_arena_borrows_plain_values_from_the_source() {
    let source = br#"<root plain="value">text</root>"#;
    let document = BorrowedDocument::parse(source.as_slice()).expect("fixture should parse");
    let root = document.root();

    assert!(inside(source, root.qualified_name().as_bytes()));
    assert!(inside(
        source,
        root.attribute("plain").expect("attribute").as_bytes()
    ));
    assert!(inside(source, root.text().expect("text").as_bytes()));
    assert_eq!(document.allocated_value_count(), 0);
}

#[test]
fn forge_and_xmltree_preserve_the_same_ordered_node_kinds() {
    let source = br#"<root>before<child/><![CDATA[raw]]><!--note--><?work now?>after</root>"#;
    let forge = BorrowedDocument::parse(source.as_slice()).expect("Forge fixture");
    let xmltree = xmltree::Element::parse(source.as_slice()).expect("xmltree fixture");

    let forge_kinds: Vec<&str> = forge
        .root()
        .children()
        .map(|node| match node {
            NodeRef::Element(_) => "element",
            NodeRef::Text(_) => "text",
            NodeRef::CData(_) => "cdata",
            NodeRef::Comment(_) => "comment",
            NodeRef::ProcessingInstruction(_) => "pi",
        })
        .collect();
    let xmltree_kinds: Vec<&str> = xmltree
        .children
        .iter()
        .map(|node| match node {
            xmltree::XMLNode::Element(_) => "element",
            xmltree::XMLNode::Text(_) => "text",
            xmltree::XMLNode::CData(_) => "cdata",
            xmltree::XMLNode::Comment(_) => "comment",
            xmltree::XMLNode::ProcessingInstruction(_, _) => "pi",
        })
        .collect();
    assert_eq!(forge_kinds, xmltree_kinds);
}

#[test]
fn forge_and_xmltree_resolve_element_namespaces_consistently() {
    let source = br#"<D:root xmlns:D="DAV:" xmlns:x="urn:example"><x:item/><D:item/></D:root>"#;
    let forge = BorrowedDocument::parse(source.as_slice()).expect("Forge fixture");
    let xmltree = xmltree::Element::parse(source.as_slice()).expect("xmltree fixture");

    assert_eq!(forge.root().namespace(), xmltree.namespace.as_deref());
    assert!(forge.root().get_child_ns("item", "urn:example").is_some());
    assert!(xmltree.get_child(("item", "urn:example")).is_some());
    assert!(forge.root().get_child_ns("item", "DAV:").is_some());
    assert!(xmltree.get_child(("item", "DAV:")).is_some());
}

#[test]
fn forge_matches_xmltree_for_the_supported_parse_contract() {
    for source in [
        br#"<root/>"#.as_slice(),
        br#"<root id="7" enabled="true">plain &amp; escaped</root>"#,
        br#"<root>before<child key="value"/>after</root>"#,
        br#"<D:root xmlns:D="DAV:"><D:item><leaf xmlns="urn:leaf"/></D:item></D:root>"#,
        br#"<root><![CDATA[<raw>]]><!--note--><?work now?></root>"#,
        "<root>中文 日本語 한국어</root>".as_bytes(),
    ] {
        let forge = BorrowedDocument::parse(source).expect("Forge fixture");
        let xmltree = xmltree::Element::parse(source).expect("xmltree fixture");
        assert_eq!(forge_snapshot(forge.root()), xmltree_snapshot(&xmltree));
    }
}

#[test]
fn forge_intentionally_preserves_element_only_whitespace_that_xmltree_drops() {
    let source = b"<root>\n  <child/>\n</root>";
    let forge = BorrowedDocument::parse(source.as_slice()).expect("Forge fixture");
    let xmltree = xmltree::Element::parse(source.as_slice()).expect("xmltree fixture");
    let nodes: Vec<_> = forge.root().children().collect();

    assert!(matches!(nodes[0], NodeRef::Text("\n  ")));
    assert_eq!(xmltree.children.len(), 1);
}

#[test]
fn exact_forge_output_and_xmltree_output_round_trip_through_each_other() {
    let source = br#"<D:root xmlns:D="DAV:"><D:item id="7">before<leaf/><![CDATA[raw]]><!--note--><?work now?>after</D:item></D:root>"#;
    let forge = BorrowedDocument::parse(source.as_slice()).expect("Forge fixture");
    let expected = forge_snapshot(forge.root());
    let mut forge_output = Vec::new();
    forge
        .write_original(&mut forge_output)
        .expect("Forge exact output");
    let xmltree_from_forge =
        xmltree::Element::parse(forge_output.as_slice()).expect("xmltree parses Forge output");
    assert_eq!(expected, xmltree_snapshot(&xmltree_from_forge));

    let xmltree = xmltree::Element::parse(source.as_slice()).expect("xmltree fixture");
    let mut options = xmltree::EmitterConfig::new();
    options.perform_indent = false;
    options.write_document_declaration = false;
    options.pad_self_closing = false;
    options.autopad_comments = false;
    let mut xmltree_output = Vec::new();
    xmltree
        .write_with_config(&mut xmltree_output, options)
        .expect("xmltree output");
    let forge_from_xmltree =
        BorrowedDocument::parse(xmltree_output.as_slice()).expect("Forge parses xmltree output");
    assert_eq!(
        xmltree_snapshot(&xmltree),
        forge_snapshot(forge_from_xmltree.root())
    );
}

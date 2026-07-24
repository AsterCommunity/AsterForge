use std::collections::BTreeMap;

use aster_forge_xml::{BorrowedDocument, ElementRef};

#[derive(Debug, PartialEq, Eq)]
struct ElementSnapshot {
    local_name: String,
    namespace: Option<String>,
    attributes: BTreeMap<(Option<String>, String), String>,
    direct_text: String,
    children: Vec<ElementSnapshot>,
}

fn forge_snapshot(element: ElementRef<'_, &[u8]>) -> ElementSnapshot {
    ElementSnapshot {
        local_name: element.name().to_owned(),
        namespace: element.namespace().map(str::to_owned),
        attributes: element
            .attributes()
            .map(|attribute| {
                (
                    (
                        attribute.namespace().map(str::to_owned),
                        attribute.name().to_owned(),
                    ),
                    attribute.value().to_owned(),
                )
            })
            .collect(),
        direct_text: element.text().unwrap_or_default().into_owned(),
        children: element.child_elements().map(forge_snapshot).collect(),
    }
}

fn roxmltree_snapshot(node: roxmltree::Node<'_, '_>) -> ElementSnapshot {
    ElementSnapshot {
        local_name: node.tag_name().name().to_owned(),
        namespace: node
            .tag_name()
            .namespace()
            .filter(|namespace| !namespace.is_empty())
            .map(str::to_owned),
        attributes: node
            .attributes()
            .map(|attribute| {
                (
                    (
                        attribute
                            .namespace()
                            .filter(|namespace| !namespace.is_empty())
                            .map(str::to_owned),
                        attribute.name().to_owned(),
                    ),
                    attribute.value().to_owned(),
                )
            })
            .collect(),
        direct_text: node
            .children()
            .filter(roxmltree::Node::is_text)
            .filter_map(|child| child.text())
            .collect::<String>(),
        children: node
            .children()
            .filter(roxmltree::Node::is_element)
            .map(roxmltree_snapshot)
            .collect(),
    }
}

#[test]
fn forge_matches_roxmltree_for_source_backed_tree_queries() {
    for source in [
        br#"<root/>"#.as_slice(),
        br#"<root a="1">before<child/>after</root>"#,
        br#"<D:prop xmlns:D="DAV:" xmlns:x="urn:x" x:id="7"><x:value>a&amp;b<![CDATA[<raw>]]></x:value></D:prop>"#,
        br#"<root xmlns="urn:one"><child xmlns=""><leaf/></child><p:item xmlns:p="urn:two"/></root>"#,
    ] {
        let forge = BorrowedDocument::parse(source).expect("Forge fixture");
        let text = std::str::from_utf8(source).expect("UTF-8 fixture");
        let roxmltree = roxmltree::Document::parse(text).expect("roxmltree fixture");

        assert_eq!(
            forge_snapshot(forge.root()),
            roxmltree_snapshot(roxmltree.root_element())
        );
    }
}

#[test]
fn forge_and_roxmltree_expose_the_same_exact_root_range() {
    let source = br#"<?xml version="1.0"?><!--before--><D:root xmlns:D="DAV:"><D:child a="1">text</D:child></D:root><!--after-->"#;
    let forge = BorrowedDocument::parse(source.as_slice()).expect("Forge fixture");
    let text = std::str::from_utf8(source).expect("UTF-8 fixture");
    let roxmltree = roxmltree::Document::parse(text).expect("roxmltree fixture");
    let range = roxmltree.root_element().range();

    assert_eq!(forge.root().raw_xml(), &source[range]);
}

#[test]
fn roxmltree_comparison_keeps_intentional_node_model_differences_explicit() {
    let source = br#"<root>text<![CDATA[cdata]]><!--comment--><?work now?></root>"#;
    let forge = BorrowedDocument::parse(source.as_slice()).expect("Forge fixture");
    let text = std::str::from_utf8(source).expect("UTF-8 fixture");
    let roxmltree = roxmltree::Document::parse(text).expect("roxmltree fixture");

    assert_eq!(forge.root().text().as_deref(), Some("textcdata"));
    assert_eq!(roxmltree.root_element().text(), Some("textcdata"));
    assert_eq!(
        roxmltree
            .root_element()
            .children()
            .filter(roxmltree::Node::is_text)
            .filter_map(|node| node.text())
            .collect::<String>(),
        "textcdata"
    );
    assert_eq!(forge.root().children().count(), 4);
    assert_eq!(roxmltree.root_element().children().count(), 3);
}

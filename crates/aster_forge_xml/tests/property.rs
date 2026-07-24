use std::collections::BTreeMap;

use aster_forge_xml::{
    BorrowedDocument, ElementRef, ParseOptions, XmlSafetyPolicy, XmlStreamEvent, XmlStreamReader,
    XmlStreamWriter, validate_xml_input,
};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use proptest::string::string_regex;

#[derive(Clone, Debug)]
struct ElementModel {
    name: String,
    attributes: BTreeMap<String, String>,
    text: String,
    children: Vec<ElementModel>,
}

fn xml_name() -> impl Strategy<Value = String> {
    string_regex("[a-z][a-z0-9]{0,7}").expect("static name regex")
}

fn xml_text() -> impl Strategy<Value = String> {
    vec(0u8..=16, 0..32).prop_map(|values| {
        values
            .into_iter()
            .map(|value| match value {
                0 => '&',
                1 => '<',
                2 => '>',
                3 => '\'',
                4 => '"',
                5 => '\n',
                6 => '\t',
                7 => '中',
                _ => char::from(b'a' + (value % 10)),
            })
            .collect()
    })
}

fn element_model() -> impl Strategy<Value = ElementModel> {
    (
        xml_name(),
        btree_map(xml_name(), xml_text(), 0..4),
        xml_text(),
    )
        .prop_map(|(name, attributes, text)| ElementModel {
            name,
            attributes,
            text,
            children: Vec::new(),
        })
        .prop_recursive(4, 96, 4, |children| {
            (
                xml_name(),
                btree_map(xml_name(), xml_text(), 0..4),
                xml_text(),
                vec(children, 0..4),
            )
                .prop_map(|(name, attributes, text, children)| ElementModel {
                    name,
                    attributes,
                    text,
                    children,
                })
        })
}

fn write_model(writer: &mut XmlStreamWriter<Vec<u8>>, model: &ElementModel) {
    writer
        .start_element(
            &model.name,
            model
                .attributes
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .expect("generated element is valid");
    writer.text(&model.text).expect("generated text is valid");
    for child in &model.children {
        write_model(writer, child);
    }
    writer.end_element().expect("generated element end");
}

fn assert_forge_model(element: ElementRef<'_, &[u8]>, model: &ElementModel) {
    assert_eq!(element.name(), model.name);
    assert_eq!(element.text().unwrap_or_default(), model.text);
    assert_eq!(element.attributes().count(), model.attributes.len());
    for (name, value) in &model.attributes {
        let normalized = normalize_attribute_value(value);
        assert_eq!(element.attribute(name), Some(normalized.as_str()));
    }
    let actual_children: Vec<_> = element.child_elements().collect();
    assert_eq!(actual_children.len(), model.children.len());
    for (actual, expected) in actual_children.into_iter().zip(&model.children) {
        assert_forge_model(actual, expected);
    }
}

fn assert_roxmltree_model(node: roxmltree::Node<'_, '_>, model: &ElementModel) {
    assert_eq!(node.tag_name().name(), model.name);
    let direct_text = node
        .children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|child| child.text())
        .collect::<String>();
    assert_eq!(direct_text, model.text);
    assert_eq!(node.attributes().len(), model.attributes.len());
    for (name, value) in &model.attributes {
        let normalized = normalize_attribute_value(value);
        assert_eq!(node.attribute(name.as_str()), Some(normalized.as_str()));
    }
    let actual_children: Vec<_> = node
        .children()
        .filter(roxmltree::Node::is_element)
        .collect();
    assert_eq!(actual_children.len(), model.children.len());
    for (actual, expected) in actual_children.into_iter().zip(&model.children) {
        assert_roxmltree_model(actual, expected);
    }
}

fn normalize_attribute_value(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            character => character,
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn streaming_writer_round_trips_generated_trees(model in element_model()) {
        let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
        write_model(&mut writer, &model);
        let output = writer.finish().expect("complete generated document");

        validate_xml_input(&output, XmlSafetyPolicy::untrusted()).expect("generated document validates");
        let forge = BorrowedDocument::parse(output.as_slice()).expect("Forge reparses generated document");
        assert_forge_model(forge.root(), &model);
        let text = std::str::from_utf8(&output).expect("writer emits UTF-8");
        let roxmltree = roxmltree::Document::parse(text).expect("roxmltree reparses generated document");
        assert_roxmltree_model(roxmltree.root_element(), &model);
    }

    #[test]
    fn arbitrary_byte_inputs_remain_bounded_and_never_panic(input in vec(any::<u8>(), 0..4096)) {
        let max_input_bytes = input.len().max(1);
        let policy = XmlSafetyPolicy {
            max_input_bytes,
            max_depth: 64,
            max_elements: 4096,
            max_attributes_per_element: 128,
            max_text_bytes: max_input_bytes,
            max_events: 16_384,
            reject_doctype: true,
        };
        let _ = validate_xml_input(&input, policy);
        let options = ParseOptions::new().safety_policy(policy);
        let _ = BorrowedDocument::parse_with_options(input.as_slice(), &options);

        let mut reader = XmlStreamReader::new(input.as_slice(), policy).expect("valid policy");
        for _ in 0..=policy.max_events {
            match reader.read_event() {
                Ok(XmlStreamEvent::Eof) | Err(_) => break,
                Ok(_) => {}
            }
        }
    }
}

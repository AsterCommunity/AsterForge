use std::io::{self, Write};

use aster_forge_xml::{
    BorrowedDocument, Error, ValidatedXml, XmlSafetyError, XmlStreamWriter, XmlWriteAttribute,
    XmlWriteOptions,
};

fn finish(writer: XmlStreamWriter<Vec<u8>>) -> Vec<u8> {
    writer.finish().expect("writer should finish")
}

fn assert_invalid_data(result: Result<(), Error>) {
    assert!(matches!(result, Err(Error::InvalidData(_))));
}

#[test]
fn writes_namespace_aware_webdav_multistatus_and_reparses() {
    let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    writer
        .start_element("D:multistatus", [("xmlns:D", "DAV:")])
        .expect("root");
    writer.start("D:response").expect("response");
    writer.start("D:href").expect("href");
    writer.text("/files/a&b").expect("text");
    writer.end_element().expect("href end");
    writer
        .empty_element(
            "x:color",
            [
                XmlWriteAttribute::new("xmlns:x", "urn:property"),
                XmlWriteAttribute::new("x:shade", "dark"),
            ],
        )
        .expect("extension property");
    writer.end_element().expect("response end");
    writer.end_element().expect("root end");

    let output = finish(writer);
    assert_eq!(
        output,
        br#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/files/a&amp;b</D:href><x:color xmlns:x="urn:property" x:shade="dark"/></D:response></D:multistatus>"#
    );
    let document = BorrowedDocument::parse(output.as_slice()).expect("Forge reparses output");
    assert_eq!(document.root().namespace(), Some("DAV:"));
    xmltree::Element::parse(output.as_slice()).expect("xmltree reparses output");
}

#[test]
fn supports_inherited_prefix_default_namespace_and_undeclaration() {
    let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    writer
        .start_element("root", [("xmlns", "urn:default"), ("xmlns:p", "urn:p")])
        .expect("root");
    writer
        .start_element("p:child", [("p:id", "7")])
        .expect("prefixed child");
    writer
        .empty_element("plain", [("xmlns", "")])
        .expect("default namespace undeclaration");
    writer.end_element().expect("child end");
    writer.end_element().expect("root end");

    let output = finish(writer);
    let document = BorrowedDocument::parse(output.as_slice()).expect("parse output");
    assert_eq!(document.root().namespace(), Some("urn:default"));
    let child = document
        .root()
        .get_child_ns("child", "urn:p")
        .expect("child");
    assert_eq!(child.attribute_ns("id", Some("urn:p")), Some("7"));
    let plain = child.get_child("plain").expect("plain");
    assert_eq!(plain.namespace(), None);
}

#[test]
fn escapes_attribute_and_text_values_once() {
    let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    writer
        .start_element("root", [("value", "&<>\"'")])
        .expect("root");
    writer.text("&<>\"'").expect("text");
    writer.end_element().expect("root end");
    let output = finish(writer);

    assert_eq!(
        output,
        br#"<root value="&amp;&lt;&gt;&quot;&apos;">&amp;&lt;&gt;&quot;&apos;</root>"#
    );
    let document = BorrowedDocument::parse(output.as_slice()).expect("parse output");
    assert_eq!(document.root().attribute("value"), Some("&<>\"'"));
    assert_eq!(document.root().text().as_deref(), Some("&<>\"'"));
}

#[test]
fn writes_declaration_cdata_comment_processing_instruction_and_empty_root() {
    let options = XmlWriteOptions::new().write_document_declaration(true);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    writer.comment("before").expect("comment");
    writer
        .processing_instruction("build", Some("id=7"))
        .expect("processing instruction");
    writer.start("root").expect("root");
    writer.cdata("a < b & c").expect("CDATA");
    writer.comment("inside").expect("comment");
    writer.end_element().expect("root end");
    writer.comment("after").expect("comment");

    let output = finish(writer);
    assert_eq!(
        output,
        br#"<?xml version="1.0" encoding="UTF-8"?><!--before--><?build id=7?><root><![CDATA[a < b & c]]><!--inside--></root><!--after-->"#
    );
    BorrowedDocument::parse(output.as_slice()).expect("parse output");

    let mut empty = XmlStreamWriter::new(Vec::new()).expect("writer");
    empty.empty("root").expect("empty root");
    assert_eq!(finish(empty), b"<root/>");
}

#[test]
fn rejects_invalid_document_state_namespaces_names_and_values() {
    let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    assert_invalid_data(writer.text("outside"));
    assert_invalid_data(writer.cdata("outside"));
    assert_invalid_data(writer.end_element());
    assert_invalid_data(writer.start("p:root"));
    assert_invalid_data(writer.start("1root"));
    assert_invalid_data(writer.start("a:b:c"));
    assert_invalid_data(writer.start_element("root", [("p:id", "7")]));
    assert_invalid_data(writer.start_element("root", [("xmlns:xml", "urn:wrong")]));
    assert_invalid_data(writer.start_element("root", [("xmlns:xmlns", "urn:x")]));
    assert_invalid_data(writer.start_element("root", [("xmlns:p", "")]));
    assert_invalid_data(writer.start_element(
        "root",
        [("xmlns:p", "http://www.w3.org/XML/1998/namespace")],
    ));
    assert_invalid_data(writer.start_element("root", [("xmlns", "http://www.w3.org/2000/xmlns/")]));
    assert_invalid_data(writer.start_element("root", [("x", "\u{0}")]));

    writer.start("root").expect("valid root after failures");
    assert_invalid_data(writer.comment("a--b"));
    assert_invalid_data(writer.comment("trailing-"));
    assert_invalid_data(writer.cdata("a]]>b"));
    assert_invalid_data(writer.processing_instruction("xml", None));
    assert_invalid_data(writer.processing_instruction("p:target", None));
    assert_invalid_data(writer.processing_instruction("target", Some("a?>b")));
    assert_invalid_data(writer.text("\u{1}"));
    writer.end_element().expect("root end");
    assert_invalid_data(writer.empty("second"));
}

#[test]
fn rejects_duplicate_attributes_and_mismatched_writer_lifecycle() {
    let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    assert_invalid_data(writer.start_element("root", [("id", "1"), ("id", "2")]));
    assert_invalid_data(writer.start_element(
        "root",
        [
            ("xmlns:a", "urn:same"),
            ("xmlns:b", "urn:same"),
            ("a:id", "1"),
            ("b:id", "2"),
        ],
    ));
    writer.start("root").expect("root");
    assert!(matches!(writer.finish(), Err(Error::InvalidData(_))));

    let writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    assert!(matches!(writer.finish(), Err(Error::InvalidData(_))));
}

#[test]
fn enforces_exact_depth_attribute_and_output_boundaries() {
    let options = XmlWriteOptions::new().max_depth(2);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    writer.start("root").expect("depth one");
    writer.start("child").expect("depth two");
    assert!(matches!(
        writer.start("too-deep"),
        Err(Error::Safety(XmlSafetyError::TooDeep))
    ));
    writer.end_element().expect("child end");
    writer.end_element().expect("root end");
    finish(writer);

    let options = XmlWriteOptions::new().max_attributes_per_element(2);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    writer
        .empty_element("root", [("a", "1"), ("b", "2")])
        .expect("exact attribute limit");
    finish(writer);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    assert!(matches!(
        writer.empty_element("root", [("a", "1"), ("b", "2"), ("c", "3")]),
        Err(Error::Safety(XmlSafetyError::TooManyAttributes))
    ));

    let exact = b"<root>123</root>";
    let options = XmlWriteOptions::new().max_output_bytes(exact.len());
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    writer.start("root").expect("root");
    writer.text("123").expect("text");
    writer.end_element().expect("root end");
    assert_eq!(finish(writer), exact);

    let options = XmlWriteOptions::new().max_output_bytes(exact.len() - 1);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    writer.start("root").expect("root");
    writer.text("123").expect("text");
    assert!(matches!(
        writer.end_element(),
        Err(Error::Safety(XmlSafetyError::OutputTooLarge))
    ));
}

#[test]
fn embeds_validated_subtree_without_building_an_output_dom() {
    let subtree = ValidatedXml::new(
        br#"<x:color xmlns:x="urn:property" shade="dark">blue</x:color>"#.to_vec(),
    )
    .expect("subtree");
    let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    assert_invalid_data(writer.validated_subtree(&subtree));
    writer.start("root").expect("root");
    writer.validated_subtree(&subtree).expect("subtree");
    writer.end_element().expect("root end");

    let output = finish(writer);
    let document = BorrowedDocument::parse(output.as_slice()).expect("parse output");
    let color = document
        .root()
        .get_child_ns("color", "urn:property")
        .expect("embedded subtree");
    assert_eq!(color.text().as_deref(), Some("blue"));
}

#[test]
fn embedded_subtree_obeys_writer_depth_and_attribute_limits() {
    let deep = ValidatedXml::new(b"<a><b/></a>".to_vec()).expect("subtree");
    let options = XmlWriteOptions::new().max_depth(2);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    writer.start("root").expect("root");
    assert!(matches!(
        writer.validated_subtree(&deep),
        Err(Error::Safety(XmlSafetyError::TooDeep))
    ));

    let attributed = ValidatedXml::new(b"<a x='1' y='2'/>".to_vec()).expect("subtree");
    let options = XmlWriteOptions::new().max_attributes_per_element(1);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("writer");
    writer.start("root").expect("root");
    assert!(matches!(
        writer.validated_subtree(&attributed),
        Err(Error::Safety(XmlSafetyError::TooManyAttributes))
    ));
}

struct AlwaysFails;

impl Write for AlwaysFails {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("fixture failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("fixture failure"))
    }
}

#[test]
fn preserves_underlying_io_failures() {
    let mut writer = XmlStreamWriter::new(AlwaysFails).expect("writer construction");
    assert!(matches!(writer.empty("root"), Err(Error::Io(_))));

    struct FlushFails(Vec<u8>);
    impl Write for FlushFails {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failure"))
        }
    }

    let mut writer = XmlStreamWriter::new(FlushFails(Vec::new())).expect("writer");
    writer.empty("root").expect("root");
    assert!(matches!(writer.finish(), Err(Error::Io(_))));
}

#[test]
fn writes_large_response_directly() {
    let responses = 25_000usize;
    let mut writer = XmlStreamWriter::new(Vec::new()).expect("writer");
    writer
        .start_element("D:multistatus", [("xmlns:D", "DAV:")])
        .expect("root");
    for index in 0..responses {
        let href = format!("/files/{index}");
        writer.start("D:response").expect("response");
        writer.start("D:href").expect("href");
        writer.text(&href).expect("href text");
        writer.end_element().expect("href end");
        writer.end_element().expect("response end");
    }
    writer.end_element().expect("root end");
    let output = finish(writer);

    let document = BorrowedDocument::parse(output.as_slice()).expect("parse large output");
    assert_eq!(document.root().child_elements().count(), responses);
}

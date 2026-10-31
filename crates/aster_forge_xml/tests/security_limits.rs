use aster_forge_xml::{
    BorrowedDocument, XmlSafetyPolicy, XmlStreamEvent, XmlStreamReader, XmlStreamWriter,
    validate_xml_input,
};

#[test]
fn default_entrypoints_reject_excessive_active_namespace_bindings() {
    let names: Vec<_> = (0..129).map(|i| format!("xmlns:p{i}")).collect();
    let mut xml = String::from("<r");
    for name in &names {
        use std::fmt::Write as _;
        write!(xml, " {name}='urn:example'").unwrap();
    }
    xml.push_str("/>");
    assert!(validate_xml_input(xml.as_bytes(), XmlSafetyPolicy::untrusted()).is_err());
    assert!(BorrowedDocument::parse(xml.as_bytes()).is_err());
    let mut reader = XmlStreamReader::new(xml.as_bytes(), XmlSafetyPolicy::untrusted()).unwrap();
    loop {
        match reader.read_event() {
            Err(_) => break,
            Ok(XmlStreamEvent::Eof) => panic!("active namespace limit bypassed"),
            Ok(_) => {}
        }
    }
    let mut writer = XmlStreamWriter::new(Vec::new()).unwrap();
    assert!(
        writer
            .empty_element("r", names.iter().map(|name| (name.as_str(), "urn:example")))
            .is_err()
    );
}

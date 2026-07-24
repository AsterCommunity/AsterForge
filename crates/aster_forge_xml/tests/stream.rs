use std::fmt::Write as _;
use std::io::{BufReader, Cursor};

use aster_forge_xml::{Error, XmlSafetyError, XmlSafetyPolicy, XmlStreamEvent, XmlStreamReader};

fn reader(input: &[u8]) -> XmlStreamReader<BufReader<&[u8]>> {
    XmlStreamReader::new(BufReader::new(input), XmlSafetyPolicy::untrusted()).expect("valid policy")
}

#[test]
fn streams_namespace_resolved_names_attributes_and_text() {
    let input =
        br#"<D:root xmlns:D="DAV:" xmlns:x="urn:attr" x:id="7"><D:name>a&amp;b</D:name></D:root>"#;
    let mut reader = reader(input);

    let XmlStreamEvent::Start(root) = reader.read_event().expect("root") else {
        panic!("expected root start");
    };
    assert!(root.name().expect("name").matches("root", Some("DAV:")));
    assert_eq!(
        root.attribute_ns("id", Some("urn:attr"))
            .expect("attribute")
            .as_deref(),
        Some("7")
    );

    let XmlStreamEvent::Start(name) = reader.read_event().expect("name") else {
        panic!("expected name start");
    };
    assert!(name.name().expect("name").matches("name", Some("DAV:")));
    let text = reader.read_text_current().expect("direct text");
    assert_eq!(text, "a&b");

    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::End(_))));
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Eof)));
}

#[test]
fn reuses_validated_values_and_captures_equivalent_mixed_content() {
    let input =
        br#"<root escaped="a&amp;b"><target><![CDATA[x<y]]><!--note-->&#65;&amp;</target></root>"#;
    let mut reader = reader(input);
    let XmlStreamEvent::Start(root) = reader.read_event().expect("root") else {
        panic!("expected root start");
    };
    let escaped = root
        .attributes()
        .find_map(|attribute| {
            let attribute = attribute.expect("valid attribute");
            (attribute.name().expect("name").qualified() == "escaped").then_some(attribute)
        })
        .expect("escaped attribute");
    assert_eq!(escaped.value().expect("first value"), "a&b");
    assert_eq!(escaped.value().expect("cached value"), "a&b");

    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Start(_))));
    let captured = reader.capture_current(1024).expect("captured target");
    assert_eq!(captured.document().root().text().as_deref(), Some("x<yA&"));
    let captured = String::from_utf8_lossy(captured.as_bytes());
    assert!(captured.contains("<![CDATA[x<y]]>"));
    assert!(captured.contains("<!--note-->"));
    assert!(captured.contains("A&amp;"));
}

#[test]
fn skips_unselected_subtrees_without_a_token_stack() {
    let input =
        b"<root><skip><nested><value>ignored</value></nested></skip><keep>yes</keep></root>";
    let mut reader = reader(input);
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Start(_))));
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Start(_))));
    reader.skip_current().expect("skip subtree");

    let XmlStreamEvent::Start(keep) = reader.read_event().expect("keep") else {
        panic!("expected keep start");
    };
    assert_eq!(keep.name().expect("name").local(), "keep");
    assert_eq!(reader.read_text_current().expect("keep text"), "yes");
}

#[test]
fn captures_only_selected_subtree_and_injects_in_scope_namespaces() {
    let input = br#"<D:root xmlns:D="DAV:" xmlns:x="urn:property"><wrapper><x:color shade="dark">a&amp;b</x:color><tail/></wrapper></D:root>"#;
    let mut reader = reader(input);
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Start(_))));
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Start(_))));
    let XmlStreamEvent::Start(color) = reader.read_event().expect("color") else {
        panic!("expected color start");
    };
    assert!(
        color
            .name()
            .expect("name")
            .matches("color", Some("urn:property"))
    );

    let captured = reader.capture_current(1024).expect("captured subtree");
    let root = captured.document().root();
    assert_eq!(root.name(), "color");
    assert_eq!(root.namespace(), Some("urn:property"));
    assert_eq!(root.attribute("shade"), Some("dark"));
    assert_eq!(root.text().as_deref(), Some("a&b"));
    assert!(captured.as_bytes().starts_with(b"<x:color"));
    assert!(String::from_utf8_lossy(captured.as_bytes()).contains("xmlns:x=\"urn:property\""));
}

#[test]
fn stream_helpers_require_the_most_recent_start_event() {
    let mut reader = reader(b"<root/>");
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Empty(_))));
    assert!(matches!(reader.skip_current(), Err(Error::InvalidData(_))));
    assert!(matches!(
        reader.capture_current(128),
        Err(Error::InvalidData(_))
    ));
}

#[test]
fn stream_enforces_input_depth_attribute_text_and_event_limits() {
    let base = XmlSafetyPolicy::untrusted();
    for (input, policy, expected) in [
        (
            b"<root/>".as_slice(),
            XmlSafetyPolicy {
                max_input_bytes: 6,
                ..base
            },
            XmlSafetyError::InputTooLarge,
        ),
        (
            b"<a><b/></a>".as_slice(),
            XmlSafetyPolicy {
                max_depth: 1,
                ..base
            },
            XmlSafetyError::TooDeep,
        ),
        (
            b"<a x='1' y='2'/>".as_slice(),
            XmlSafetyPolicy {
                max_attributes_per_element: 1,
                ..base
            },
            XmlSafetyError::TooManyAttributes,
        ),
        (
            b"<a>text</a>".as_slice(),
            XmlSafetyPolicy {
                max_text_bytes: 3,
                ..base
            },
            XmlSafetyError::TextTooLarge,
        ),
        (
            b"<a><b/></a>".as_slice(),
            XmlSafetyPolicy {
                max_events: 2,
                ..base
            },
            XmlSafetyError::TooManyEvents,
        ),
    ] {
        let mut reader = XmlStreamReader::new(BufReader::new(input), policy).expect("policy");
        let error = loop {
            match reader.read_event() {
                Ok(XmlStreamEvent::Eof) => panic!("fixture should cross a limit"),
                Ok(_) => {}
                Err(error) => break error,
            }
        };
        assert!(matches!(error, Error::Safety(actual) if actual == expected));
    }
}

#[test]
fn stream_rejects_doctype_unknown_prefix_and_multiple_roots() {
    for input in [
        b"<!DOCTYPE root><root/>".as_slice(),
        b"<p:root/>",
        b"<a/><b/>",
    ] {
        let mut reader = reader(input);
        let error = loop {
            match reader.read_event() {
                Ok(XmlStreamEvent::Eof) => panic!("fixture should fail"),
                Ok(_) => {}
                Err(error) => break error,
            }
        };
        assert!(matches!(error, Error::Safety(_) | Error::InvalidXml(_)));
    }
}

#[test]
fn selective_capture_enforces_its_own_byte_limit() {
    let mut reader = reader(b"<root><selected>0123456789</selected></root>");
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Start(_))));
    assert!(matches!(reader.read_event(), Ok(XmlStreamEvent::Start(_))));
    assert!(matches!(
        reader.capture_current(16),
        Err(Error::Safety(XmlSafetyError::InputTooLarge))
    ));
}

#[test]
fn streams_and_selectively_captures_a_hundred_thousand_element_document() {
    let items = 100_000usize;
    let mut input = String::from("<D:multistatus xmlns:D=\"DAV:\">");
    for index in 0..items {
        write!(
            input,
            "<D:response id=\"{index}\"><D:href>/files/{index}</D:href></D:response>"
        )
        .expect("write string fixture");
    }
    input.push_str("</D:multistatus>");
    let policy = XmlSafetyPolicy {
        max_input_bytes: input.len(),
        max_elements: items * 3,
        max_text_bytes: input.len(),
        max_events: items * 8,
        ..XmlSafetyPolicy::untrusted()
    };
    let mut reader = XmlStreamReader::new(BufReader::new(Cursor::new(input.as_bytes())), policy)
        .expect("reader");
    let mut responses = 0usize;
    let mut captured = None;

    loop {
        match reader.read_event().expect("stream event") {
            XmlStreamEvent::Start(start)
                if start
                    .name()
                    .expect("name")
                    .matches("response", Some("DAV:")) =>
            {
                responses += 1;
                if responses == items / 2 {
                    captured = Some(reader.capture_current(256).expect("capture one response"));
                }
            }
            XmlStreamEvent::Eof => break,
            _ => {}
        }
    }

    assert_eq!(responses, items);
    let captured = captured.expect("selected response");
    assert_eq!(
        captured.document().root().attribute("id"),
        Some((items / 2 - 1).to_string().as_str())
    );
    assert_eq!(
        captured
            .document()
            .root()
            .get_child_ns("href", "DAV:")
            .expect("captured href")
            .text()
            .as_deref(),
        Some(format!("/files/{}", items / 2 - 1).as_str())
    );
}

#[test]
fn input_limit_consumes_only_one_probe_byte_past_the_boundary() {
    let input = vec![b'x'; 128 * 1024];
    let policy = XmlSafetyPolicy {
        max_input_bytes: 32,
        ..XmlSafetyPolicy::untrusted()
    };
    let mut reader = XmlStreamReader::new(Cursor::new(input), policy).expect("reader");
    let error = loop {
        match reader.read_event() {
            Ok(XmlStreamEvent::Eof) => panic!("fixture should cross the input limit"),
            Ok(_) => {}
            Err(error) => break error,
        }
    };

    assert!(matches!(
        error,
        Error::Safety(XmlSafetyError::InputTooLarge)
    ));
    assert_eq!(reader.into_inner().position(), 33);
}

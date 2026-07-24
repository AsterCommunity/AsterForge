use std::io::{Read, Write};

use aster_forge_xml::{XmlSafetyPolicy, XmlStreamEvent, XmlStreamReader, XmlStreamWriter};
use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use quick_xml::writer::Writer;

pub(crate) fn cpu_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    // The 10,000-response fixture is reserved for one-shot heap/RSS probes. Generating it here
    // would add about 2.5 MiB of unused setup to every multi-sample Criterion benchmark.
    vec![
        (
            "propfind",
            br#"<D:propfind xmlns:D="DAV:"><D:prop><D:displayname/><D:getcontentlength/><D:getetag/></D:prop></D:propfind>"#
                .to_vec(),
        ),
        ("wopi", wopi_discovery(250).into_bytes()),
        ("multistatus_1000", multistatus(1_000).into_bytes()),
    ]
}

#[allow(dead_code)] // This shared module is also compiled into the CPU-only bench target.
pub(crate) fn memory_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let mut fixtures = cpu_fixtures();
    // This fixture remains below the default 10 MiB input, 100,000-element, one-million-event,
    // and 64 MiB writer limits; it is retained here specifically for large-document memory data.
    fixtures.push(("multistatus_10000", multistatus(10_000).into_bytes()));
    fixtures
}

pub(crate) fn wopi_discovery(actions: usize) -> String {
    let mut xml =
        String::from("<wopi-discovery><net-zone name=\"external-https\"><app name=\"Word\">");
    for index in 0..actions {
        xml.push_str(&format!(
            "<action name=\"view\" ext=\"x{index}\" urlsrc=\"https://office.example.test/view?file=&lt;WOPI_URL&gt;\"/>"
        ));
    }
    xml.push_str("</app></net-zone></wopi-discovery>");
    xml
}

pub(crate) fn multistatus(responses: usize) -> String {
    let mut xml = String::from("<D:multistatus xmlns:D=\"DAV:\">");
    for index in 0..responses {
        xml.push_str(&format!(
            "<D:response><D:href>/files/{index}</D:href><D:propstat><D:prop><D:displayname>file-{index}</D:displayname><D:getcontentlength>{}</D:getcontentlength><D:getetag>&quot;etag-{index}&quot;</D:getetag></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
            index * 1024
        ));
    }
    xml.push_str("</D:multistatus>");
    xml
}

pub(crate) fn walk_quick_xml_events(input: &[u8]) -> usize {
    let mut reader = Reader::from_reader(input);
    let mut checksum = 0usize;
    loop {
        match reader.read_event().expect("benchmark fixture is valid XML") {
            Event::Start(start) | Event::Empty(start) => {
                checksum = checksum.wrapping_add(start.name().as_ref().len());
                for attribute in start.attributes() {
                    let attribute = attribute.expect("benchmark attributes are valid");
                    checksum = checksum
                        .wrapping_add(attribute.key.as_ref().len())
                        .wrapping_add(attribute.value.as_ref().len());
                }
            }
            Event::Text(text) | Event::Comment(text) => {
                checksum = checksum.wrapping_add(text.as_ref().len());
            }
            Event::CData(text) => checksum = checksum.wrapping_add(text.as_ref().len()),
            Event::Eof => break,
            _ => {}
        }
    }
    checksum
}

pub(crate) fn walk_quick_xml_ns_buffered(input: &[u8]) -> usize {
    let mut reader = NsReader::from_reader(input.take(u64::MAX));
    let mut buffer = Vec::new();
    let mut checksum = 0usize;
    loop {
        buffer.clear();
        let event = reader
            .read_event_into(&mut buffer)
            .expect("benchmark fixture is valid XML");
        match event {
            Event::Start(start) | Event::Empty(start) => {
                checksum = checksum
                    .wrapping_add(start.name().as_ref().len())
                    .wrapping_add(namespace_len(
                        reader.resolver().resolve_element(start.name()).0,
                    ));
                for attribute in start.attributes() {
                    let attribute = attribute.expect("benchmark attributes are valid");
                    checksum = checksum
                        .wrapping_add(attribute.key.as_ref().len())
                        .wrapping_add(namespace_len(
                            reader.resolver().resolve_attribute(attribute.key).0,
                        ))
                        .wrapping_add(
                            attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Explicit1_0,
                                    reader.decoder(),
                                )
                                .expect("benchmark value is valid")
                                .len(),
                        );
                }
            }
            Event::End(end) => {
                checksum = checksum
                    .wrapping_add(end.name().as_ref().len())
                    .wrapping_add(namespace_len(
                        reader.resolver().resolve_element(end.name()).0,
                    ));
            }
            Event::Text(text) => {
                let decoded = text.decode().expect("benchmark text is valid");
                checksum = checksum.wrapping_add(
                    unescape(&decoded)
                        .expect("benchmark text entities are valid")
                        .len(),
                );
            }
            Event::GeneralRef(reference) => {
                checksum = checksum.wrapping_add(
                    reference
                        .resolve_char_ref()
                        .expect("benchmark reference is valid")
                        .map_or(1, char::len_utf8),
                );
            }
            Event::CData(text) => {
                checksum =
                    checksum.wrapping_add(text.decode().expect("benchmark CDATA is valid").len());
            }
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) | Event::DocType(_) => {}
            Event::Eof => break,
        }
    }
    checksum
}

fn namespace_len(namespace: ResolveResult<'_>) -> usize {
    match namespace {
        ResolveResult::Bound(namespace) => namespace.as_ref().len(),
        ResolveResult::Unbound => 0,
        ResolveResult::Unknown(prefix) => prefix.len(),
    }
}

pub(crate) fn walk_forge_stream(input: &[u8]) -> usize {
    let mut reader =
        XmlStreamReader::new(input, stream_policy(input)).expect("benchmark policy is valid");
    let mut checksum = 0usize;
    loop {
        match reader.read_event().expect("benchmark fixture is valid XML") {
            XmlStreamEvent::Start(start) | XmlStreamEvent::Empty(start) => {
                let name = start.name().expect("benchmark name is valid");
                checksum = checksum
                    .wrapping_add(name.qualified().len())
                    .wrapping_add(name.namespace().map_or(0, str::len));
                for attribute in start.attributes() {
                    let attribute = attribute.expect("benchmark attribute is valid");
                    let name = attribute.name().expect("benchmark attribute name is valid");
                    checksum = checksum
                        .wrapping_add(name.qualified().len())
                        .wrapping_add(name.namespace().map_or(0, str::len))
                        .wrapping_add(attribute.value().expect("benchmark value is valid").len());
                }
            }
            XmlStreamEvent::End(end) => {
                let name = end.name().expect("benchmark name is valid");
                checksum = checksum
                    .wrapping_add(name.qualified().len())
                    .wrapping_add(name.namespace().map_or(0, str::len));
            }
            XmlStreamEvent::Text(text) => {
                checksum = checksum.wrapping_add(text.value().len());
            }
            XmlStreamEvent::CData(text) => {
                checksum = checksum.wrapping_add(text.value().len());
            }
            XmlStreamEvent::Comment(_)
            | XmlStreamEvent::ProcessingInstruction(_)
            | XmlStreamEvent::Declaration
            | XmlStreamEvent::DocType => {}
            XmlStreamEvent::Eof => break,
        }
    }
    checksum
}

pub(crate) fn validate_forge_stream(input: &[u8]) -> usize {
    let mut reader =
        XmlStreamReader::new(input, stream_policy(input)).expect("benchmark policy is valid");
    let mut events = 0usize;
    loop {
        let event = reader.read_event().expect("benchmark fixture is valid XML");
        if matches!(event, XmlStreamEvent::Eof) {
            return events;
        }
        events = events.wrapping_add(1);
    }
}

fn stream_policy(input: &[u8]) -> XmlSafetyPolicy {
    XmlSafetyPolicy {
        max_input_bytes: input.len().max(1),
        max_elements: 1_000_000,
        max_text_bytes: input.len().max(1),
        max_events: 10_000_000,
        ..XmlSafetyPolicy::untrusted()
    }
}

pub(crate) fn write_forge_multistatus<W: Write>(output: W, responses: usize) -> W {
    let mut writer = XmlStreamWriter::new(output).expect("benchmark writer policy is valid");
    writer
        .start_element("D:multistatus", [("xmlns:D", "DAV:")])
        .expect("write multistatus root");
    for index in 0..responses {
        let href = format!("/files/{index}");
        let display_name = format!("file-{index}");
        let content_length = (index * 1024).to_string();
        let etag = format!("\"etag-{index}\"");
        writer.start("D:response").expect("write response");
        write_forge_text_element(&mut writer, "D:href", &href);
        writer.start("D:propstat").expect("write propstat");
        writer.start("D:prop").expect("write prop");
        write_forge_text_element(&mut writer, "D:displayname", &display_name);
        write_forge_text_element(&mut writer, "D:getcontentlength", &content_length);
        write_forge_text_element(&mut writer, "D:getetag", &etag);
        writer.end_element().expect("write prop end");
        write_forge_text_element(&mut writer, "D:status", "HTTP/1.1 200 OK");
        writer.end_element().expect("write propstat end");
        writer.end_element().expect("write response end");
    }
    writer.end_element().expect("write multistatus end");
    writer.finish().expect("finish benchmark XML")
}

fn write_forge_text_element<W: Write>(writer: &mut XmlStreamWriter<W>, name: &str, value: &str) {
    writer.start(name).expect("write text element");
    writer.text(value).expect("write text");
    writer.end_element().expect("write text element end");
}

pub(crate) fn write_quick_xml_multistatus<W: Write>(output: W, responses: usize) -> W {
    let mut writer = Writer::new(output);
    let mut root = BytesStart::new("D:multistatus");
    root.push_attribute(("xmlns:D", "DAV:"));
    writer.write_event(Event::Start(root)).expect("write root");
    for index in 0..responses {
        let href = format!("/files/{index}");
        let display_name = format!("file-{index}");
        let content_length = (index * 1024).to_string();
        let etag = format!("\"etag-{index}\"");
        writer
            .write_event(Event::Start(BytesStart::new("D:response")))
            .expect("write response");
        write_quick_text_element(&mut writer, "D:href", &href);
        writer
            .write_event(Event::Start(BytesStart::new("D:propstat")))
            .expect("write propstat");
        writer
            .write_event(Event::Start(BytesStart::new("D:prop")))
            .expect("write prop");
        write_quick_text_element(&mut writer, "D:displayname", &display_name);
        write_quick_text_element(&mut writer, "D:getcontentlength", &content_length);
        write_quick_text_element(&mut writer, "D:getetag", &etag);
        writer
            .write_event(Event::End(BytesEnd::new("D:prop")))
            .expect("write prop end");
        write_quick_text_element(&mut writer, "D:status", "HTTP/1.1 200 OK");
        writer
            .write_event(Event::End(BytesEnd::new("D:propstat")))
            .expect("write propstat end");
        writer
            .write_event(Event::End(BytesEnd::new("D:response")))
            .expect("write response end");
    }
    writer
        .write_event(Event::End(BytesEnd::new("D:multistatus")))
        .expect("write root end");
    writer.into_inner()
}

fn write_quick_text_element<W: Write>(writer: &mut Writer<W>, name: &str, value: &str) {
    writer
        .write_event(Event::Start(BytesStart::new(name)))
        .expect("write text element");
    writer
        .write_event(Event::Text(BytesText::new(value)))
        .expect("write text");
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .expect("write text element end");
}

pub(crate) fn write_xmltree_multistatus(responses: usize) -> Vec<u8> {
    let mut root = dav_element("multistatus");
    let mut namespaces = xmltree::Namespace::empty();
    namespaces.put("D", "DAV:");
    root.namespaces = Some(namespaces);
    for index in 0..responses {
        let mut response = dav_element("response");
        response
            .children
            .push(xmltree::XMLNode::Element(xmltree_text_element(
                "href",
                format!("/files/{index}"),
            )));
        let mut propstat = dav_element("propstat");
        let mut prop = dav_element("prop");
        prop.children
            .push(xmltree::XMLNode::Element(xmltree_text_element(
                "displayname",
                format!("file-{index}"),
            )));
        prop.children
            .push(xmltree::XMLNode::Element(xmltree_text_element(
                "getcontentlength",
                (index * 1024).to_string(),
            )));
        prop.children
            .push(xmltree::XMLNode::Element(xmltree_text_element(
                "getetag",
                format!("\"etag-{index}\""),
            )));
        propstat.children.push(xmltree::XMLNode::Element(prop));
        propstat
            .children
            .push(xmltree::XMLNode::Element(xmltree_text_element(
                "status",
                "HTTP/1.1 200 OK".to_owned(),
            )));
        response.children.push(xmltree::XMLNode::Element(propstat));
        root.children.push(xmltree::XMLNode::Element(response));
    }
    let mut options = xmltree::EmitterConfig::new();
    options.perform_indent = false;
    options.write_document_declaration = false;
    options.pad_self_closing = false;
    options.autopad_comments = false;
    let mut output = Vec::new();
    root.write_with_config(&mut output, options)
        .expect("write xmltree benchmark output");
    output
}

fn dav_element(name: &str) -> xmltree::Element {
    let mut element = xmltree::Element::new(name);
    element.prefix = Some("D".to_owned());
    element.namespace = Some("DAV:".to_owned());
    element
}

fn xmltree_text_element(name: &str, value: String) -> xmltree::Element {
    let mut element = dav_element(name);
    element.children.push(xmltree::XMLNode::Text(value));
    element
}

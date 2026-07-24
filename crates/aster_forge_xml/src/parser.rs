//! XML parser — powered by `quick-xml::Reader`
//!
//! Uses a recursive-descent algorithm to convert an XML byte stream into an
//! `Element` tree. Supports safety checks: depth limit, element count limit,
//! input size limit, and DTD/ENTITY rejection.

use std::io::{BufRead, Read};

use quick_xml::escape::unescape as unescape_entities;
use quick_xml::events::Event;
use quick_xml::Reader;
use quick_xml::XmlVersion;

use crate::error::Error;
use crate::{Element, PI};

fn check_size<R: BufRead>(reader: R, options: &ParseOptions) -> Result<Vec<u8>, Error> {
    if let Some(max_size) = options.max_size {
        let mut buf = Vec::new();
        let mut take = reader.take((max_size + 1) as u64);
        let n = take
            .read_to_end(&mut buf)
            .map_err(|e| Error::Io(e.to_string()))?;
        if n > max_size {
            return Err(Error::MaxSizeExceeded);
        }
        Ok(buf)
    } else {
        let mut buf = Vec::new();
        let mut reader = reader;
        reader
            .read_to_end(&mut buf)
            .map_err(|e| Error::Io(e.to_string()))?;
        Ok(buf)
    }
}

#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// Maximum nesting depth (default: 128)
    pub max_depth: usize,
    /// Maximum number of elements (default: 100,000)
    pub max_elements: usize,
    /// Maximum input size in bytes. `None` means unlimited (default: 10 MB)
    pub max_size: Option<usize>,
    /// Whether DTD declarations are allowed (default: false, security)
    pub allow_dtd: bool,
    /// Whether ENTITY declarations are allowed (default: false, security)
    pub allow_entity: bool,
    /// Whether to trim leading/trailing whitespace from text nodes (default: true)
    pub trim_whitespace: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            max_depth: 128,
            max_elements: 100_000,
            max_size: Some(10 * 1024 * 1024), // 10 MB
            allow_dtd: false,
            allow_entity: false,
            trim_whitespace: true,
        }
    }
}

impl ParseOptions {
    /// Creates default `ParseOptions`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum nesting depth.
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Sets the maximum number of elements.
    pub fn max_elements(mut self, count: usize) -> Self {
        self.max_elements = count;
        self
    }

    /// Sets the maximum input size in bytes. Pass `None` for unlimited.
    pub fn max_size(mut self, size: impl Into<Option<usize>>) -> Self {
        self.max_size = size.into();
        self
    }

    /// Sets whether DTD is allowed.
    pub fn allow_dtd(mut self, allow: bool) -> Self {
        self.allow_dtd = allow;
        self
    }

    /// Sets whether ENTITY is allowed.
    pub fn allow_entity(mut self, allow: bool) -> Self {
        self.allow_entity = allow;
        self
    }

    /// Sets whether text whitespace should be trimmed.
    pub fn trim_whitespace(mut self, trim: bool) -> Self {
        self.trim_whitespace = trim;
        self
    }
}

pub(crate) fn parse<R: BufRead>(
    reader: R,
    options: &ParseOptions,
) -> Result<Element, Error> {
    // Security: read all input first and check size limit.
    // This catches oversized documents (e.g. XML bombs) before parsing.
    let bytes = check_size(reader, options)?;

    let mut reader = Reader::from_reader(bytes.as_slice());

    reader.config_mut().trim_text(options.trim_whitespace);

    let mut buf = Vec::new();
    let mut element_count = 0usize;

    // Skip non-root events (comments, PIs, etc.) before the root element
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                // Key pattern: extract all data inside a block, then clear buf.
                // This releases `e`'s borrow on buf before the recursive call.
                let elem = {
                    let name = extract_name(e.name().as_ref());
                    let attrs = extract_attributes(&e)?;

                    buf.clear();

                    build_tree(
                        &mut reader,
                        name,
                        attrs,
                        &mut Vec::new(),
                        1,
                        options,
                        &mut element_count,
                    )?
                };
                buf.clear();
                return Ok(elem);
            }
            Event::Empty(e) => {
                let elem = {
                    let name = extract_name(e.name().as_ref());
                    let attrs = extract_attributes(&e)?;
                    buf.clear();
                    check_element_count(&mut element_count, options)?;
                    Element {
                        name,
                        attributes: attrs,
                        children: Vec::new(),
                        text: None,
                        pi: Vec::new(),
                        namespace: None,
                    }
                };
                buf.clear();
                return Ok(elem);
            }
            Event::Decl(_) => {
                buf.clear();
                continue;
            }
            Event::PI(_) => {
                buf.clear();
                continue;
            }
            Event::Comment(_) => {
                buf.clear();
                continue;
            }
            Event::DocType(_) => {
                if !options.allow_dtd {
                    buf.clear();
                    return Err(Error::DtdNotAllowed);
                }
                if !options.allow_entity {
                    buf.clear();
                    return Err(Error::EntityNotAllowed);
                }
                buf.clear();
                continue;
            }
            Event::Eof => {
                return Err(Error::InvalidXml("empty document or no root element found".into()));
            }
            _ => {
                buf.clear();
                continue;
            }
        }
    }
}

fn build_tree<R: BufRead>(
    reader: &mut Reader<R>,
    name: String,
    attributes: std::collections::HashMap<String, String>,
    buf: &mut Vec<u8>,
    depth: usize,
    options: &ParseOptions,
    count: &mut usize,
) -> Result<Element, Error> {
    if depth > options.max_depth {
        return Err(Error::MaxDepthExceeded);
    }

    let mut current_text: Option<String> = None;
    let mut children: Vec<Element> = Vec::new();
    let mut pi_list: Vec<PI> = Vec::new();

    loop {
        match reader.read_event_into(buf)? {
            // Child element start → recurse
            Event::Start(e) => {
                let (child_name, child_attrs) = {
                    let name = extract_name(e.name().as_ref());
                    let attrs = extract_attributes(&e)?;
                    buf.clear();
                    (name, attrs)
                };
                let child = build_tree(reader, child_name, child_attrs, buf, depth + 1, options, count)?;
                children.push(child);
            }

            // Current element end → return to parent
            Event::End(_) => {
                buf.clear();
                break;
            }

            // Self-closing child element
            Event::Empty(e) => {
                let (child_name, child_attrs) = {
                    let name = extract_name(e.name().as_ref());
                    let attrs = extract_attributes(&e)?;
                    buf.clear();
                    (name, attrs)
                };
                check_element_count(count, options)?;
                children.push(Element {
                    name: child_name,
                    attributes: child_attrs,
                    children: Vec::new(),
                    text: None,
                    pi: Vec::new(),
                    namespace: None,
                });
            }

            // Text content (decoded encoding + unescaped entities)
            Event::Text(e) => {
                let decoded = e.decode()?;
                let unescaped = unescape_entities(decoded.as_ref())?;
                if !unescaped.is_empty() {
                    match current_text.as_mut() {
                        Some(ref mut s) => s.push_str(unescaped.as_ref()),
                        None => current_text = Some(unescaped.into_owned()),
                    }
                }
                buf.clear();
            }

            // CDATA section
            Event::CData(e) => {
                let text_str = String::from_utf8_lossy(e.as_ref());
                if !text_str.is_empty() {
                    match current_text.as_mut() {
                        Some(ref mut s) => s.push_str(&text_str),
                        None => current_text = Some(text_str.into_owned()),
                    }
                }
                buf.clear();
            }

            // Processing instruction
            Event::PI(e) => {
                let target = String::from_utf8_lossy(e.target()).into_owned();
                let content = String::from_utf8_lossy(e.content()).into_owned();
                pi_list.push(PI {
                    name: target,
                    content,
                });
                buf.clear();
            }

            // XML declaration (<?xml ...?>) — ignore
            Event::Decl(_) => {
                buf.clear();
            }

            // Comment — ignore
            Event::Comment(_) => {
                buf.clear();
            }

            // DTD
            Event::DocType(_) => {
                if !options.allow_dtd {
                    buf.clear();
                    return Err(Error::DtdNotAllowed);
                }
                if !options.allow_entity {
                    buf.clear();
                    return Err(Error::EntityNotAllowed);
                }
                buf.clear();
            }

            // Entity reference/character reference (&amp; → &, &#60; → <)
            Event::GeneralRef(e) => {
                let name = e.decode()?;
                let entity_ref = format!("&{};", name);
                let resolved = unescape_entities(&entity_ref)?;
                if !resolved.is_empty() {
                    match current_text.as_mut() {
                        Some(ref mut s) => s.push_str(resolved.as_ref()),
                        None => current_text = Some(resolved.into_owned()),
                    }
                }
                buf.clear();
            }

            // Unexpected EOF
            Event::Eof => {
                return Err(Error::InvalidXml(
                    "unexpected end of XML: element was not closed".into(),
                ));
            }
        }
    }

    Ok(Element {
        name,
        attributes,
        children,
        text: current_text,
        pi: pi_list,
        namespace: None,
    })
}

fn extract_name(name_bytes: &[u8]) -> String {
    String::from_utf8_lossy(name_bytes).into_owned()
}

fn extract_attributes(
    start: &quick_xml::events::BytesStart,
) -> Result<std::collections::HashMap<String, String>, Error> {
    let mut attrs = std::collections::HashMap::new();
    for attr_result in start.attributes() {
        let attr = attr_result?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        let value = attr.normalized_value(XmlVersion::Explicit1_0)?.into_owned();
        attrs.insert(key, value);
    }
    Ok(attrs)
}

fn check_element_count(count: &mut usize, options: &ParseOptions) -> Result<(), Error> {
    *count += 1;
    if *count > options.max_elements {
        return Err(Error::MaxElementsExceeded);
    }
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Element;

    // -----------------------------------------------------------------------
    // Basic parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_self_closing_element() {
        let xml = "<root/>";
        let elem = Element::from_str(xml).expect("should parse self-closing tag");
        assert_eq!(elem.name, "root");
        assert!(elem.attributes.is_empty());
        assert!(elem.children.is_empty());
        assert!(elem.text.is_none());
    }

    #[test]
    fn test_element_with_text() {
        let xml = "<root>hello world</root>";
        let elem = Element::from_str(xml).expect("should parse element with text");
        assert_eq!(elem.name, "root");
        assert_eq!(elem.get_text(), Some("hello world"));
    }

    #[test]
    fn test_nested_elements() {
        let xml = "<root><child>text</child></root>";
        let elem = Element::from_str(xml).expect("should parse nested elements");
        assert_eq!(elem.name, "root");
        assert_eq!(elem.children.len(), 1);
        assert_eq!(elem.children[0].name, "child");
        assert_eq!(elem.children[0].get_text(), Some("text"));
    }

    #[test]
    fn test_multiple_children() {
        let xml = "<root><a/><b/><c/></root>";
        let elem = Element::from_str(xml).expect("should parse multiple children");
        assert_eq!(elem.children.len(), 3);
        assert_eq!(elem.children[0].name, "a");
        assert_eq!(elem.children[1].name, "b");
        assert_eq!(elem.children[2].name, "c");
    }

    // -----------------------------------------------------------------------
    // Attributes
    // -----------------------------------------------------------------------

    #[test]
    fn test_single_attribute() {
        let xml = r#"<root name="value"/>"#;
        let elem = Element::from_str(xml).expect("should parse element with attribute");
        assert_eq!(elem.get_attr("name"), Some("value"));
    }

    #[test]
    fn test_multiple_attributes() {
        let xml = r#"<root a="1" b="2" c="3"/>"#;
        let elem = Element::from_str(xml).expect("should parse element with multiple attributes");
        assert_eq!(elem.get_attr("a"), Some("1"));
        assert_eq!(elem.get_attr("b"), Some("2"));
        assert_eq!(elem.get_attr("c"), Some("3"));
    }

    #[test]
    fn test_attribute_with_escape() {
        let xml = r#"<root text="a&amp;b&quot;c"/>"#;
        let elem = Element::from_str(xml).expect("should decode escaped attribute values");
        assert_eq!(elem.get_attr("text"), Some("a&b\"c"));
    }

    #[test]
    fn test_empty_attribute_value() {
        let xml = r#"<root empty=""/>"#;
        let elem = Element::from_str(xml).expect("should parse empty attribute value");
        assert_eq!(elem.get_attr("empty"), Some(""));
    }

    // -----------------------------------------------------------------------
    // Deep nesting
    // -----------------------------------------------------------------------

    #[test]
    fn test_deeply_nested() {
        let mut xml = String::from("<root>");
        for i in 0..10 {
            xml.push_str(&format!("<level{}>", i));
        }
        xml.push_str("deep");
        for i in (0..10).rev() {
            xml.push_str(&format!("</level{}>", i));
        }
        xml.push_str("</root>");

        let elem = Element::from_str(&xml).expect("should parse deeply nested XML");
        assert_eq!(elem.name, "root");

        let mut current = &elem.children[0];
        for _ in 0..9 {
            current = &current.children[0];
        }
        assert_eq!(current.get_text(), Some("deep"));
    }

    // -----------------------------------------------------------------------
    // Safety checks
    // -----------------------------------------------------------------------

    #[test]
    fn test_max_depth_exceeded() {
        let depth = ParseOptions::default().max_depth + 1;
        let mut xml = String::from("<root>");
        for _ in 0..depth {
            xml.push_str("<a>");
        }
        xml.push_str("x");
        for _ in 0..depth {
            xml.push_str("</a>");
        }
        xml.push_str("</root>");

        let options = ParseOptions::default();
        let result = Element::from_reader(xml.as_bytes(), &options);
        assert!(
            matches!(result, Err(Error::MaxDepthExceeded)),
            "depth exceeded should return MaxDepthExceeded, got {:?}",
            result
        );
    }

    #[test]
    fn test_dtd_rejected_by_default() {
        let xml = r#"<!DOCTYPE foo><root/>"#;
        let result = Element::from_str(xml);
        assert!(
            matches!(result, Err(Error::DtdNotAllowed)),
            "DTD should be rejected by default, got {:?}",
            result
        );
    }

    #[test]
    fn test_dtd_allowed_with_option() {
        let xml = r#"<!DOCTYPE foo><root/>"#;
        let options = ParseOptions::new()
            .allow_dtd(true)
            .allow_entity(true);
        let elem = Element::from_reader(xml.as_bytes(), &options)
            .expect("setting allow_dtd(true) and allow_entity(true) should allow DTD");
        assert_eq!(elem.name, "root");
    }

    #[test]
    fn test_max_size_exceeded() {
        let xml = "<root><child>hello</child></root>";
        let options = ParseOptions::new().max_size(5);
        let result = Element::from_reader(xml.as_bytes(), &options);
        assert!(
            matches!(result, Err(Error::MaxSizeExceeded)),
            "size exceeded should return MaxSizeExceeded, got {:?}",
            result
        );
    }

    #[test]
    fn test_max_size_no_limit_when_none() {
        let xml = "<root><child>hello</child></root>";
        let options = ParseOptions::new().max_size(None::<usize>);
        let elem = Element::from_reader(xml.as_bytes(), &options)
            .expect("no limit should parse normally");
        assert_eq!(elem.name, "root");
    }

    // -----------------------------------------------------------------------
    // Processing instructions
    // -----------------------------------------------------------------------

    #[test]
    fn test_pi_inside_element() {
        let xml = "<root><?pi data?></root>";
        let elem = Element::from_str(xml).expect("should parse PI inside element");
        assert_eq!(elem.pi.len(), 1);
        assert_eq!(elem.pi[0].name, "pi");
        assert_eq!(elem.pi[0].content, " data");
    }

    // -----------------------------------------------------------------------
    // CDATA
    // -----------------------------------------------------------------------

    #[test]
    fn test_cdata_section() {
        let xml = "<root><![CDATA[<raw> & stuff]]></root>";
        let elem = Element::from_str(xml).expect("should parse CDATA");
        assert_eq!(elem.get_text(), Some("<raw> & stuff"));
    }

    // -----------------------------------------------------------------------
    // Escaped characters
    // -----------------------------------------------------------------------

    #[test]
    fn test_escaped_characters_in_text() {
        let xml = "<root>&amp;&lt;&gt;&quot;&apos;</root>";
        let elem = Element::from_str(xml).expect("should decode escaped characters in text");
        assert_eq!(elem.get_text(), Some("&<>\"'"));
    }

    // -----------------------------------------------------------------------
    // Error handling
    // -----------------------------------------------------------------------

    #[test]
    fn test_malformed_xml() {
        let xml = "<root><unclosed>";
        let result = Element::from_str(xml);
        assert!(result.is_err(), "malformed XML should return an error");
    }

    #[test]
    fn test_empty_document() {
        let xml = "";
        let result = Element::from_str(xml);
        assert!(
            matches!(result, Err(Error::InvalidXml(_))),
            "empty document should return InvalidXml, got {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // XML declaration
    // -----------------------------------------------------------------------

    #[test]
    fn test_xml_declaration() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><root/>"#;
        let elem = Element::from_str(xml).expect("should skip XML declaration");
        assert_eq!(elem.name, "root");
    }

    // -----------------------------------------------------------------------
    // Comments
    // -----------------------------------------------------------------------

    #[test]
    fn test_comment_ignored() {
        let xml = "<root><!-- comment --></root>";
        let elem = Element::from_str(xml).expect("should ignore comments");
        assert!(elem.text.is_none());
    }

    #[test]
    fn test_comment_before_root() {
        let xml = "<!-- comment --><root/>";
        let elem = Element::from_str(xml).expect("should skip comments before root");
        assert_eq!(elem.name, "root");
    }

    // -----------------------------------------------------------------------
    // Mixed content
    // -----------------------------------------------------------------------

    #[test]
    fn test_mixed_text_and_children() {
        let xml = "<root>before<child/>after</root>";
        let elem = Element::from_str(xml).expect("should parse mixed content");
        assert_eq!(elem.get_text(), Some("beforeafter"));
        assert_eq!(elem.children.len(), 1);
        assert_eq!(elem.children[0].name, "child");
    }

    #[test]
    fn test_multiple_text_nodes() {
        let xml = "<root>a<b/>c</root>";
        let elem = Element::from_str(xml).expect("should parse multiple text nodes");
        assert_eq!(elem.get_text(), Some("ac"));
    }

    // -----------------------------------------------------------------------
    // Empty element
    // -----------------------------------------------------------------------

    #[test]
    fn test_element_no_text() {
        let xml = "<root></root>";
        let elem = Element::from_str(xml).expect("should parse empty element");
        assert!(elem.text.is_none());
        assert!(elem.children.is_empty());
    }

    // -----------------------------------------------------------------------
    // Query methods
    // -----------------------------------------------------------------------

    #[test]
    fn test_has_attr() {
        let xml = r#"<root a="1"/>"#;
        let elem = Element::from_str(xml).unwrap();
        assert!(elem.has_attr("a"));
        assert!(!elem.has_attr("b"));
    }

    #[test]
    fn test_get_child() {
        let xml = "<root><child>text</child></root>";
        let elem = Element::from_str(xml).unwrap();
        let child = elem.get_child("child").expect("child should be found");
        assert_eq!(child.get_text(), Some("text"));
    }

    #[test]
    fn test_get_child_not_found() {
        let xml = "<root/>";
        let elem = Element::from_str(xml).unwrap();
        assert!(elem.get_child("nonexistent").is_none());
    }

    #[test]
    fn test_get_children_multiple() {
        let xml = "<root><item/><item/><item/></root>";
        let elem = Element::from_str(xml).unwrap();
        let items = elem.get_children("item");
        assert_eq!(items.len(), 3);
    }

    // -----------------------------------------------------------------------
    // From bytes
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_bytes() {
        let xml = b"<root>bytes</root>";
        let elem = Element::from_bytes(xml).expect("should parse byte slice");
        assert_eq!(elem.get_text(), Some("bytes"));
    }

    // -----------------------------------------------------------------------
    // Namespaced attribute
    // -----------------------------------------------------------------------

    #[test]
    fn test_namespaced_attribute() {
        let xml = r#"<root xml:lang="en"/>"#;
        let elem = Element::from_str(xml).expect("should parse namespaced attribute");
        assert_eq!(elem.get_attr("xml:lang"), Some("en"));
    }

    // -----------------------------------------------------------------------
    // Indented XML (trim_whitespace enabled by default)
    // -----------------------------------------------------------------------

    #[test]
    fn test_indented_xml() {
        let xml = "<root>\n  <child>text</child>\n</root>";
        let elem = Element::from_str(xml).expect("should parse indented XML");
        assert_eq!(elem.name, "root");
        assert!(elem.text.is_none() || elem.get_text().unwrap().trim().is_empty());
        assert_eq!(elem.children.len(), 1);
        assert_eq!(elem.children[0].get_text(), Some("text"));
    }
}

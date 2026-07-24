//! XML serializer — powered by `quick-xml::Writer`
//!
//! Converts an `Element` tree into a well-formed XML string.
//! Supports indentation, automatic attribute/text escaping, and
//! processing instruction output.

use std::io::Write;

use quick_xml::events::{BytesEnd, BytesPI, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;

use crate::error::Error;
use crate::Element;

#[derive(Debug, Clone)]
pub struct SerializeOptions {
    /// Indentation character (space `b' '` or tab `b'\t'`)
    pub indent_char: u8,
    /// Number of indent characters per level
    pub indent_size: usize,
    /// Whether to use indentation (false = compact output)
    pub use_indent: bool,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        SerializeOptions {
            indent_char: b' ',
            indent_size: 2,
            use_indent: true,
        }
    }
}

impl SerializeOptions {
    /// Creates default serialization options (2-space indent).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets indent character and size.
    pub fn indent(mut self, ch: u8, size: usize) -> Self {
        self.indent_char = ch;
        self.indent_size = size;
        self.use_indent = true;
        self
    }

    /// Disables indentation, enabling compact mode (everything on one line).
    pub fn no_indent(mut self) -> Self {
        self.use_indent = false;
        self
    }
}

pub fn to_string(elem: &Element) -> Result<String, Error> {
    let mut buffer = Vec::new();
    to_writer(&mut buffer, elem, &SerializeOptions::default())?;
    String::from_utf8(buffer).map_err(|e| Error::Io(e.to_string()))
}

pub fn to_writer<W: Write>(
    writer: W,
    elem: &Element,
    options: &SerializeOptions,
) -> Result<(), Error> {
    let mut writer = if options.use_indent {
        Writer::new_with_indent(writer, options.indent_char, options.indent_size)
    } else {
        Writer::new(writer)
    };

    write_element(&mut writer, elem)
        .map_err(|e| Error::Io(e.to_string()))?;

    Ok(())
}

fn write_element<W: Write>(writer: &mut Writer<W>, elem: &Element) -> std::io::Result<()> {
    // Build the start tag (with attributes)
    let mut start = BytesStart::new(&elem.name);
    for (key, value) in &elem.attributes {
        // quick-xml's Writer automatically escapes attribute values (" → &quot; etc.)
        start.push_attribute((key.as_str(), value.as_str()));
    }

    let has_content = elem.text.is_some() || !elem.children.is_empty() || !elem.pi.is_empty();

    if !has_content {
        // Self-closing tag: <tag attr="value"/>
        writer.write_event(Event::Empty(start))
    } else {
        // Start tag
        writer.write_event(Event::Start(start))?;

        // Processing instructions
        for pi in &elem.pi {
            // BytesPI::new takes "target content" format
            let pi_str = if pi.content.starts_with(' ') {
                format!("{}{}", pi.name, pi.content)
            } else if pi.content.is_empty() {
                pi.name.clone()
            } else {
                format!("{} {}", pi.name, pi.content)
            };
            writer.write_event(Event::PI(BytesPI::new(&pi_str)))?;
        }

        // Text content
        if let Some(ref text) = elem.text {
            // quick-xml's Writer automatically escapes text (< → &lt; etc.)
            let bt = BytesText::new(text.as_str());
            writer.write_event(Event::Text(bt))?;
        }

        // Recursively write children
        for child in &elem.children {
            write_element(writer, child)?;
        }

        // End tag
        writer.write_event(Event::End(BytesEnd::new(&elem.name)))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Element;

    // -----------------------------------------------------------------------
    // Basic serialization
    // -----------------------------------------------------------------------

    #[test]
    fn test_self_closing() {
        let elem = Element::new("root");
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, "<root/>");
    }

    #[test]
    fn test_element_with_text() {
        let mut elem = Element::new("root");
        elem.set_text("hello world");
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, "<root>hello world</root>");
    }

    #[test]
    fn test_nested_elements() {
        let mut parent = Element::new("parent");
        let mut child = Element::new("child");
        child.set_text("inner");
        parent.push(child);

        let xml = to_string(&parent).unwrap();
        assert_eq!(xml, "<parent>\n  <child>inner</child>\n</parent>");
    }

    // -----------------------------------------------------------------------
    // Attributes
    // -----------------------------------------------------------------------

    #[test]
    fn test_single_attribute() {
        let mut elem = Element::new("item");
        elem.set_attr("id", "42");
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, r#"<item id="42"/>"#);
    }

    #[test]
    fn test_multiple_attributes() {
        let mut elem = Element::new("item");
        elem.set_attr("id", "1");
        elem.set_attr("name", "test");
        elem.set_attr("type", "foo");
        let xml = to_string(&elem).unwrap();
        assert!(xml.starts_with("<item "));
        assert!(xml.ends_with("/>"));
        assert!(xml.contains(r#"id="1""#));
        assert!(xml.contains(r#"name="test""#));
        assert!(xml.contains(r#"type="foo""#));
    }

    #[test]
    fn test_attribute_value_escaping() {
        let mut elem = Element::new("root");
        elem.set_attr("msg", r#"he said "hello""#);
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, r#"<root msg="he said &quot;hello&quot;"/>"#);
    }

    #[test]
    fn test_attribute_with_ampersand() {
        let mut elem = Element::new("root");
        elem.set_attr("url", "a&b");
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, r#"<root url="a&amp;b"/>"#);
    }

    #[test]
    fn test_empty_attribute_value() {
        let mut elem = Element::new("root");
        elem.set_attr("empty", "");
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, r#"<root empty=""/>"#);
    }

    // -----------------------------------------------------------------------
    // Text escaping
    // -----------------------------------------------------------------------

    #[test]
    fn test_text_escaping_lt_gt_amp() {
        let mut elem = Element::new("root");
        elem.set_text("a < b > c & d");
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, "<root>a &lt; b &gt; c &amp; d</root>");
    }

    #[test]
    fn test_text_with_special_chars() {
        let mut elem = Element::new("data");
        elem.set_text("'single' & \"double\" <tag>");
        let xml = to_string(&elem).unwrap();
        assert_eq!(
            xml,
            "<data>&apos;single&apos; &amp; &quot;double&quot; &lt;tag&gt;</data>"
        );
    }

    #[test]
    fn test_unicode_text() {
        let mut elem = Element::new("root");
        elem.set_text("中文 日本語 한국어 🌍");
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, "<root>中文 日本語 한국어 🌍</root>");
    }

    // -----------------------------------------------------------------------
    // Mixed content and structure
    // -----------------------------------------------------------------------

    #[test]
    fn test_mixed_content() {
        let mut root = Element::new("root");
        root.set_text("before");
        let mut child = Element::new("child");
        child.set_text("inside");
        root.push(child);

        let xml = to_string(&root).unwrap();
        assert!(xml.contains("before"));
        assert!(xml.contains("inside"));
        assert!(xml.contains("<child>"));
        assert!(xml.contains("</child>"));
    }

    #[test]
    fn test_multiple_children_indented() {
        let mut root = Element::new("root");
        root.push(Element::new("a"));
        root.push(Element::new("b"));
        root.push(Element::new("c"));

        let xml = to_string(&root).unwrap();
        assert_eq!(xml, "<root>\n  <a/>\n  <b/>\n  <c/>\n</root>");
    }

    #[test]
    fn test_element_with_child_only() {
        let mut root = Element::new("root");
        root.push(Element::new("child"));
        let xml = to_string(&root).unwrap();
        assert!(xml.starts_with("<root>"), "root should start with <root>, not <root/>");
        assert!(xml.trim_end().ends_with("</root>"), "root should end with </root>");
        assert!(xml.contains("<child/>"), "child should be self-closing");
    }

    // -----------------------------------------------------------------------
    // Empty / self-closing
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_open_close() {
        let mut elem = Element::new("root");
        elem.text = Some(String::new());
        let xml = to_string(&elem).unwrap();
        assert_eq!(xml, "<root></root>");
    }

    // -----------------------------------------------------------------------
    // Indentation modes
    // -----------------------------------------------------------------------

    #[test]
    fn test_no_indent() {
        let mut root = Element::new("root");
        let mut child = Element::new("child");
        child.set_attr("id", "1");
        child.set_text("text");
        root.push(child);

        let mut buf = Vec::new();
        let opts = SerializeOptions::new().no_indent();
        to_writer(&mut buf, &root, &opts).unwrap();
        let xml = String::from_utf8(buf).unwrap();
        assert_eq!(xml, r#"<root><child id="1">text</child></root>"#);
    }

    #[test]
    fn test_tab_indent() {
        let mut root = Element::new("root");
        root.push(Element::new("child"));

        let mut buf = Vec::new();
        let opts = SerializeOptions::new().indent(b'\t', 1);
        to_writer(&mut buf, &root, &opts).unwrap();
        let xml = String::from_utf8(buf).unwrap();
        assert_eq!(xml, "<root>\n\t<child/>\n</root>");
    }

    #[test]
    fn test_custom_indent_size() {
        let mut root = Element::new("root");
        root.push(Element::new("child"));

        let mut buf = Vec::new();
        let opts = SerializeOptions::new().indent(b' ', 4);
        to_writer(&mut buf, &root, &opts).unwrap();
        let xml = String::from_utf8(buf).unwrap();
        assert_eq!(xml, "<root>\n    <child/>\n</root>");
    }

    // -----------------------------------------------------------------------
    // Deep nesting
    // -----------------------------------------------------------------------

    #[test]
    fn test_deeply_nested_indented() {
        let mut root = Element::new("root");
        let mut current = &mut root;
        for _ in 0..5 {
            let child = Element::new("l");
            current.push(child);
            current = current.children.last_mut().unwrap();
        }

        let xml = to_string(&root).unwrap();
        assert!(xml.starts_with("<root>"), "should start with <root>");
        assert!(xml.trim_end().ends_with("</root>"), "should end with </root>");
    }

    #[test]
    fn test_deep_nesting_no_indent() {
        let mut root = Element::new("0");
        let mut current = &mut root;
        for i in 1..100 {
            let child = Element::new(format!("{}", i));
            current.push(child);
            current = current.children.last_mut().unwrap();
        }

        let mut buf = Vec::new();
        let opts = SerializeOptions::new().no_indent();
        to_writer(&mut buf, &root, &opts).unwrap();
        let xml = String::from_utf8(buf).unwrap();

        assert!(xml.starts_with("<0>"), "should start with <0>");
        assert!(xml.ends_with("</0>"), "should end with </0>");

        for i in 1..99 {
            assert!(xml.contains(&format!("<{}>", i)), "missing <{}>", i);
            assert!(xml.contains(&format!("</{}>", i)), "missing </{}>", i);
        }
        assert!(xml.contains("<99/>"), "leaf element 99 should be self-closing");
    }

    // -----------------------------------------------------------------------
    // Processing instructions
    // -----------------------------------------------------------------------

    #[test]
    fn test_element_with_pi() {
        let mut elem = Element::new("root");
        elem.pi.push(crate::PI {
            name: "xml-stylesheet".into(),
            content: r#" href="style.css""#.into(),
        });

        let xml = to_string(&elem).unwrap();
        assert_eq!(
            xml,
            r#"<root>
  <?xml-stylesheet href="style.css"?>
</root>"#
        );
    }

    #[test]
    fn test_element_with_pi_no_indent() {
        let mut elem = Element::new("root");
        elem.pi.push(crate::PI {
            name: "xml-stylesheet".into(),
            content: r#" href="style.css""#.into(),
        });

        let mut buf = Vec::new();
        let opts = SerializeOptions::new().no_indent();
        to_writer(&mut buf, &elem, &opts).unwrap();
        let xml = String::from_utf8(buf).unwrap();
        assert_eq!(
            xml,
            r#"<root><?xml-stylesheet href="style.css"?></root>"#
        );
    }

    #[test]
    fn test_multiple_pis_indented() {
        let mut elem = Element::new("root");
        elem.pi.push(crate::PI {
            name: "pi1".into(),
            content: " data1".into(),
        });
        elem.pi.push(crate::PI {
            name: "pi2".into(),
            content: " data2".into(),
        });

        let xml = to_string(&elem).unwrap();
        assert_eq!(
            xml,
            "<root>\n  <?pi1 data1?>\n  <?pi2 data2?>\n</root>"
        );
    }

    // -----------------------------------------------------------------------
    // Namespace / naming
    // -----------------------------------------------------------------------

    #[test]
    fn test_namespaced_name() {
        let mut elem = Element::new("ns:root");
        elem.set_attr("xmlns:ns", "http://example.com");
        let xml = to_string(&elem).unwrap();
        assert_eq!(
            xml,
            r#"<ns:root xmlns:ns="http://example.com"/>"#
        );
    }

    // -----------------------------------------------------------------------
    // Write to Vec<u8>
    // -----------------------------------------------------------------------

    #[test]
    fn test_write_to_vec() {
        let elem = Element::new("root");
        let mut buf = Vec::new();
        to_writer(&mut buf, &elem, &SerializeOptions::default()).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "<root/>");
    }
}

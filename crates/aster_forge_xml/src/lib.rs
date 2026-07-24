//! `aster_forge_xml` — a high-performance XML tree structure library.
//!
//! Built on top of `quick-xml`, this crate provides a DOM-like XML tree API
//! that is functionally equivalent to `xmltree`, but with significantly better
//! performance (estimated 3–8× faster parsing).


mod parser;
pub mod serializer;
mod error;

pub use error::Error;
pub use parser::ParseOptions;
pub use serializer::SerializeOptions;

use std::collections::HashMap;
use std::fmt;
use std::io::Write;

#[derive(Debug, Clone, PartialEq)]
pub struct PI {
    /// Instruction target, e.g. `"xml"`
    pub name: String,
    /// Instruction content string
    pub content: String,
}
pub trait ElementPredicate {
    /// Returns `true` if this element matches the predicate.
    fn match_element(&self, element: &Element) -> bool;
}

impl ElementPredicate for str {
    fn match_element(&self, element: &Element) -> bool {
        element.name == self
    }
}

impl ElementPredicate for &str {
    fn match_element(&self, element: &Element) -> bool {
        element.name == *self
    }
}

impl ElementPredicate for String {
    fn match_element(&self, element: &Element) -> bool {
        element.name == *self
    }
}

impl<TN: AsRef<str>, NS: AsRef<str>> ElementPredicate for (TN, NS) {
    fn match_element(&self, element: &Element) -> bool {
        element.name == self.0.as_ref()
            && element
                .namespace
                .as_ref()
                .map(|ns| ns == self.1.as_ref())
                .unwrap_or(false)
    }
}


#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// Element name (e.g. `"root"`, `"child"`)
    pub name: String,
    /// Attribute key-value pairs
    pub attributes: HashMap<String, String>,
    /// Child element nodes
    pub children: Vec<Element>,
    /// Text content (`None` means no text)
    pub text: Option<String>,
    /// Processing instructions
    pub pi: Vec<PI>,
    /// Optional namespace URI
    pub namespace: Option<String>,
}


impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Element {
            name: name.into(),
            attributes: HashMap::new(),
            children: Vec::new(),
            text: None,
            pi: Vec::new(),
            namespace: None,
        }
    }
}

impl Element {
    pub fn with_attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(name.into(), value.into());
        self
    }
    pub fn with_child(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    /// Builder-style: sets text content and returns self.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Builder-style: sets the namespace and returns self.
    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = Some(ns.into());
        self
    }
}

impl Element {
    /// Gets an attribute value by name.
    pub fn get_attr(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(|s| s.as_str())
    }

    /// Sets an attribute value.
    pub fn set_attr(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.attributes.insert(name.into(), value.into());
    }

    /// Returns `true` if the element has the named attribute.
    pub fn has_attr(&self, name: &str) -> bool {
        self.attributes.contains_key(name)
    }

    /// Removes an attribute by name and returns its value.
    pub fn remove_attr(&mut self, name: &str) -> Option<String> {
        self.attributes.remove(name)
    }

    /// Clears all attributes.
    pub fn clear_attributes(&mut self) {
        self.attributes.clear();
    }

    /// Returns the number of attributes.
    pub fn num_attrs(&self) -> usize {
        self.attributes.len()
    }

    /// Returns an iterator over (key, value) attribute pairs.
    pub fn iter_attrs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attributes.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl Element {
    /// Gets the text content, if any.
    pub fn get_text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Sets the text content.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = Some(text.into());
    }

    /// Takes the text content (consumes ownership).
    pub fn take_text(&mut self) -> Option<String> {
        self.text.take()
    }

    /// Returns `true` if the element has text content.
    pub fn has_text(&self) -> bool {
        self.text.is_some()
    }

    /// Clears the text content.
    pub fn clear_text(&mut self) {
        self.text = None;
    }
}

impl Element {
    /// Appends a child element.
    pub fn push(&mut self, child: Element) {
        self.children.push(child);
    }

    /// Returns `true` if the element has any children.
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }

    /// Returns the number of children.
    pub fn num_children(&self) -> usize {
        self.children.len()
    }

    /// Returns `true` if the element is empty (no children, no text, no PI).
    pub fn is_empty(&self) -> bool {
        self.children.is_empty() && self.text.is_none() && self.pi.is_empty()
    }

    /// Clears all children.
    pub fn clear_children(&mut self) {
        self.children.clear();
    }

    pub fn get_child<P: ElementPredicate>(&self, predicate: P) -> Option<&Element> {
        self.children.iter().find(|c| predicate.match_element(c))
    }

    /// Finds the first child matching the predicate (mutable).
    pub fn get_child_mut<P: ElementPredicate>(&mut self, predicate: P) -> Option<&mut Element> {
        self.children.iter_mut().find(|c| predicate.match_element(c))
    }

    /// Returns all children matching the predicate.
    pub fn get_children<P: ElementPredicate>(&self, predicate: P) -> Vec<&Element> {
        self.children
            .iter()
            .filter(|c| predicate.match_element(c))
            .collect()
    }

    pub fn take_child<P: ElementPredicate>(&mut self, predicate: P) -> Option<Element> {
        let pos = self.children.iter().position(|c| predicate.match_element(c))?;
        Some(self.children.remove(pos))
    }

    /// Removes the first matching child (`take_child` alias).
    pub fn remove_child<P: ElementPredicate>(&mut self, predicate: P) -> Option<Element> {
        self.take_child(predicate)
    }
}

pub struct Descendants<'a> {
    stack: Vec<&'a Element>,
}

impl<'a> Descendants<'a> {
    fn new(root: &'a Element) -> Self {
        Descendants { stack: vec![root] }
    }
}

impl<'a> Iterator for Descendants<'a> {
    type Item = &'a Element;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        // Push children in reverse so they pop in natural order
        for child in node.children.iter().rev() {
            self.stack.push(child);
        }
        Some(node)
    }
}

unsafe fn collect_mut_ptr<'a>(
    elem: *mut Element,
    result: &mut Vec<&'a mut Element>,
) {
    // SAFETY: elem is guaranteed non-null and uniquely borrowed by the caller.
    // rust_2024_compatibility requires explicit unsafe blocks inside unsafe fns.
    unsafe {
        result.push(&mut *elem);
        for child in (*elem).children.iter_mut() {
            collect_mut_ptr(child as *mut Element, result);
        }
    }
}

impl Element {
    pub fn descendants(&self) -> Descendants<'_> {
        Descendants::new(self)
    }

    pub fn descendants_mut(&mut self) -> Vec<&mut Element> {
        let mut result = Vec::new();
        unsafe { collect_mut_ptr(self as *mut Element, &mut result) };
        result
    }

    pub fn find(&self, path: &str) -> Option<&Element> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = self;
        for part in &parts {
            current = current.get_child(*part)?;
        }
        Some(current)
    }

    /// Finds a descendant by path (mutable).
    pub fn find_mut(&mut self, path: &str) -> Option<&mut Element> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = self;
        for part in &parts {
            current = current.get_child_mut(*part)?;
        }
        Some(current)
    }
}

impl Element {

    pub fn matches<P: ElementPredicate>(&self, predicate: P) -> bool {
        predicate.match_element(self)
    }
}


impl Element {

    pub fn write<W: Write>(&self, writer: W) -> Result<(), Error> {
        let options = SerializeOptions::default();
        serializer::to_writer(writer, self, &options)
    }


    pub fn write_with_config<W: Write>(
        &self,
        writer: W,
        options: &SerializeOptions,
    ) -> Result<(), Error> {
        serializer::to_writer(writer, self, options)
    }
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match crate::serializer::to_string(self) {
            Ok(xml) => f.write_str(&xml),
            Err(e) => write!(f, "<!-- serialization error: {} -->", e),
        }
    }
}


impl Element {
    pub fn from_str(xml: &str) -> Result<Element, Error> {
        Element::from_reader(xml.as_bytes(), &ParseOptions::default())
    }

    /// Parses a byte slice into an `Element` tree.
    pub fn from_bytes(bytes: &[u8]) -> Result<Element, Error> {
        Element::from_reader(bytes, &ParseOptions::default())
    }

    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Element, Error> {
        let bytes = std::fs::read(path)?;
        Element::from_bytes(&bytes)
    }

    pub fn from_reader<R: std::io::Read>(
        reader: R,
        options: &ParseOptions,
    ) -> Result<Element, Error> {
        parser::parse(std::io::BufReader::new(reader), options)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Builder pattern
    // -----------------------------------------------------------------------

    #[test]
    fn test_builder_with_attr() {
        let elem = Element::new("root")
            .with_attr("id", "1")
            .with_attr("name", "test");
        assert_eq!(elem.get_attr("id"), Some("1"));
        assert_eq!(elem.get_attr("name"), Some("test"));
    }

    #[test]
    fn test_builder_with_child() {
        let elem = Element::new("root")
            .with_child(Element::new("a"))
            .with_child(Element::new("b"));
        assert_eq!(elem.children.len(), 2);
    }

    #[test]
    fn test_builder_with_text() {
        let elem = Element::new("root").with_text("hello");
        assert_eq!(elem.get_text(), Some("hello"));
    }

    #[test]
    fn test_builder_full_chain() {
        let elem = Element::new("root")
            .with_attr("xmlns", "urn:test")
            .with_child(Element::new("child").with_text("data"))
            .with_namespace("urn:test");
        assert_eq!(elem.name, "root");
        assert_eq!(elem.get_attr("xmlns"), Some("urn:test"));
        assert_eq!(elem.get_child("child").unwrap().get_text(), Some("data"));
    }

    // -----------------------------------------------------------------------
    // take_child
    // -----------------------------------------------------------------------

    #[test]
    fn test_take_child_removes_and_returns() {
        let mut parent = Element::new("parent");
        parent.push(Element::new("child"));
        parent.push(Element::new("other"));

        let taken = parent.take_child("child").expect("child should be found");
        assert_eq!(taken.name, "child");
        assert_eq!(parent.children.len(), 1);
        assert_eq!(parent.children[0].name, "other");
    }

    #[test]
    fn test_take_child_not_found() {
        let mut parent = Element::new("parent");
        assert!(parent.take_child("nonexistent").is_none());
    }

    // -----------------------------------------------------------------------
    // is_empty / has_children / has_text
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_empty() {
        let elem = Element::new("empty");
        assert!(elem.is_empty());
    }

    #[test]
    fn test_has_children() {
        let mut elem = Element::new("root");
        assert!(!elem.has_children());
        elem.push(Element::new("child"));
        assert!(elem.has_children());
    }

    #[test]
    fn test_has_text() {
        let mut elem = Element::new("root");
        assert!(!elem.has_text());
        elem.set_text("content");
        assert!(elem.has_text());
    }

    #[test]
    fn test_num_children() {
        let mut elem = Element::new("root");
        assert_eq!(elem.num_children(), 0);
        elem.push(Element::new("a"));
        elem.push(Element::new("b"));
        assert_eq!(elem.num_children(), 2);
    }

    // -----------------------------------------------------------------------
    // descendants
    // -----------------------------------------------------------------------

    #[test]
    fn test_descendants_flat() {
        let mut root = Element::new("root");
        root.push(Element::new("a"));
        root.push(Element::new("b"));
        root.push(Element::new("c"));

        let names: Vec<&str> = root.descendants().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["root", "a", "b", "c"]);
    }

    #[test]
    fn test_descendants_nested() {
        let mut root = Element::new("root");
        let mut child = Element::new("child");
        child.push(Element::new("grandchild"));
        root.push(child);

        let names: Vec<&str> = root.descendants().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["root", "child", "grandchild"]);
    }

    #[test]
    fn test_descendants_mut_modify() {
        let mut root = Element::new("root");
        root.push(Element::new("a"));
        root.push(Element::new("b"));

        for elem in root.descendants_mut() {
            if elem.name == "a" {
                elem.name = "modified".into();
            }
        }

        assert_eq!(root.get_child("modified").unwrap().name, "modified");
        assert!(root.get_child("a").is_none());
    }

    // -----------------------------------------------------------------------
    // find
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_single_level() {
        let mut root = Element::new("root");
        root.push(Element::new("child"));
        assert!(root.find("child").is_some());
    }

    #[test]
    fn test_find_nested() {
        let mut root = Element::new("root");
        let mut child = Element::new("child");
        child.push(Element::new("grandchild"));
        root.push(child);

        let found = root.find("child/grandchild");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "grandchild");
    }

    #[test]
    fn test_find_not_found() {
        let root = Element::new("root");
        assert!(root.find("nonexistent").is_none());
    }

    #[test]
    fn test_find_mut() {
        let mut root = Element::new("root");
        root.push(Element::new("target"));

        let found = root.find_mut("target");
        assert!(found.is_some());
        found.unwrap().set_attr("found", "yes");

        assert_eq!(root.get_child("target").unwrap().get_attr("found"), Some("yes"));
    }

    // -----------------------------------------------------------------------
    // matches / ElementPredicate
    // -----------------------------------------------------------------------

    #[test]
    fn test_matches_by_name() {
        let elem = Element::new("foo");
        assert!(elem.matches("foo"));
        assert!(!elem.matches("bar"));
    }

    #[test]
    fn test_element_predicate_via_element_method() {
        let elem = Element::new("test");
        assert!(elem.matches("test"));
        assert!(!elem.matches("other"));
    }

    #[test]
    fn test_element_predicate_tuple_namespace() {
        let mut elem = Element::new("foo");
        elem.namespace = Some("urn:bar".into());
        assert!(elem.matches(("foo", "urn:bar")));
        assert!(!elem.matches(("foo", "urn:baz")));
        assert!(!elem.matches(("bar", "urn:bar")));
    }

    // -----------------------------------------------------------------------
    // get_child with predicate
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_child_with_tuple_predicate() {
        let mut root = Element::new("root");
        let mut child = Element::new("child");
        child.namespace = Some("urn:ns".into());
        root.push(child);

        assert!(root.get_child(("child", "urn:ns")).is_some());
        assert!(root.get_child(("child", "urn:wrong")).is_none());
    }

    // -----------------------------------------------------------------------
    // get_children with predicate
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_children_matching() {
        let mut root = Element::new("root");
        root.push(Element::new("item"));
        root.push(Element::new("other"));
        root.push(Element::new("item"));

        assert_eq!(root.get_children("item").len(), 2);
        assert_eq!(root.get_children("other").len(), 1);
    }

    // -----------------------------------------------------------------------
    // remove_attr / clear_attributes / num_attrs / iter_attrs
    // -----------------------------------------------------------------------

    #[test]
    fn test_remove_attr() {
        let mut elem = Element::new("root").with_attr("key", "val");
        assert_eq!(elem.remove_attr("key"), Some("val".into()));
        assert!(elem.get_attr("key").is_none());
    }

    #[test]
    fn test_clear_attributes() {
        let mut elem = Element::new("root")
            .with_attr("a", "1")
            .with_attr("b", "2");
        elem.clear_attributes();
        assert_eq!(elem.num_attrs(), 0);
    }

    #[test]
    fn test_num_attrs() {
        let elem = Element::new("root")
            .with_attr("a", "1")
            .with_attr("b", "2");
        assert_eq!(elem.num_attrs(), 2);
    }

    #[test]
    fn test_iter_attrs() {
        let elem = Element::new("root")
            .with_attr("x", "10")
            .with_attr("y", "20");
        let mut count = 0;
        for (k, v) in elem.iter_attrs() {
            assert!(!k.is_empty());
            assert!(!v.is_empty());
            count += 1;
        }
        assert_eq!(count, 2);
    }

    // -----------------------------------------------------------------------
    // take_text / clear_text
    // -----------------------------------------------------------------------

    #[test]
    fn test_take_text() {
        let mut elem = Element::new("root").with_text("hello");
        assert_eq!(elem.take_text(), Some("hello".into()));
        assert!(elem.get_text().is_none());
    }

    #[test]
    fn test_clear_text() {
        let mut elem = Element::new("root").with_text("hello");
        elem.clear_text();
        assert!(elem.get_text().is_none());
    }

    // -----------------------------------------------------------------------
    // clear_children
    // -----------------------------------------------------------------------

    #[test]
    fn test_clear_children() {
        let mut root = Element::new("root");
        root.push(Element::new("a"));
        root.push(Element::new("b"));
        root.clear_children();
        assert!(!root.has_children());
    }

    // -----------------------------------------------------------------------
    // write / write_with_config
    // -----------------------------------------------------------------------

    #[test]
    fn test_write_to_vec() {
        let elem = Element::new("root");
        let mut buf = Vec::new();
        elem.write(&mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "<root/>");
    }

    #[test]
    fn test_write_with_config() {
        let elem = Element::new("root");
        let mut buf = Vec::new();
        let opts = SerializeOptions::new().no_indent();
        elem.write_with_config(&mut buf, &opts).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "<root/>");
    }

    // -----------------------------------------------------------------------
    // Round-trip: parse → modify → serialize → re-parse
    // -----------------------------------------------------------------------

    #[test]
    fn test_roundtrip_simple() {
        let xml = "<root><item id=\"1\">hello</item></root>";
        let elem = Element::from_str(xml).expect("should parse");
        let serialized = elem.to_string();
        let reparsed = Element::from_str(&serialized).expect("should re-parse");

        assert_eq!(reparsed.name, "root");
        assert_eq!(reparsed.get_child("item").unwrap().get_attr("id"), Some("1"));
        assert_eq!(reparsed.get_child("item").unwrap().get_text(), Some("hello"));
    }

    #[test]
    fn test_roundtrip_modify_and_reserialize() {
        let xml = "<root><item>old</item></root>";
        let mut elem = Element::from_str(xml).expect("should parse");

        elem.set_attr("version", "2");
        if let Some(item) = elem.get_child_mut("item") {
            item.set_text("new");
        }
        elem.push(Element::new("extra").with_attr("flag", "true"));

        let serialized = elem.to_string();
        assert!(serialized.contains(r#"version="2""#));
        assert!(serialized.contains("<item>new</item>"));
        assert!(serialized.contains(r#"<extra flag="true"/>"#));

        let reparsed = Element::from_str(&serialized).unwrap();
        assert_eq!(reparsed.get_attr("version"), Some("2"));
        assert_eq!(reparsed.get_child("item").unwrap().get_text(), Some("new"));
        assert!(reparsed.get_child("extra").is_some());
    }

    #[test]
    fn test_roundtrip_xml_declaration_skipped() {
        let xml = r#"<?xml version="1.0"?><root><child/></root>"#;
        let elem = Element::from_str(xml).expect("should parse XML with declaration");
        let serialized = elem.to_string();

        assert!(!serialized.starts_with("<?xml"));

        let reparsed = Element::from_str(&serialized).unwrap();
        assert_eq!(reparsed.name, "root");
        assert!(reparsed.get_child("child").is_some());
    }

    #[test]
    fn test_roundtrip_attributes_escaping() {
        let mut elem = Element::new("root");
        elem.set_attr("data", "a & b < c > d \"quoted\"");
        let serialized = elem.to_string();
        assert!(serialized.contains("&amp;"));
        assert!(serialized.contains("&lt;"));
        assert!(serialized.contains("&gt;"));
        assert!(serialized.contains("&quot;"));

        let reparsed = Element::from_str(&serialized).unwrap();
        assert_eq!(reparsed.get_attr("data"), Some("a & b < c > d \"quoted\""));
    }

    #[test]
    fn test_roundtrip_no_indent() {
        let xml = "<root><child>text</child></root>";
        let elem = Element::from_str(xml).unwrap();

        let mut buf = Vec::new();
        let opts = SerializeOptions::new().no_indent();
        elem.write_with_config(&mut buf, &opts).unwrap();
        let compact = String::from_utf8(buf).unwrap();

        assert!(!compact.contains('\n'));
        let reparsed = Element::from_str(&compact).unwrap();
        assert_eq!(reparsed.get_child("child").unwrap().get_text(), Some("text"));
    }

    // -----------------------------------------------------------------------
    // Complex document round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn test_roundtrip_complex_document() {
        let xml = r#"<?xml version="1.0"?>
<library>
  <book id="1">
    <title>Rust Programming</title>
    <author>John Doe</author>
  </book>
  <book id="2">
    <title>Systems Programming</title>
    <author>Jane Smith</author>
  </book>
</library>"#;

        let elem = Element::from_str(xml).expect("should parse complex document");

        for book in elem.get_children("book") {
            assert!(book.has_attr("id"));
        }

        assert!(elem.find("book/title").is_some());
        assert_eq!(
            elem.find("book/title").unwrap().get_text(),
            Some("Rust Programming")
        );

        let serialized = elem.to_string();
        let reparsed = Element::from_str(&serialized).unwrap();
        assert_eq!(reparsed.get_children("book").len(), 2);
        assert_eq!(
            reparsed.find("book/title").unwrap().get_text(),
            Some("Rust Programming")
        );
        assert_eq!(
            reparsed.find("book/author").unwrap().get_text(),
            Some("John Doe")
        );
    }

    // -----------------------------------------------------------------------
    // from_bytes round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_bytes_roundtrip() {
        let xml = b"<root><child attr=\"val\"/></root>";
        let elem = Element::from_bytes(xml).unwrap();
        assert_eq!(elem.name, "root");
        assert!(elem.get_child("child").unwrap().has_attr("attr"));
    }

    // -----------------------------------------------------------------------
    // Display trait usage
    // -----------------------------------------------------------------------

    #[test]
    fn test_display_trait() {
        let elem = Element::new("root").with_text("hello");
        let display_str = format!("{}", elem);
        assert_eq!(display_str, "<root>hello</root>");
    }

    #[test]
    fn test_display_roundtrip() {
        let xml = "<root><child>text</child></root>";
        let elem = Element::from_str(xml).unwrap();
        let display_str = elem.to_string();

        let reparsed = Element::from_str(&display_str).unwrap();
        assert_eq!(reparsed.get_child("child").unwrap().get_text(), Some("text"));
    }
}

//! Source-backed, non-recursive XML document tree.

use std::borrow::Cow;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::ops::Range;
use std::sync::Arc;

use aster_forge_utils::numbers::{u32_to_usize, u64_to_usize, usize_to_u32, usize_to_u64};
use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};

use crate::{Error, ParseOptions, XmlSafetyError, XmlSafetyPolicy};

const OWNED_VALUE_OFFSET: u64 = u64::MAX;
const XML_NAMESPACE_URI: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE_URI: &str = "http://www.w3.org/2000/xmlns/";

/// Stable identifier for a node in an [`XmlDocument`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(NonZeroU32);

impl NodeId {
    fn from_index(index: usize) -> Result<Self, Error> {
        let value = usize_to_u32(index, "XML node index")
            .ok()
            .and_then(|index| index.checked_add(1))
            .and_then(NonZeroU32::new)
            .ok_or(XmlSafetyError::TooManyElements)?;
        Ok(Self(value))
    }

    fn index(self) -> usize {
        stored_index(self.0.get() - 1, "XML node index")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ScopeId(NonZeroU32);

impl ScopeId {
    fn from_index(index: usize) -> Result<Self, Error> {
        let value = usize_to_u32(index, "XML namespace scope index")
            .ok()
            .and_then(|index| index.checked_add(1))
            .and_then(NonZeroU32::new)
            .ok_or_else(|| Error::InvalidXml("too many namespace scopes".into()))?;
        Ok(Self(value))
    }

    fn index(self) -> usize {
        stored_index(self.0.get() - 1, "XML namespace scope index")
    }
}

/// A byte range in the original XML source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: u64,
    pub end: u64,
}

impl SourceSpan {
    fn as_range(self, source_len: usize) -> Option<Range<usize>> {
        let start = u64_to_usize(self.start, "XML source span start").ok()?;
        let end = u64_to_usize(self.end, "XML source span end").ok()?;
        (start <= end && end <= source_len).then_some(start..end)
    }
}

#[derive(Debug, Clone, Copy)]
struct ValueRef {
    offset: u64,
    length: u32,
    owned_index: u32,
}

impl ValueRef {
    fn source(offset: u64, length: u32) -> Self {
        Self {
            offset,
            length,
            owned_index: 0,
        }
    }

    fn owned(index: u32, length: u32) -> Self {
        Self {
            offset: OWNED_VALUE_OFFSET,
            length,
            owned_index: index,
        }
    }
}

#[derive(Debug)]
struct ArenaNode {
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    next_sibling: Option<NodeId>,
    kind: NodeKind,
}

#[derive(Debug)]
enum NodeKind {
    Element(ElementData),
    Text(ValueRef),
    CData(ValueRef),
    Comment(ValueRef),
    ProcessingInstruction {
        target: ValueRef,
        content: Option<ValueRef>,
    },
}

#[derive(Debug)]
struct ElementData {
    qualified_name: ValueRef,
    attributes: Range<u32>,
    namespace_scope: Option<ScopeId>,
    source: SourceSpan,
}

#[derive(Debug)]
struct AttributeData {
    qualified_name: ValueRef,
    value: ValueRef,
}

#[derive(Debug)]
struct NamespaceScope {
    parent: Option<ScopeId>,
    bindings: Range<u32>,
}

#[derive(Debug)]
struct NamespaceBinding {
    prefix: ValueRef,
    uri: Option<ValueRef>,
}

/// An immutable XML tree whose nodes reference ranges in `source` whenever possible.
///
/// `S` may be `&[u8]`, `Arc<[u8]>`, `Vec<u8>`, or another byte container.
#[derive(Debug)]
pub struct XmlDocument<S> {
    source: S,
    nodes: Box<[ArenaNode]>,
    attributes: Box<[AttributeData]>,
    namespace_scopes: Box<[NamespaceScope]>,
    namespace_bindings: Box<[NamespaceBinding]>,
    owned_values: Box<[Box<str>]>,
    root: NodeId,
}

/// A document borrowing its complete source buffer.
pub type BorrowedDocument<'a> = XmlDocument<&'a [u8]>;

/// A document sharing ownership of its source buffer.
pub type OwnedDocument = XmlDocument<Arc<[u8]>>;

impl<S: AsRef<[u8]>> XmlDocument<S> {
    /// Parses a complete XML document with the default bounded policy.
    pub fn parse(source: S) -> Result<Self, Error> {
        Self::parse_with_options(source, &ParseOptions::default())
    }

    /// Parses a complete XML document into a flat arena.
    pub fn parse_with_options(source: S, options: &ParseOptions) -> Result<Self, Error> {
        options.safety.validate()?;
        if source.as_ref().len() > options.safety.max_input_bytes {
            return Err(XmlSafetyError::InputTooLarge.into());
        }

        let (nodes, attributes, namespace_scopes, namespace_bindings, owned_values, root) = {
            let mut builder = DocumentBuilder::new(source.as_ref(), options);
            builder.parse()?;
            let root = builder.root.ok_or(XmlSafetyError::Malformed)?;
            (
                builder.nodes.into_boxed_slice(),
                builder.attributes.into_boxed_slice(),
                builder.namespace_scopes.into_boxed_slice(),
                builder.namespace_bindings.into_boxed_slice(),
                builder.owned_values.into_boxed_slice(),
                root,
            )
        };
        Ok(Self {
            source,
            nodes,
            attributes,
            namespace_scopes,
            namespace_bindings,
            owned_values,
            root,
        })
    }

    pub fn source(&self) -> &[u8] {
        self.source.as_ref()
    }

    pub fn into_source(self) -> S {
        self.source
    }

    pub fn root(&self) -> ElementRef<'_, S> {
        ElementRef {
            document: self,
            id: self.root,
        }
    }

    pub fn node(&self, id: NodeId) -> Option<NodeRef<'_, S>> {
        let node = self.nodes.get(id.index())?;
        Some(match &node.kind {
            NodeKind::Element(_) => NodeRef::Element(ElementRef { document: self, id }),
            NodeKind::Text(value) => NodeRef::Text(self.value(*value)),
            NodeKind::CData(value) => NodeRef::CData(self.value(*value)),
            NodeKind::Comment(value) => NodeRef::Comment(self.value(*value)),
            NodeKind::ProcessingInstruction { target, content } => {
                NodeRef::ProcessingInstruction(ProcessingInstructionRef {
                    target: self.value(*target),
                    content: content.map(|value| self.value(value)),
                })
            }
        })
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn allocated_value_count(&self) -> usize {
        self.owned_values.len()
    }

    pub fn write_original<W: Write>(&self, mut writer: W) -> Result<(), Error> {
        writer.write_all(self.source())?;
        Ok(())
    }

    fn value(&self, value: ValueRef) -> &str {
        if value.offset == OWNED_VALUE_OFFSET {
            return &self.owned_values[stored_index(value.owned_index, "owned XML value index")];
        }
        let start = u64_to_usize(value.offset, "XML value offset").unwrap_or(0);
        let end = start + stored_index(value.length, "XML value length");
        // Values are validated as UTF-8 when their ValueRef is created.
        std::str::from_utf8(&self.source.as_ref()[start..end]).unwrap_or("")
    }

    fn element_data(&self, id: NodeId) -> Option<&ElementData> {
        match &self.nodes.get(id.index())?.kind {
            NodeKind::Element(element) => Some(element),
            _ => None,
        }
    }

    fn resolve_namespace(&self, mut scope: Option<ScopeId>, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE_URI);
        }
        while let Some(scope_id) = scope {
            let scope_data = &self.namespace_scopes[scope_id.index()];
            for binding_index in scope_data.bindings.clone().rev() {
                let binding = &self.namespace_bindings
                    [stored_index(binding_index, "XML namespace binding index")];
                if self.value(binding.prefix) == prefix {
                    return binding.uri.map(|uri| self.value(uri));
                }
            }
            scope = scope_data.parent;
        }
        None
    }
}

impl XmlDocument<Arc<[u8]>> {
    /// Reads and parses a complete document with the default bounded policy.
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, Error> {
        Self::from_reader_with_options(reader, &ParseOptions::default())
    }

    /// Reads at most one byte beyond the configured limit before parsing an owned document.
    pub fn from_reader_with_options<R: Read>(
        reader: R,
        options: &ParseOptions,
    ) -> Result<Self, Error> {
        options.safety.validate()?;
        let read_limit = options.safety.max_input_bytes.saturating_add(1);
        let read_limit = usize_to_u64(read_limit, "XML reader byte limit").unwrap_or(u64::MAX);
        let mut reader = reader.take(read_limit);
        let mut source = Vec::new();
        reader.read_to_end(&mut source)?;
        if source.len() > options.safety.max_input_bytes {
            return Err(XmlSafetyError::InputTooLarge.into());
        }
        Self::parse_with_options(Arc::from(source), options)
    }
}

/// A cheap-to-clone, validated XML document retaining the exact original bytes.
#[derive(Debug, Clone)]
pub struct ValidatedXml(Arc<OwnedDocument>);

impl ValidatedXml {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Result<Self, Error> {
        Self::with_policy(bytes, XmlSafetyPolicy::untrusted())
    }

    pub fn with_policy(
        bytes: impl Into<Arc<[u8]>>,
        policy: XmlSafetyPolicy,
    ) -> Result<Self, Error> {
        let source = bytes.into();
        let document =
            XmlDocument::parse_with_options(source, &ParseOptions::new().safety_policy(policy))?;
        Ok(Self(Arc::new(document)))
    }

    pub fn from_reader<R: Read>(reader: R) -> Result<Self, Error> {
        let document = OwnedDocument::from_reader(reader)?;
        Ok(Self(Arc::new(document)))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.source()
    }

    pub fn document(&self) -> &OwnedDocument {
        &self.0
    }
}

/// A borrowed view of an element node.
pub struct ElementRef<'document, S> {
    document: &'document XmlDocument<S>,
    id: NodeId,
}

impl<S> Copy for ElementRef<'_, S> {}

impl<S> Clone for ElementRef<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'document, S: AsRef<[u8]>> ElementRef<'document, S> {
    pub fn id(self) -> NodeId {
        self.id
    }

    pub fn parent(self) -> Option<ElementRef<'document, S>> {
        self.document.nodes[self.id.index()]
            .parent
            .map(|id| ElementRef {
                document: self.document,
                id,
            })
    }

    pub fn qualified_name(self) -> &'document str {
        let Some(data) = self.document.element_data(self.id) else {
            return "";
        };
        self.document.value(data.qualified_name)
    }

    pub fn prefix(self) -> Option<&'document str> {
        split_qualified_name(self.qualified_name()).0
    }

    pub fn name(self) -> &'document str {
        split_qualified_name(self.qualified_name()).1
    }

    pub fn namespace(self) -> Option<&'document str> {
        let data = self.document.element_data(self.id)?;
        self.document
            .resolve_namespace(data.namespace_scope, self.prefix().unwrap_or(""))
    }

    pub fn raw_xml(self) -> &'document [u8] {
        let Some(data) = self.document.element_data(self.id) else {
            return &[];
        };
        let Some(range) = data.source.as_range(self.document.source().len()) else {
            return &[];
        };
        &self.document.source()[range]
    }

    pub fn attributes(self) -> Attributes<'document, S> {
        let range = self
            .document
            .element_data(self.id)
            .map(|data| data.attributes.clone())
            .unwrap_or(0..0);
        Attributes {
            element: self,
            next: range.start,
            end: range.end,
        }
    }

    pub fn attribute(self, qualified_name: &str) -> Option<&'document str> {
        self.attributes()
            .find(|attribute| attribute.qualified_name() == qualified_name)
            .map(AttributeRef::value)
    }

    pub fn attribute_ns(self, name: &str, namespace: Option<&str>) -> Option<&'document str> {
        self.attributes()
            .find(|attribute| attribute.name() == name && attribute.namespace() == namespace)
            .map(AttributeRef::value)
    }

    pub fn children(self) -> Children<'document, S> {
        Children {
            document: self.document,
            next: self.document.nodes[self.id.index()].first_child,
        }
    }

    pub fn child_elements(self) -> ChildElements<'document, S> {
        ChildElements {
            children: self.children(),
        }
    }

    pub fn get_child(self, name: &str) -> Option<ElementRef<'document, S>> {
        self.child_elements().find(|element| element.name() == name)
    }

    pub fn get_child_ns(self, name: &str, namespace: &str) -> Option<ElementRef<'document, S>> {
        self.child_elements()
            .find(|element| element.name() == name && element.namespace() == Some(namespace))
    }

    pub fn descendants(self) -> DescendantElements<'document, S> {
        DescendantElements { stack: vec![self] }
    }

    pub fn text(self) -> Option<Cow<'document, str>> {
        let mut values = self.children().filter_map(|node| match node {
            NodeRef::Text(text) | NodeRef::CData(text) => Some(text),
            _ => None,
        });
        let first = values.next()?;
        match values.next() {
            None => Some(Cow::Borrowed(first)),
            Some(second) => {
                let mut output = String::with_capacity(first.len() + second.len());
                output.push_str(first);
                output.push_str(second);
                values.for_each(|value| output.push_str(value));
                Some(Cow::Owned(output))
            }
        }
    }
}

/// A borrowed XML node view.
pub enum NodeRef<'document, S> {
    Element(ElementRef<'document, S>),
    Text(&'document str),
    CData(&'document str),
    Comment(&'document str),
    ProcessingInstruction(ProcessingInstructionRef<'document>),
}

impl<S> Copy for NodeRef<'_, S> {}

impl<S> Clone for NodeRef<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessingInstructionRef<'a> {
    pub target: &'a str,
    pub content: Option<&'a str>,
}

pub struct Children<'document, S> {
    document: &'document XmlDocument<S>,
    next: Option<NodeId>,
}

impl<'document, S: AsRef<[u8]>> Iterator for Children<'document, S> {
    type Item = NodeRef<'document, S>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.next?;
        self.next = self.document.nodes[id.index()].next_sibling;
        self.document.node(id)
    }
}

pub struct ChildElements<'document, S> {
    children: Children<'document, S>,
}

impl<'document, S: AsRef<[u8]>> Iterator for ChildElements<'document, S> {
    type Item = ElementRef<'document, S>;

    fn next(&mut self) -> Option<Self::Item> {
        self.children.find_map(|node| match node {
            NodeRef::Element(element) => Some(element),
            _ => None,
        })
    }
}

pub struct DescendantElements<'document, S> {
    stack: Vec<ElementRef<'document, S>>,
}

impl<'document, S: AsRef<[u8]>> Iterator for DescendantElements<'document, S> {
    type Item = ElementRef<'document, S>;

    fn next(&mut self) -> Option<Self::Item> {
        let element = self.stack.pop()?;
        let children: Vec<_> = element.child_elements().collect();
        self.stack.extend(children.into_iter().rev());
        Some(element)
    }
}

pub struct Attributes<'document, S> {
    element: ElementRef<'document, S>,
    next: u32,
    end: u32,
}

impl<'document, S: AsRef<[u8]>> Iterator for Attributes<'document, S> {
    type Item = AttributeRef<'document, S>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        self.next += 1;
        Some(AttributeRef {
            element: self.element,
            index,
        })
    }
}

pub struct AttributeRef<'document, S> {
    element: ElementRef<'document, S>,
    index: u32,
}

impl<S> Copy for AttributeRef<'_, S> {}

impl<S> Clone for AttributeRef<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'document, S: AsRef<[u8]>> AttributeRef<'document, S> {
    fn data(self) -> &'document AttributeData {
        &self.document().attributes[stored_index(self.index, "XML attribute index")]
    }

    fn document(self) -> &'document XmlDocument<S> {
        self.element.document
    }

    pub fn qualified_name(self) -> &'document str {
        self.document().value(self.data().qualified_name)
    }

    pub fn prefix(self) -> Option<&'document str> {
        split_qualified_name(self.qualified_name()).0
    }

    pub fn name(self) -> &'document str {
        split_qualified_name(self.qualified_name()).1
    }

    pub fn namespace(self) -> Option<&'document str> {
        let prefix = self.prefix()?;
        let scope = self
            .document()
            .element_data(self.element.id)
            .and_then(|element| element.namespace_scope);
        self.document().resolve_namespace(scope, prefix)
    }

    pub fn value(self) -> &'document str {
        self.document().value(self.data().value)
    }
}

struct DocumentBuilder<'a> {
    source: &'a [u8],
    options: &'a ParseOptions,
    nodes: Vec<ArenaNode>,
    attributes: Vec<AttributeData>,
    namespace_scopes: Vec<NamespaceScope>,
    namespace_bindings: Vec<NamespaceBinding>,
    owned_values: Vec<Box<str>>,
    open: Vec<NodeId>,
    root: Option<NodeId>,
    root_complete: bool,
    element_count: usize,
    text_bytes: usize,
    event_count: usize,
}

impl<'a> DocumentBuilder<'a> {
    fn new(source: &'a [u8], options: &'a ParseOptions) -> Self {
        Self {
            source,
            options,
            nodes: Vec::new(),
            attributes: Vec::new(),
            namespace_scopes: Vec::new(),
            namespace_bindings: Vec::new(),
            owned_values: Vec::new(),
            open: Vec::new(),
            root: None,
            root_complete: false,
            element_count: 0,
            text_bytes: 0,
            event_count: 0,
        }
    }

    fn parse(&mut self) -> Result<(), Error> {
        let mut reader = Reader::from_reader(self.source);
        reader.config_mut().trim_text(false);
        loop {
            let event_start = reader.buffer_position();
            let event = reader.read_event().map_err(map_quick_xml_error)?;
            let event_end = reader.buffer_position();
            if !matches!(event, Event::Eof) {
                self.count_event()?;
            }
            match event {
                Event::Start(start) => {
                    self.start_element(&reader, &start, event_start, event_end)?
                }
                Event::Empty(start) => {
                    self.empty_element(&reader, &start, event_start, event_end)?
                }
                Event::End(_) => self.end_element(event_end)?,
                Event::Text(text) => {
                    let raw = utf8(text.as_ref())?;
                    let value =
                        unescape(raw).map_err(|error| Error::InvalidXml(error.to_string()))?;
                    self.text_node(value, false)?;
                }
                Event::CData(text) => self.text_node(Cow::Borrowed(utf8(text.as_ref())?), true)?,
                Event::Comment(comment) => {
                    let value = self.source_value(utf8(comment.as_ref())?)?;
                    self.push_content(NodeKind::Comment(value))?;
                }
                Event::PI(pi) => {
                    let target = self.source_value(utf8(pi.target())?)?;
                    let content = utf8(pi.content())?
                        .trim_start_matches(|character: char| character.is_ascii_whitespace());
                    let content = (!content.is_empty())
                        .then(|| self.source_value(content))
                        .transpose()?;
                    self.push_content(NodeKind::ProcessingInstruction { target, content })?;
                }
                Event::GeneralRef(reference) => {
                    let value = if let Some(character) = reference
                        .resolve_char_ref()
                        .map_err(|error| Error::InvalidXml(error.to_string()))?
                    {
                        Cow::Owned(character.to_string())
                    } else {
                        Cow::Owned(
                            match utf8(reference.as_ref())? {
                                "amp" => "&",
                                "lt" => "<",
                                "gt" => ">",
                                "apos" => "'",
                                "quot" => "\"",
                                _ => return Err(XmlSafetyError::ExternalEntity.into()),
                            }
                            .to_owned(),
                        )
                    };
                    self.text_node(value, false)?;
                }
                Event::Decl(_) => {
                    if self.root.is_some() || !self.open.is_empty() || self.root_complete {
                        return Err(XmlSafetyError::Malformed.into());
                    }
                }
                Event::DocType(_) => {
                    if self.options.safety.reject_doctype {
                        return Err(XmlSafetyError::ExternalEntity.into());
                    }
                    if self.root.is_some() || !self.open.is_empty() || self.root_complete {
                        return Err(XmlSafetyError::Malformed.into());
                    }
                }
                Event::Eof => {
                    if !self.open.is_empty() || !self.root_complete {
                        return Err(XmlSafetyError::Malformed.into());
                    }
                    return Ok(());
                }
            }
        }
    }

    fn start_element(
        &mut self,
        reader: &Reader<&[u8]>,
        start: &BytesStart<'a>,
        source_start: u64,
        source_end: u64,
    ) -> Result<(), Error> {
        self.check_element()?;
        let id = self.build_element(reader, start, source_start, source_end)?;
        self.open.push(id);
        Ok(())
    }

    fn empty_element(
        &mut self,
        reader: &Reader<&[u8]>,
        start: &BytesStart<'a>,
        source_start: u64,
        source_end: u64,
    ) -> Result<(), Error> {
        self.check_element()?;
        self.build_element(reader, start, source_start, source_end)?;
        if self.open.is_empty() {
            self.root_complete = true;
        }
        Ok(())
    }

    fn end_element(&mut self, source_end: u64) -> Result<(), Error> {
        let id = self.open.pop().ok_or(XmlSafetyError::Malformed)?;
        let NodeKind::Element(element) = &mut self.nodes[id.index()].kind else {
            return Err(XmlSafetyError::Malformed.into());
        };
        element.source.end = source_end;
        if self.open.is_empty() {
            self.root_complete = true;
        }
        Ok(())
    }

    fn build_element(
        &mut self,
        reader: &Reader<&[u8]>,
        start: &BytesStart<'a>,
        source_start: u64,
        source_end: u64,
    ) -> Result<NodeId, Error> {
        if self.open.is_empty() && self.root_complete {
            return Err(XmlSafetyError::Malformed.into());
        }
        let start_name = start.name();
        let qualified_name = utf8(start_name.as_ref())?;
        let (prefix, _) = validate_qualified_name(qualified_name)?;
        let parent_scope = self.open.last().and_then(|id| {
            let NodeKind::Element(element) = &self.nodes[id.index()].kind else {
                return None;
            };
            element.namespace_scope
        });
        let binding_start = arena_len(self.namespace_bindings.len(), "namespace bindings")?;
        let mut attribute_count = 0usize;
        for attribute in start.attributes() {
            attribute_count = attribute_count
                .checked_add(1)
                .ok_or(XmlSafetyError::TooManyAttributes)?;
            if attribute_count > self.options.safety.max_attributes_per_element {
                return Err(XmlSafetyError::TooManyAttributes.into());
            }
            let attribute = attribute.map_err(|error| Error::InvalidXml(error.to_string()))?;
            let name = utf8(attribute.key.as_ref())?;
            if name == "xmlns" || name.starts_with("xmlns:") {
                let namespace_prefix = name.strip_prefix("xmlns:").unwrap_or("");
                let uri = attribute
                    .decoded_and_normalized_value(XmlVersion::Explicit1_0, reader.decoder())
                    .map_err(|error| Error::InvalidXml(error.to_string()))?;
                validate_namespace_binding(namespace_prefix, &uri)?;
                let prefix_value = if namespace_prefix.is_empty() {
                    ValueRef::source(0, 0)
                } else {
                    self.source_value(namespace_prefix)?
                };
                let uri_value = if uri.is_empty() {
                    None
                } else {
                    Some(self.cow_value(uri)?)
                };
                self.namespace_bindings.push(NamespaceBinding {
                    prefix: prefix_value,
                    uri: uri_value,
                });
            }
        }
        let binding_end = arena_len(self.namespace_bindings.len(), "namespace bindings")?;
        let namespace_scope = if binding_start == binding_end {
            parent_scope
        } else {
            let id = ScopeId::from_index(self.namespace_scopes.len())?;
            self.namespace_scopes.push(NamespaceScope {
                parent: parent_scope,
                bindings: binding_start..binding_end,
            });
            Some(id)
        };
        if let Some(prefix) = prefix
            && self.resolve_namespace(namespace_scope, prefix).is_none()
        {
            return Err(XmlSafetyError::Malformed.into());
        }

        let attribute_start = arena_len(self.attributes.len(), "attributes")?;
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|error| Error::InvalidXml(error.to_string()))?;
            let name = utf8(attribute.key.as_ref())?;
            if name == "xmlns" || name.starts_with("xmlns:") {
                continue;
            }
            let (prefix, _) = validate_qualified_name(name)?;
            if let Some(prefix) = prefix
                && prefix != "xml"
                && self.resolve_namespace(namespace_scope, prefix).is_none()
            {
                return Err(XmlSafetyError::Malformed.into());
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Explicit1_0, reader.decoder())
                .map_err(|error| Error::InvalidXml(error.to_string()))?;
            let qualified_name = self.source_value(name)?;
            let value = self.cow_value(value)?;
            self.attributes.push(AttributeData {
                qualified_name,
                value,
            });
        }
        let attribute_end = arena_len(self.attributes.len(), "attributes")?;
        let qualified_name = self.source_value(qualified_name)?;
        let id = self.push_node(NodeKind::Element(ElementData {
            qualified_name,
            attributes: attribute_start..attribute_end,
            namespace_scope,
            source: SourceSpan {
                start: source_start,
                end: source_end,
            },
        }))?;
        if self.root.is_none() {
            self.root = Some(id);
        }
        Ok(id)
    }

    fn check_element(&mut self) -> Result<(), Error> {
        let depth = self
            .open
            .len()
            .checked_add(1)
            .ok_or(XmlSafetyError::TooDeep)?;
        if depth > self.options.safety.max_depth {
            return Err(XmlSafetyError::TooDeep.into());
        }
        self.element_count = self
            .element_count
            .checked_add(1)
            .ok_or(XmlSafetyError::TooManyElements)?;
        if self.element_count > self.options.safety.max_elements {
            return Err(XmlSafetyError::TooManyElements.into());
        }
        Ok(())
    }

    fn count_event(&mut self) -> Result<(), Error> {
        self.event_count = self
            .event_count
            .checked_add(1)
            .ok_or(XmlSafetyError::TooManyEvents)?;
        if self.event_count > self.options.safety.max_events {
            return Err(XmlSafetyError::TooManyEvents.into());
        }
        Ok(())
    }

    fn text_node(&mut self, value: Cow<'_, str>, cdata: bool) -> Result<(), Error> {
        let value = if self.options.trim_whitespace {
            match value {
                Cow::Borrowed(value) => Cow::Borrowed(value.trim()),
                Cow::Owned(value) => Cow::Owned(value.trim().to_owned()),
            }
        } else {
            value
        };
        if value.is_empty() {
            return Ok(());
        }
        self.text_bytes = self
            .text_bytes
            .checked_add(value.len())
            .ok_or(XmlSafetyError::TextTooLarge)?;
        if self.text_bytes > self.options.safety.max_text_bytes {
            return Err(XmlSafetyError::TextTooLarge.into());
        }
        if self.open.is_empty() {
            if value.chars().all(char::is_whitespace) {
                return Ok(());
            }
            return Err(XmlSafetyError::Malformed.into());
        }
        let value = self.cow_value(value)?;
        self.push_content(if cdata {
            NodeKind::CData(value)
        } else {
            NodeKind::Text(value)
        })?;
        Ok(())
    }

    fn push_content(&mut self, kind: NodeKind) -> Result<(), Error> {
        if self.open.is_empty() {
            return Ok(());
        }
        self.push_node(kind).map(|_| ())
    }

    fn push_node(&mut self, kind: NodeKind) -> Result<NodeId, Error> {
        let id = NodeId::from_index(self.nodes.len())?;
        let parent = self.open.last().copied();
        self.nodes.push(ArenaNode {
            parent,
            first_child: None,
            last_child: None,
            next_sibling: None,
            kind,
        });
        if let Some(parent) = parent {
            if let Some(previous) = self.nodes[parent.index()].last_child {
                self.nodes[previous.index()].next_sibling = Some(id);
            } else {
                self.nodes[parent.index()].first_child = Some(id);
            }
            self.nodes[parent.index()].last_child = Some(id);
        }
        Ok(id)
    }

    fn source_value(&self, value: &str) -> Result<ValueRef, Error> {
        let source_start = self.source.as_ptr() as usize;
        let value_start = value.as_ptr() as usize;
        let offset = value_start
            .checked_sub(source_start)
            .filter(|offset| offset.saturating_add(value.len()) <= self.source.len())
            .ok_or_else(|| Error::InvalidXml("borrowed value is outside XML source".into()))?;
        Ok(ValueRef::source(
            usize_to_u64(offset, "XML value offset").map_err(|_| XmlSafetyError::InputTooLarge)?,
            usize_to_u32(value.len(), "XML value length")
                .map_err(|_| XmlSafetyError::InputTooLarge)?,
        ))
    }

    fn cow_value(&mut self, value: Cow<'_, str>) -> Result<ValueRef, Error> {
        match value {
            Cow::Borrowed(value) => self.source_value(value),
            Cow::Owned(value) => {
                let index = arena_len(self.owned_values.len(), "owned XML values")?;
                let length = usize_to_u32(value.len(), "owned XML value length")
                    .map_err(|_| XmlSafetyError::InputTooLarge)?;
                self.owned_values.push(value.into_boxed_str());
                Ok(ValueRef::owned(index, length))
            }
        }
    }

    fn resolve_namespace(&self, mut scope: Option<ScopeId>, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE_URI);
        }
        while let Some(scope_id) = scope {
            let scope_data = &self.namespace_scopes[scope_id.index()];
            for binding_index in scope_data.bindings.clone().rev() {
                let binding = &self.namespace_bindings
                    [stored_index(binding_index, "XML namespace binding index")];
                if self.builder_value(binding.prefix) == prefix {
                    return binding.uri.map(|uri| self.builder_value(uri));
                }
            }
            scope = scope_data.parent;
        }
        None
    }

    fn builder_value(&self, value: ValueRef) -> &str {
        if value.offset == OWNED_VALUE_OFFSET {
            &self.owned_values[stored_index(value.owned_index, "owned XML value index")]
        } else {
            let start = u64_to_usize(value.offset, "XML value offset").unwrap_or(0);
            let end = start + stored_index(value.length, "XML value length");
            std::str::from_utf8(&self.source[start..end]).unwrap_or("")
        }
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|_| XmlSafetyError::InvalidEncoding.into())
}

fn validate_qualified_name(name: &str) -> Result<(Option<&str>, &str), Error> {
    let (prefix, local) = split_qualified_name(name);
    if !valid_name(local) || prefix.is_some_and(|prefix| !valid_name(prefix)) {
        return Err(XmlSafetyError::Malformed.into());
    }
    if name.matches(':').count() > 1 {
        return Err(XmlSafetyError::Malformed.into());
    }
    Ok((prefix, local))
}

fn split_qualified_name(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, name),
    }
}

fn valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(is_name_start) && characters.all(is_name_char)
}

fn is_name_start(character: char) -> bool {
    matches!(
        character,
        'A'..='Z'
            | '_'
            | 'a'..='z'
            | '\u{00C0}'..='\u{00D6}'
            | '\u{00D8}'..='\u{00F6}'
            | '\u{00F8}'..='\u{02FF}'
            | '\u{0370}'..='\u{037D}'
            | '\u{037F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || character.is_ascii_digit()
        || matches!(character, '-' | '.' | '\u{B7}')
        || ('\u{300}'..='\u{36F}').contains(&character)
        || ('\u{203F}'..='\u{2040}').contains(&character)
}

fn validate_namespace_binding(prefix: &str, uri: &str) -> Result<(), Error> {
    if prefix == "xmlns"
        || uri == XMLNS_NAMESPACE_URI
        || (prefix == "xml" && uri != XML_NAMESPACE_URI)
        || (prefix != "xml" && uri == XML_NAMESPACE_URI)
        || (!prefix.is_empty() && uri.is_empty())
    {
        Err(XmlSafetyError::Malformed.into())
    } else {
        Ok(())
    }
}

fn arena_len(length: usize, label: &str) -> Result<u32, Error> {
    usize_to_u32(length, label).map_err(|_| Error::InvalidXml(format!("too many {label}")))
}

fn stored_index(value: u32, label: &str) -> usize {
    // Rust's supported platforms can represent every u32 as usize. Keep the checked Forge
    // conversion at the representation boundary and make malformed internal state fail indexing.
    u32_to_usize(value, label).unwrap_or(usize::MAX)
}

fn map_quick_xml_error(error: quick_xml::Error) -> Error {
    match error {
        quick_xml::Error::Encoding(_) => XmlSafetyError::InvalidEncoding.into(),
        error => Error::InvalidXml(error.to_string()),
    }
}

#[cfg(test)]
mod layout_tests {
    use std::mem::size_of_val;

    use super::*;

    fn arena_payload_bytes<S>(document: &XmlDocument<S>) -> usize {
        size_of_val(document.nodes.as_ref())
            + size_of_val(document.attributes.as_ref())
            + size_of_val(document.namespace_scopes.as_ref())
            + size_of_val(document.namespace_bindings.as_ref())
            + size_of_val(document.owned_values.as_ref())
            + document
                .owned_values
                .iter()
                .map(|value| value.len())
                .sum::<usize>()
    }

    #[test]
    fn large_owned_document_payload_stays_below_six_times_input() {
        const RESPONSES: usize = 10_000;
        let mut source = String::from("<D:multistatus xmlns:D=\"DAV:\">");
        for index in 0..RESPONSES {
            source.push_str(&format!(
                "<D:response><D:href>/files/{index}</D:href><D:propstat><D:prop><D:displayname>file-{index}</D:displayname><D:getcontentlength>{}</D:getcontentlength><D:getetag>&quot;etag-{index}&quot;</D:getetag></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
                index * 1024
            ));
        }
        source.push_str("</D:multistatus>");
        let input_bytes = source.len();
        let options = ParseOptions::new()
            .max_size(input_bytes)
            .max_elements(RESPONSES * 8 + 1);
        let document = XmlDocument::parse_with_options(Arc::from(source.into_bytes()), &options)
            .expect("large document");
        let retained_payload = input_bytes + arena_payload_bytes(&document);

        assert!(
            retained_payload <= input_bytes * 6,
            "retained payload {retained_payload} exceeds 6x input {input_bytes}"
        );
    }
}

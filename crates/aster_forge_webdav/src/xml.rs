//! `WebDAV` XML grammar and representation boundary.
//!
//! The concrete XML implementation is intentionally private to this module. Products consume
//! WebDAV-specific request models and [`DavXmlElement`] instead of depending on an XML crate.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use aster_forge_xml::{
    BorrowedDocument, ElementRef, Error as ForgeXmlError, NodeRef, OwnedDocument, ParseOptions,
    XmlSafetyError, XmlSafetyPolicy, XmlStreamWriter, XmlWriteAttribute, is_valid_xml_local_name,
};

use crate::deltav::{DavExpandPropertySelection, DavParsedReport};

const DAV_NAMESPACE: &str = "DAV:";

/// XML failure returned by the `WebDAV` grammar boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavXmlError {
    /// The document declares a DTD or entity.
    #[error("XML external entity declarations are not allowed")]
    ExternalEntity,
    /// The document exceeds the configured nesting depth.
    #[error("XML nesting depth exceeds the configured limit")]
    TooDeep,
    /// The request XML exceeds an input or decoded-text size limit.
    #[error("XML input exceeds the configured size limit")]
    TooLarge,
    /// The document is malformed or is not a single-root document.
    #[error("malformed XML input")]
    Malformed,
    /// The document is well-formed XML but violates the method grammar.
    #[error("invalid WebDAV XML grammar")]
    InvalidGrammar,
}

impl From<XmlSafetyError> for DavXmlError {
    fn from(error: XmlSafetyError) -> Self {
        match error {
            XmlSafetyError::ExternalEntity => Self::ExternalEntity,
            XmlSafetyError::TooDeep => Self::TooDeep,
            XmlSafetyError::InputTooLarge | XmlSafetyError::TextTooLarge => Self::TooLarge,
            XmlSafetyError::InvalidPolicy
            | XmlSafetyError::OutputTooLarge
            | XmlSafetyError::TooManyElements
            | XmlSafetyError::TooManyAttributes
            | XmlSafetyError::TooManyEvents
            | XmlSafetyError::InvalidEncoding
            | XmlSafetyError::Malformed => Self::Malformed,
        }
    }
}

/// XML content owned by the `WebDAV` boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavXmlNode {
    /// Child element.
    Element(DavXmlElement),
    /// Escaped character data.
    Text(String),
    /// CDATA content.
    CData(String),
    /// Comment content.
    Comment(String),
    /// Processing instruction.
    ProcessingInstruction(String, Option<String>),
}

/// Owned DAV element used for persisted subtrees and response composition.
///
/// Known request grammars traverse the source-backed `aster_forge_xml` arena directly and only
/// materialize the owner or property subtrees that must cross the backend boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavXmlElement {
    /// Local element name.
    pub name: String,
    /// Lexical prefix, when present.
    pub prefix: Option<String>,
    /// Resolved namespace URI, when present.
    pub namespace: Option<String>,
    /// In-scope namespace declarations keyed by prefix; an empty key is the default namespace.
    pub namespaces: BTreeMap<String, String>,
    /// Element attributes in their lexical form.
    pub attributes: BTreeMap<String, String>,
    /// Ordered child content.
    pub children: Vec<DavXmlNode>,
}

impl DavXmlElement {
    /// Creates an element from a lexical `QName` such as `D:href`.
    #[must_use]
    pub fn new(name: &str) -> Self {
        let (prefix, local_name) = name
            .split_once(':')
            .map_or((None, name), |(prefix, local)| {
                (Some(prefix.to_owned()), local)
            });
        Self {
            name: local_name.to_owned(),
            prefix,
            namespace: None,
            namespaces: BTreeMap::new(),
            attributes: BTreeMap::new(),
            children: Vec::new(),
        }
    }

    /// Creates a `DAV:` element using the conventional `D` prefix.
    #[must_use]
    pub fn dav(local_name: &str) -> Self {
        let mut element = Self::new(&format!("D:{local_name}"));
        element.namespace = Some(DAV_NAMESPACE.to_owned());
        element
    }

    /// Parses one bounded XML element.
    ///
    /// # Errors
    ///
    /// Returns [`DavXmlError`] when the bounded XML element is unsafe, malformed, or invalid.
    pub fn parse(bytes: &[u8]) -> Result<Self, DavXmlError> {
        parse_element(bytes)
    }

    /// Parses one bounded XML element from a reader.
    ///
    /// # Errors
    ///
    /// Returns [`DavXmlError`] when reader input is unsafe, malformed, or invalid XML.
    pub fn parse_reader(reader: impl Read) -> Result<Self, DavXmlError> {
        let options = webdav_parse_options();
        let document = OwnedDocument::from_reader_with_options(reader, &options)
            .map_err(|error| map_forge_xml_error(&error))?;
        Ok(element_from_forge(document.root()))
    }

    /// Serializes the element as UTF-8 XML bytes.
    ///
    /// # Errors
    ///
    /// Returns [`DavXmlError`] when the element cannot be serialized as valid XML.
    pub fn to_bytes(&self) -> Result<Vec<u8>, DavXmlError> {
        let mut writer =
            XmlStreamWriter::new(Vec::new()).map_err(|error| map_forge_xml_error(&error))?;
        write_element(&mut writer, self, &BTreeMap::new())
            .map_err(|error| map_forge_xml_error(&error))?;
        writer.finish().map_err(|error| map_forge_xml_error(&error))
    }

    /// Iterates over direct child elements while ignoring text, comments, and CDATA.
    pub fn child_elements(&self) -> impl Iterator<Item = &Self> {
        self.children.iter().filter_map(|child| match child {
            DavXmlNode::Element(element) => Some(element),
            DavXmlNode::Text(_)
            | DavXmlNode::CData(_)
            | DavXmlNode::Comment(_)
            | DavXmlNode::ProcessingInstruction(_, _) => None,
        })
    }

    /// Returns concatenated direct text and CDATA content.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let text = self
            .children
            .iter()
            .filter_map(|child| match child {
                DavXmlNode::Text(text) | DavXmlNode::CData(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        (!text.is_empty()).then_some(text)
    }
}

/// Property name selected by PROPFIND.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavRequestedProperty {
    /// Local property name.
    pub name: String,
    /// Resolved namespace URI.
    pub namespace: Option<String>,
    /// Client-supplied lexical prefix.
    pub prefix: Option<String>,
}

/// Parsed PROPFIND request selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavPropfindRequest {
    /// All live/dead properties plus optional explicit properties.
    AllProp {
        /// Additional requested properties.
        include: Vec<DavRequestedProperty>,
    },
    /// Property names without values.
    PropName,
    /// Explicit property selection.
    Prop(Vec<DavRequestedProperty>),
}

/// One ordered PROPPATCH operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavPropertyPatchRequest {
    /// Whether the operation sets rather than removes the property.
    pub set: bool,
    /// Property value/name.
    pub property: DavPropertyPatchValue,
}

/// Validated property element carried by PROPPATCH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavPropertyPatchValue {
    /// Local property name.
    pub name: String,
    /// Resolved namespace URI.
    pub namespace: Option<String>,
    /// Lexical prefix.
    pub prefix: Option<String>,
    /// Standalone validated element, including inherited `xml:lang` when needed.
    pub element: DavXmlElement,
}

/// Parsed LOCK creation body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavLockRequestBody {
    /// Whether the requested lock scope is shared.
    pub shared: bool,
    /// Optional owner element, preserved for discovery and persistence.
    pub owner: Option<DavXmlElement>,
}

/// Parses a PROPFIND body. An absent body selects `allprop`.
///
/// # Errors
///
/// Returns [`DavXmlError`] when the body is unsafe or violates PROPFIND grammar.
pub fn parse_propfind_request(body: &[u8]) -> Result<DavPropfindRequest, DavXmlError> {
    if body.is_empty() {
        return Ok(DavPropfindRequest::AllProp {
            include: Vec::new(),
        });
    }
    let document = parse_document(body)?;
    let root = document.root();
    if !is_dav_element(root, "propfind") {
        return Err(DavXmlError::InvalidGrammar);
    }
    require_element_content(root)?;

    let mut kind = None;
    let mut include = Vec::new();
    let mut include_seen = false;
    for child in root.child_elements() {
        if is_dav_element(child, "propname") {
            if kind.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            require_element_content(child)?;
            kind = Some(DavPropfindRequest::PropName);
        } else if is_dav_element(child, "allprop") {
            if kind.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            require_element_content(child)?;
            kind = Some(DavPropfindRequest::AllProp {
                include: Vec::new(),
            });
        } else if is_dav_element(child, "include") {
            if include_seen {
                return Err(DavXmlError::InvalidGrammar);
            }
            include_seen = true;
            require_property_names(child)?;
            include.extend(child.child_elements().map(requested_property));
        } else if is_dav_element(child, "prop") {
            if kind.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            require_property_names(child)?;
            kind = Some(DavPropfindRequest::Prop(
                child.child_elements().map(requested_property).collect(),
            ));
        }
    }

    match kind {
        Some(DavPropfindRequest::AllProp { .. }) => Ok(DavPropfindRequest::AllProp { include }),
        Some(kind) if !include_seen => Ok(kind),
        _ => Err(DavXmlError::InvalidGrammar),
    }
}

/// Parses an ordered PROPPATCH request.
///
/// # Errors
///
/// Returns [`DavXmlError`] when the body is unsafe or violates PROPPATCH grammar.
pub fn parse_proppatch_request(body: &[u8]) -> Result<Vec<DavPropertyPatchRequest>, DavXmlError> {
    let document = parse_document(body)?;
    let root = document.root();
    if !is_dav_element(root, "propertyupdate") {
        return Err(DavXmlError::InvalidGrammar);
    }
    require_element_content(root)?;
    let root_lang = xml_lang_value(root).map(str::to_owned);
    let mut patches = Vec::new();
    for action in root.child_elements() {
        let set = if is_dav_element(action, "set") {
            true
        } else if is_dav_element(action, "remove") {
            false
        } else {
            // RFC 4918 section 17: unknown extension elements are ignored with their subtree.
            continue;
        };
        require_element_content(action)?;
        let action_lang = xml_lang_value(action).or(root_lang.as_deref());
        let prop_container =
            unique_dav_child(action, "prop")?.ok_or(DavXmlError::InvalidGrammar)?;
        require_element_content(prop_container)?;
        let container_lang = xml_lang_value(prop_container).or(action_lang);
        for property in prop_container.child_elements() {
            if !set {
                require_property_name(property)?;
            }
            let mut element = element_from_forge(property);
            let inherited_lang = xml_lang_value(property).or(container_lang);
            if let Some(lang) = inherited_lang.filter(|lang| !lang.is_empty()) {
                element
                    .attributes
                    .entry("xml:lang".to_owned())
                    .or_insert_with(|| lang.to_owned());
            }
            patches.push(DavPropertyPatchRequest {
                set,
                property: DavPropertyPatchValue {
                    name: element.name.clone(),
                    namespace: element.namespace.clone(),
                    prefix: element.prefix.clone(),
                    element,
                },
            });
        }
    }
    if patches.is_empty() {
        return Err(DavXmlError::InvalidGrammar);
    }
    Ok(patches)
}

/// Parses a LOCK creation body.
///
/// # Errors
///
/// Returns [`DavXmlError`] when the body is unsafe or violates LOCK creation grammar.
pub fn parse_lock_request(body: &[u8]) -> Result<DavLockRequestBody, DavXmlError> {
    let document = parse_document(body)?;
    let root = document.root();
    if !is_dav_element(root, "lockinfo") {
        return Err(DavXmlError::InvalidGrammar);
    }
    require_element_content(root)?;
    let mut shared = None;
    let mut write_lock = false;
    let mut owner = None;
    for child in root.child_elements() {
        if is_dav_element(child, "lockscope") {
            if shared.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            require_element_content(child)?;
            let exclusive_scope = unique_dav_child(child, "exclusive")?;
            let shared_scope = unique_dav_child(child, "shared")?;
            let (selected_scope, is_shared) = match (exclusive_scope, shared_scope) {
                (Some(scope), None) => (scope, false),
                (None, Some(scope)) => (scope, true),
                (Some(_), Some(_)) | (None, None) => return Err(DavXmlError::InvalidGrammar),
            };
            require_element_content(selected_scope)?;
            shared = Some(is_shared);
        } else if is_dav_element(child, "locktype") {
            if write_lock {
                return Err(DavXmlError::InvalidGrammar);
            }
            require_element_content(child)?;
            let write = unique_dav_child(child, "write")?.ok_or(DavXmlError::InvalidGrammar)?;
            require_element_content(write)?;
            write_lock = true;
        } else if is_dav_element(child, "owner") {
            if owner.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            owner = Some(element_from_forge(child));
        }
    }
    match (shared, write_lock) {
        (Some(shared), true) => Ok(DavLockRequestBody { shared, owner }),
        _ => Err(DavXmlError::InvalidGrammar),
    }
}

pub(crate) fn parse_report_request(
    body: &[u8],
    maximum_input_bytes: usize,
    maximum_xml_depth: usize,
    maximum_expansion_depth: usize,
    maximum_properties: usize,
) -> Result<DavParsedReport, DavXmlError> {
    let options = webdav_parse_options()
        .max_size(maximum_input_bytes)
        .max_depth(maximum_xml_depth);
    let document = parse_document_with_options(body, &options)?;
    let root = document.root();
    if is_dav_element(root, "version-tree") {
        return Ok(DavParsedReport::VersionTree(parse_version_tree_prop(root)?));
    }
    if is_dav_element(root, "expand-property") {
        return Ok(DavParsedReport::ExpandProperty(parse_expand_property(
            root,
            maximum_expansion_depth,
            maximum_properties,
        )?));
    }
    Ok(DavParsedReport::Other(requested_property(root)))
}

/// Validates an optional RFC 3253 VERSION-CONTROL request body.
pub(crate) fn parse_version_control_request(body: &[u8]) -> Result<(), DavXmlError> {
    if body.is_empty() {
        return Ok(());
    }
    let document = parse_document(body)?;
    if !is_dav_element(document.root(), "version-control") {
        return Err(DavXmlError::InvalidGrammar);
    }
    // RFC 3253 section 3.5 declares DAV:version-control as ANY. The complete document has
    // already passed the shared WebDAV safety policy, so extensions and mixed content are kept.
    Ok(())
}

fn parse_element(bytes: &[u8]) -> Result<DavXmlElement, DavXmlError> {
    let document = parse_document(bytes)?;
    Ok(element_from_forge(document.root()))
}

fn parse_document(bytes: &[u8]) -> Result<BorrowedDocument<'_>, DavXmlError> {
    // The Forge parser applies the WebDAV safety policy while building its source-backed arena.
    // A separate validator pass here would scan every request twice.
    BorrowedDocument::parse_with_options(bytes, &webdav_parse_options())
        .map_err(|error| map_forge_xml_error(&error))
}

fn parse_document_with_options<'a>(
    bytes: &'a [u8],
    options: &ParseOptions,
) -> Result<BorrowedDocument<'a>, DavXmlError> {
    BorrowedDocument::parse_with_options(bytes, options)
        .map_err(|error| map_forge_xml_error(&error))
}

fn is_dav_element<S: AsRef<[u8]>>(element: ElementRef<'_, S>, local_name: &str) -> bool {
    element.name() == local_name && element.namespace() == Some(DAV_NAMESPACE)
}

fn parse_version_tree_prop<S: AsRef<[u8]>>(
    root: ElementRef<'_, S>,
) -> Result<Option<Vec<DavRequestedProperty>>, DavXmlError> {
    require_element_content(root)?;
    if let Some(prop) = unique_dav_child(root, "prop")? {
        require_property_names(prop)?;
        return Ok(Some(
            prop.child_elements().map(requested_property).collect(),
        ));
    }
    Ok(None)
}

fn parse_expand_property<S: AsRef<[u8]>>(
    root: ElementRef<'_, S>,
    maximum_depth: usize,
    maximum_properties: usize,
) -> Result<Vec<DavExpandPropertySelection>, DavXmlError> {
    if maximum_depth == 0 || maximum_properties == 0 {
        return Err(DavXmlError::InvalidGrammar);
    }
    require_element_content(root)?;
    if root
        .child_elements()
        .any(|child| !is_dav_element(child, "property"))
    {
        return Err(DavXmlError::InvalidGrammar);
    }
    let mut count = 0usize;
    root.child_elements()
        .filter(|child| is_dav_element(*child, "property"))
        .map(|property| {
            parse_expand_property_selection(
                property,
                1,
                maximum_depth,
                maximum_properties,
                &mut count,
            )
        })
        .collect()
}

fn parse_expand_property_selection<S: AsRef<[u8]>>(
    property: ElementRef<'_, S>,
    depth: usize,
    maximum_depth: usize,
    maximum_properties: usize,
    count: &mut usize,
) -> Result<DavExpandPropertySelection, DavXmlError> {
    if depth > maximum_depth {
        return Err(DavXmlError::TooDeep);
    }
    *count = count.checked_add(1).ok_or(DavXmlError::TooLarge)?;
    if *count > maximum_properties {
        return Err(DavXmlError::TooLarge);
    }
    require_element_content(property)?;
    if property
        .child_elements()
        .any(|child| !is_dav_element(child, "property"))
    {
        return Err(DavXmlError::InvalidGrammar);
    }
    let name = property
        .attribute("name")
        .filter(|name| is_valid_xml_local_name(name))
        .ok_or(DavXmlError::InvalidGrammar)?;
    let namespace = property.attribute("namespace").unwrap_or(DAV_NAMESPACE);
    if namespace.is_empty() || namespace.trim() != namespace {
        return Err(DavXmlError::InvalidGrammar);
    }
    let nested = property
        .child_elements()
        .filter(|child| is_dav_element(*child, "property"))
        .map(|child| {
            parse_expand_property_selection(
                child,
                depth + 1,
                maximum_depth,
                maximum_properties,
                count,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DavExpandPropertySelection {
        property: DavRequestedProperty {
            name: name.to_owned(),
            namespace: Some(namespace.to_owned()),
            prefix: None,
        },
        nested,
    })
}

fn unique_dav_child<'document, S: AsRef<[u8]>>(
    parent: ElementRef<'document, S>,
    local_name: &str,
) -> Result<Option<ElementRef<'document, S>>, DavXmlError> {
    let mut selected = None;
    for child in parent.child_elements() {
        if is_dav_element(child, local_name) {
            if selected.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            selected = Some(child);
        }
    }
    Ok(selected)
}

fn require_element_content<S: AsRef<[u8]>>(element: ElementRef<'_, S>) -> Result<(), DavXmlError> {
    if element
        .children()
        .any(|child| matches!(child, NodeRef::Text(_) | NodeRef::CData(_)))
    {
        Err(DavXmlError::InvalidGrammar)
    } else {
        Ok(())
    }
}

fn require_property_names<S: AsRef<[u8]>>(container: ElementRef<'_, S>) -> Result<(), DavXmlError> {
    require_element_content(container)?;
    for property in container.child_elements() {
        require_property_name(property)?;
    }
    Ok(())
}

fn require_property_name<S: AsRef<[u8]>>(property: ElementRef<'_, S>) -> Result<(), DavXmlError> {
    // In a property-name context every child element is unrecognized and RFC 4918 section 17
    // removes its complete subtree from semantic processing. Direct character data would still
    // be a property value, which PROPFIND/REPORT selectors and PROPPATCH remove do not permit.
    require_element_content(property)
}

fn requested_property<S: AsRef<[u8]>>(element: ElementRef<'_, S>) -> DavRequestedProperty {
    DavRequestedProperty {
        name: element.name().to_owned(),
        namespace: element.namespace().map(str::to_owned),
        prefix: element.prefix().map(str::to_owned),
    }
}

fn xml_lang_value<S: AsRef<[u8]>>(element: ElementRef<'_, S>) -> Option<&str> {
    element.attribute("xml:lang")
}

fn webdav_parse_options() -> ParseOptions {
    // Preserve the established WebDAV XML boundary: formatting whitespace is ignored and retained
    // text is trimmed before WebDAV grammar evaluation or dead-property persistence.
    ParseOptions::new()
        .safety_policy(XmlSafetyPolicy::untrusted())
        .trim_whitespace(true)
}

fn map_forge_xml_error(error: &ForgeXmlError) -> DavXmlError {
    match error {
        ForgeXmlError::Safety(error) => (*error).into(),
        ForgeXmlError::InvalidXml(_) | ForgeXmlError::InvalidData(_) | ForgeXmlError::Io(_) => {
            DavXmlError::Malformed
        }
    }
}

fn element_from_forge<S: AsRef<[u8]>>(element: ElementRef<'_, S>) -> DavXmlElement {
    let mut namespaces = BTreeMap::new();
    match (element.prefix(), element.namespace()) {
        (Some(prefix), Some(namespace)) if prefix != "xml" => {
            namespaces.insert(prefix.to_owned(), namespace.to_owned());
        }
        (None, Some(namespace)) => {
            namespaces.insert(String::new(), namespace.to_owned());
        }
        // This owned subtree may later be embedded under a default namespace. Declaring the
        // empty namespace keeps an originally unqualified element unqualified.
        (None, None) => {
            namespaces.insert(String::new(), String::new());
        }
        _ => {}
    }

    let mut attributes = BTreeMap::new();
    for attribute in element.attributes() {
        if let (Some(prefix), Some(namespace)) = (attribute.prefix(), attribute.namespace())
            && prefix != "xml"
        {
            namespaces
                .entry(prefix.to_owned())
                .or_insert_with(|| namespace.to_owned());
        }
        attributes.insert(
            attribute.qualified_name().to_owned(),
            attribute.value().to_owned(),
        );
    }

    DavXmlElement {
        name: element.name().to_owned(),
        prefix: element.prefix().map(str::to_owned),
        namespace: element.namespace().map(str::to_owned),
        namespaces,
        attributes,
        children: element
            .children()
            .map(|child| match child {
                NodeRef::Element(element) => DavXmlNode::Element(element_from_forge(element)),
                NodeRef::Text(text) => DavXmlNode::Text(text.to_owned()),
                NodeRef::CData(text) => DavXmlNode::CData(text.to_owned()),
                NodeRef::Comment(text) => DavXmlNode::Comment(text.to_owned()),
                NodeRef::ProcessingInstruction(instruction) => DavXmlNode::ProcessingInstruction(
                    instruction.target.to_owned(),
                    instruction.content.map(str::to_owned),
                ),
            })
            .collect(),
    }
}

pub(crate) fn write_element<W: Write>(
    writer: &mut XmlStreamWriter<W>,
    element: &DavXmlElement,
    inherited_namespaces: &BTreeMap<String, String>,
) -> Result<(), ForgeXmlError> {
    let qualified_name = element.prefix.as_ref().map_or_else(
        || element.name.clone(),
        |prefix| format!("{prefix}:{}", element.name),
    );
    let mut namespaces = inherited_namespaces.clone();
    let mut attributes = BTreeMap::new();
    for (prefix, namespace) in &element.namespaces {
        if namespaces.get(prefix) != Some(namespace) {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            attributes.insert(name, namespace.clone());
        }
        namespaces.insert(prefix.clone(), namespace.clone());
    }
    attributes.extend(element.attributes.clone());
    for (name, namespace) in &attributes {
        if let Some(prefix) = namespace_declaration_prefix(name) {
            namespaces.insert(prefix.to_owned(), namespace.clone());
        }
    }

    if let Some(namespace) = &element.namespace {
        let prefix = element.prefix.as_deref().unwrap_or("");
        if namespaces.get(prefix).map(String::as_str) != Some(namespace) {
            let binding_name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            match attributes.get(&binding_name) {
                Some(binding) if binding != namespace => {
                    return Err(ForgeXmlError::InvalidData(
                        "conflicting XML namespace binding".to_owned(),
                    ));
                }
                Some(_) => {}
                None => {
                    attributes.insert(binding_name, namespace.clone());
                }
            }
            namespaces.insert(prefix.to_owned(), namespace.clone());
        }
    } else if element.prefix.is_none()
        && namespaces
            .get("")
            .is_some_and(|namespace| !namespace.is_empty())
    {
        match attributes.get("xmlns") {
            Some(namespace) if !namespace.is_empty() => {
                return Err(ForgeXmlError::InvalidData(
                    "conflicting XML default namespace binding".to_owned(),
                ));
            }
            Some(_) => {}
            None => {
                attributes.insert("xmlns".to_owned(), String::new());
            }
        }
        namespaces.insert(String::new(), String::new());
    }

    let write_attributes = attributes
        .iter()
        .map(|(name, value)| XmlWriteAttribute::new(name, value));
    if element.children.is_empty() {
        writer.empty_element(&qualified_name, write_attributes)?;
        return Ok(());
    }
    writer.start_element(&qualified_name, write_attributes)?;
    for child in &element.children {
        match child {
            DavXmlNode::Element(element) => write_element(writer, element, &namespaces)?,
            DavXmlNode::Text(text) => writer.text(text)?,
            DavXmlNode::CData(text) => writer.cdata(text)?,
            DavXmlNode::Comment(text) => writer.comment(text)?,
            DavXmlNode::ProcessingInstruction(target, content) => {
                writer.processing_instruction(target, content.as_deref())?;
            }
        }
    }
    writer.end_element()
}

fn namespace_declaration_prefix(name: &str) -> Option<&str> {
    if name == "xmlns" {
        Some("")
    } else {
        name.strip_prefix("xmlns:")
    }
}

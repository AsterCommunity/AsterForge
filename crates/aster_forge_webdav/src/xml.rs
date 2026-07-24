//! WebDAV XML grammar and representation boundary.
//!
//! The concrete XML implementation is intentionally private to this module. Products consume
//! WebDAV-specific request models and [`DavXmlElement`] instead of depending on an XML crate.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use aster_forge_utils::xml::{XmlSafetyError, XmlSafetyPolicy, validate_xml_input};
use xmltree::{Element, XMLNode};

const DAV_NAMESPACE: &str = "DAV:";

/// XML failure returned by the WebDAV grammar boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavXmlError {
    /// The document declares a DTD or entity.
    #[error("XML external entity declarations are not allowed")]
    ExternalEntity,
    /// The document exceeds the configured nesting depth.
    #[error("XML nesting depth exceeds the configured limit")]
    TooDeep,
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
            XmlSafetyError::Malformed | XmlSafetyError::InvalidPolicy => Self::Malformed,
        }
    }
}

/// XML content owned by the WebDAV boundary.
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

/// XML element whose concrete parser/serializer is private to AsterForge.
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
    /// Creates an element from a lexical QName such as `D:href`.
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
    pub fn parse(bytes: &[u8]) -> Result<Self, DavXmlError> {
        parse_element(bytes)
    }

    /// Parses one bounded XML element from a reader.
    pub fn parse_reader(mut reader: impl Read) -> Result<Self, DavXmlError> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|_| DavXmlError::Malformed)?;
        Self::parse(&bytes)
    }

    /// Serializes the element as UTF-8 XML bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, DavXmlError> {
        let mut bytes = Vec::new();
        element_to_xmltree(self)
            .write(&mut bytes)
            .map_err(|_| DavXmlError::Malformed)?;
        Ok(bytes)
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
pub fn parse_propfind_request(body: &[u8]) -> Result<DavPropfindRequest, DavXmlError> {
    if body.is_empty() {
        return Ok(DavPropfindRequest::AllProp {
            include: Vec::new(),
        });
    }
    let root = parse_element(body)?;
    if !is_dav_element(&root, "propfind") {
        return Err(DavXmlError::InvalidGrammar);
    }

    let mut kind = None;
    let mut include = Vec::new();
    let mut include_seen = false;
    for child in root.child_elements() {
        if is_dav_element(child, "propname") {
            if kind.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            kind = Some(DavPropfindRequest::PropName);
        } else if is_dav_element(child, "allprop") {
            if kind.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            kind = Some(DavPropfindRequest::AllProp {
                include: Vec::new(),
            });
        } else if is_dav_element(child, "include") {
            if include_seen {
                return Err(DavXmlError::InvalidGrammar);
            }
            include_seen = true;
            include.extend(child.child_elements().map(requested_property));
        } else if is_dav_element(child, "prop") {
            if kind.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
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
pub fn parse_proppatch_request(body: &[u8]) -> Result<Vec<DavPropertyPatchRequest>, DavXmlError> {
    let root = parse_element(body)?;
    if !is_dav_element(&root, "propertyupdate") {
        return Err(DavXmlError::InvalidGrammar);
    }
    let root_lang = xml_lang_value(&root).map(str::to_owned);
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
        let action_lang = xml_lang_value(action).or(root_lang.as_deref());
        let dav_children = action
            .child_elements()
            .filter(|child| child.namespace.as_deref() == Some(DAV_NAMESPACE))
            .collect::<Vec<_>>();
        if !matches!(dav_children.as_slice(), [child] if is_dav_element(child, "prop")) {
            return Err(DavXmlError::InvalidGrammar);
        }
        let prop_container = dav_children[0];
        let container_lang = xml_lang_value(prop_container).or(action_lang);
        for property in prop_container.child_elements() {
            let mut element = property.clone();
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
pub fn parse_lock_request(body: &[u8]) -> Result<DavLockRequestBody, DavXmlError> {
    let root = parse_element(body)?;
    if !is_dav_element(&root, "lockinfo") {
        return Err(DavXmlError::InvalidGrammar);
    }
    let mut shared = None;
    let mut write_lock = false;
    let mut owner = None;
    for child in root.child_elements() {
        if is_dav_element(child, "lockscope") {
            if shared.is_some() {
                return Err(DavXmlError::InvalidGrammar);
            }
            let children = child
                .child_elements()
                .filter(|scope| scope.namespace.as_deref() == Some(DAV_NAMESPACE))
                .collect::<Vec<_>>();
            shared = match children.as_slice() {
                [scope] if is_dav_element(scope, "exclusive") => Some(false),
                [scope] if is_dav_element(scope, "shared") => Some(true),
                _ => return Err(DavXmlError::InvalidGrammar),
            };
        } else if is_dav_element(child, "locktype") {
            if write_lock {
                return Err(DavXmlError::InvalidGrammar);
            }
            let children = child
                .child_elements()
                .filter(|kind| kind.namespace.as_deref() == Some(DAV_NAMESPACE))
                .collect::<Vec<_>>();
            if !matches!(children.as_slice(), [kind] if is_dav_element(kind, "write")) {
                return Err(DavXmlError::InvalidGrammar);
            }
            write_lock = true;
        } else if is_dav_element(child, "owner") && owner.replace(child.clone()).is_some() {
            return Err(DavXmlError::InvalidGrammar);
        }
    }
    match (shared, write_lock) {
        (Some(shared), true) => Ok(DavLockRequestBody { shared, owner }),
        _ => Err(DavXmlError::InvalidGrammar),
    }
}

/// Returns the QName of a bounded REPORT root.
pub fn parse_report_root(body: &[u8]) -> Result<DavRequestedProperty, DavXmlError> {
    let root = parse_element(body)?;
    Ok(requested_property(&root))
}

fn parse_element(bytes: &[u8]) -> Result<DavXmlElement, DavXmlError> {
    validate_xml_input(bytes, XmlSafetyPolicy::untrusted())?;
    Element::parse(Cursor::new(bytes))
        .map(element_from_xmltree)
        .map_err(|_| DavXmlError::Malformed)
}

fn is_dav_element(element: &DavXmlElement, local_name: &str) -> bool {
    element.name == local_name && element.namespace.as_deref() == Some(DAV_NAMESPACE)
}

fn requested_property(element: &DavXmlElement) -> DavRequestedProperty {
    DavRequestedProperty {
        name: element.name.clone(),
        namespace: element.namespace.clone(),
        prefix: element.prefix.clone(),
    }
}

fn xml_lang_value(element: &DavXmlElement) -> Option<&str> {
    element
        .attributes
        .get("xml:lang")
        .or_else(|| element.attributes.get("lang"))
        .map(String::as_str)
}

fn element_from_xmltree(element: Element) -> DavXmlElement {
    DavXmlElement {
        name: element.name,
        prefix: element.prefix,
        namespace: element.namespace,
        namespaces: element
            .namespaces
            .map(|namespaces| {
                namespaces
                    .iter()
                    .map(|(prefix, namespace)| (prefix.to_owned(), namespace.to_owned()))
                    .collect()
            })
            .unwrap_or_default(),
        attributes: element.attributes.into_iter().collect(),
        children: element
            .children
            .into_iter()
            .map(|child| match child {
                XMLNode::Element(element) => DavXmlNode::Element(element_from_xmltree(element)),
                XMLNode::Text(text) => DavXmlNode::Text(text),
                XMLNode::CData(text) => DavXmlNode::CData(text),
                XMLNode::Comment(text) => DavXmlNode::Comment(text),
                XMLNode::ProcessingInstruction(name, value) => {
                    DavXmlNode::ProcessingInstruction(name, value)
                }
            })
            .collect(),
    }
}

fn element_to_xmltree(element: &DavXmlElement) -> Element {
    let mut result = Element::new(&element.name);
    result.prefix.clone_from(&element.prefix);
    result.namespace.clone_from(&element.namespace);
    if !element.namespaces.is_empty() {
        let mut namespaces = xmltree::Namespace::empty();
        for (prefix, namespace) in &element.namespaces {
            namespaces.force_put(prefix.clone(), namespace.clone());
        }
        result.namespaces = Some(namespaces);
    }
    result.attributes.extend(element.attributes.clone());
    result.children = element
        .children
        .iter()
        .map(|child| match child {
            DavXmlNode::Element(element) => XMLNode::Element(element_to_xmltree(element)),
            DavXmlNode::Text(text) => XMLNode::Text(text.clone()),
            DavXmlNode::CData(text) => XMLNode::CData(text.clone()),
            DavXmlNode::Comment(text) => XMLNode::Comment(text.clone()),
            DavXmlNode::ProcessingInstruction(name, value) => {
                XMLNode::ProcessingInstruction(name.clone(), value.clone())
            }
        })
        .collect();
    result
}

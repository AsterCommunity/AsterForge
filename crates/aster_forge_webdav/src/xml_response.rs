//! Product-neutral WebDAV XML response grammar.

use std::time::Duration;

use http::StatusCode;

use crate::{DavRequestedProperty, DavXmlElement, DavXmlNode, encode_href};

/// One `<D:propstat>` group in a multistatus response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavPropStat {
    /// HTTP status applying to every property in this group.
    pub status: u16,
    /// Ordered property elements.
    pub properties: Vec<DavXmlElement>,
}

/// WebDAV precondition or postcondition carried inside `<D:error>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavErrorCondition {
    /// RFC 4918 `no-external-entities` precondition.
    NoExternalEntities,
    /// RFC 4918 `lock-token-submitted` precondition.
    LockTokenSubmitted {
        /// Encoded resource href whose token must be submitted.
        href: String,
    },
    /// RFC 4918 `lock-token-matches-request-uri` precondition.
    LockTokenMatchesRequestUri,
    /// RFC 4918 `propfind-finite-depth` precondition.
    PropfindFiniteDepth,
}

/// One `<D:response>` entry in a multistatus response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavMultiStatusItem {
    /// Encoded resource href.
    pub href: String,
    /// Resource-wide status, used by COPY/MOVE/DELETE failures.
    pub status: Option<u16>,
    /// Property status groups, used by PROPFIND and PROPPATCH.
    pub propstats: Vec<DavPropStat>,
    /// Optional WebDAV condition accompanying the resource status.
    pub error: Option<DavErrorCondition>,
}

impl DavMultiStatusItem {
    /// Creates a property response entry.
    #[must_use]
    pub fn properties(href: impl Into<String>, propstats: Vec<DavPropStat>) -> Self {
        Self {
            href: href.into(),
            status: None,
            propstats,
            error: None,
        }
    }

    /// Creates a resource-wide status response entry.
    #[must_use]
    pub fn status(href: impl Into<String>, status: u16) -> Self {
        Self {
            href: href.into(),
            status: Some(status),
            propstats: Vec::new(),
            error: None,
        }
    }

    /// Attaches a WebDAV error condition.
    #[must_use]
    pub fn with_error(mut self, error: DavErrorCondition) -> Self {
        self.error = Some(error);
        self
    }
}

/// Protocol-visible lock values used to render `lockdiscovery`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavLockXml {
    /// Raw lock token; the XML writer percent-encodes it for the href.
    pub token: String,
    /// Optional validated owner element.
    pub owner: Option<DavXmlElement>,
    /// Negotiated timeout, or `None` for `Infinite`.
    pub timeout: Option<Duration>,
    /// Whether the lock is shared rather than exclusive.
    pub shared: bool,
    /// Whether the lock depth is infinity rather than zero.
    pub deep: bool,
    /// Encoded href of the lock root.
    pub root_href: String,
}

/// One version entry in a DeltaV version-tree response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavVersionXml {
    /// Encoded version href.
    pub href: String,
    /// Protocol-visible version name.
    pub version_name: String,
    /// Creator display name.
    pub creator: String,
    /// Content length.
    pub content_length: i64,
    /// HTTP-date formatted modification time.
    pub last_modified: String,
}

/// Creates a `DAV:` element using the conventional `D` prefix.
#[must_use]
pub fn dav_element(local_name: &str) -> DavXmlElement {
    DavXmlElement::dav(local_name)
}

/// Creates a `DAV:` element containing one text node.
#[must_use]
pub fn dav_text_element(local_name: &str, text: impl Into<String>) -> DavXmlElement {
    let mut element = dav_element(local_name);
    element.children.push(DavXmlNode::Text(text.into()));
    element
}

/// Creates an empty property element using its expanded name and preferred prefix.
#[must_use]
pub fn dav_property_name_element(name: &DavRequestedProperty) -> DavXmlElement {
    property_element(name, None)
}

/// Creates a property element containing one text node.
#[must_use]
pub fn dav_property_text_element(
    name: &DavRequestedProperty,
    text: impl Into<String>,
) -> DavXmlElement {
    property_element(name, Some(DavXmlNode::Text(text.into())))
}

/// Creates a property element containing one child element.
#[must_use]
pub fn dav_property_child_element(
    name: &DavRequestedProperty,
    child: DavXmlElement,
) -> DavXmlElement {
    property_element(name, Some(DavXmlNode::Element(child)))
}

/// Reconstructs a persisted dead property under the requested lexical QName.
///
/// Matching validated XML contributes its attributes and children. Legacy malformed or
/// mismatched values are emitted as escaped text instead of response markup.
#[must_use]
pub fn dav_dead_property_element(
    stored_name: &DavRequestedProperty,
    requested_name: Option<&DavRequestedProperty>,
    stored_xml: Option<&[u8]>,
) -> DavXmlElement {
    let output_name = requested_name.unwrap_or(stored_name);
    let mut output = property_element(output_name, None);
    let Some(stored_xml) = stored_xml.filter(|xml| !xml.is_empty()) else {
        return output;
    };
    if let Ok(stored) = DavXmlElement::parse(stored_xml)
        && stored.name == stored_name.name
        && stored.namespace == stored_name.namespace
    {
        for (key, value) in stored.attributes {
            if key.starts_with("xmlns") {
                continue;
            }
            let key = if key == "lang" { "xml:lang" } else { &key };
            output.attributes.entry(key.to_owned()).or_insert(value);
        }
        output.children = stored.children;
    } else {
        output.children.push(DavXmlNode::Text(
            String::from_utf8_lossy(stored_xml).into_owned(),
        ));
    }
    output
}

/// Creates a WebDAV HTTP status-line element.
#[must_use]
pub fn dav_status_element(status: u16) -> DavXmlElement {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    dav_text_element(
        "status",
        format!(
            "HTTP/1.1 {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown"),
        ),
    )
}

/// Creates a complete `<D:error>` document.
#[must_use]
pub fn dav_error_element(condition: &DavErrorCondition) -> DavXmlElement {
    let mut error = dav_element("error");
    declare_dav_namespace(&mut error);
    error
        .children
        .push(DavXmlNode::Element(error_condition_element(condition)));
    error
}

/// Creates one `<D:propstat>` element.
#[must_use]
pub fn dav_propstat_element(propstat: DavPropStat) -> DavXmlElement {
    let mut element = dav_element("propstat");
    let mut properties = dav_element("prop");
    properties
        .children
        .extend(propstat.properties.into_iter().map(DavXmlNode::Element));
    element.children.push(DavXmlNode::Element(properties));
    element
        .children
        .push(DavXmlNode::Element(dav_status_element(propstat.status)));
    element
}

/// Creates one `<D:response>` element.
#[must_use]
pub fn dav_response_element(item: DavMultiStatusItem) -> DavXmlElement {
    let mut response = dav_element("response");
    response
        .children
        .push(DavXmlNode::Element(dav_text_element("href", item.href)));
    response.children.extend(
        item.propstats
            .into_iter()
            .map(dav_propstat_element)
            .map(DavXmlNode::Element),
    );
    if let Some(status) = item.status {
        response
            .children
            .push(DavXmlNode::Element(dav_status_element(status)));
    }
    if let Some(error) = item.error {
        let mut error_element = dav_element("error");
        error_element
            .children
            .push(DavXmlNode::Element(error_condition_element(&error)));
        response.children.push(DavXmlNode::Element(error_element));
    }
    response
}

/// Creates a complete `<D:multistatus>` document.
#[must_use]
pub fn dav_multistatus_element(items: Vec<DavMultiStatusItem>) -> DavXmlElement {
    let mut multistatus = dav_element("multistatus");
    declare_dav_namespace(&mut multistatus);
    multistatus.children.extend(
        items
            .into_iter()
            .map(dav_response_element)
            .map(DavXmlNode::Element),
    );
    multistatus
}

/// Creates the RFC 4918 `supportedlock` property value.
#[must_use]
pub fn dav_supported_lock_element() -> DavXmlElement {
    let mut supported = dav_element("supportedlock");
    for scope in ["exclusive", "shared"] {
        let mut entry = dav_element("lockentry");
        let mut lockscope = dav_element("lockscope");
        lockscope
            .children
            .push(DavXmlNode::Element(dav_element(scope)));
        entry.children.push(DavXmlNode::Element(lockscope));
        let mut locktype = dav_element("locktype");
        locktype
            .children
            .push(DavXmlNode::Element(dav_element("write")));
        entry.children.push(DavXmlNode::Element(locktype));
        supported.children.push(DavXmlNode::Element(entry));
    }
    supported
}

/// Creates the RFC 4918 `lockdiscovery` property value.
#[must_use]
pub fn dav_lock_discovery_element(locks: &[DavLockXml]) -> DavXmlElement {
    let mut discovery = dav_element("lockdiscovery");
    discovery.children.extend(
        locks
            .iter()
            .map(active_lock_element)
            .map(DavXmlNode::Element),
    );
    discovery
}

/// Creates a complete LOCK response `<D:prop>` document.
#[must_use]
pub fn dav_lock_response_element(locks: &[DavLockXml]) -> DavXmlElement {
    let mut prop = dav_element("prop");
    declare_dav_namespace(&mut prop);
    prop.children
        .push(DavXmlNode::Element(dav_lock_discovery_element(locks)));
    prop
}

/// Creates a complete DeltaV version-tree multistatus document.
#[must_use]
pub fn dav_version_multistatus_element(versions: Vec<DavVersionXml>) -> DavXmlElement {
    let items = versions
        .into_iter()
        .map(|version| {
            DavMultiStatusItem::properties(
                version.href,
                vec![DavPropStat {
                    status: StatusCode::OK.as_u16(),
                    properties: vec![
                        dav_text_element("version-name", version.version_name),
                        dav_text_element("creator-displayname", version.creator),
                        dav_text_element("getcontentlength", version.content_length.to_string()),
                        dav_text_element("getlastmodified", version.last_modified),
                    ],
                }],
            )
        })
        .collect();
    dav_multistatus_element(items)
}

fn active_lock_element(lock: &DavLockXml) -> DavXmlElement {
    let mut active = dav_element("activelock");
    let mut lockscope = dav_element("lockscope");
    lockscope
        .children
        .push(DavXmlNode::Element(dav_element(if lock.shared {
            "shared"
        } else {
            "exclusive"
        })));
    active.children.push(DavXmlNode::Element(lockscope));

    let mut locktype = dav_element("locktype");
    locktype
        .children
        .push(DavXmlNode::Element(dav_element("write")));
    active.children.push(DavXmlNode::Element(locktype));
    if let Some(owner) = &lock.owner {
        active.children.push(DavXmlNode::Element(owner.clone()));
    }
    active.children.push(DavXmlNode::Element(dav_text_element(
        "timeout",
        lock.timeout.map_or_else(
            || "Infinite".to_owned(),
            |timeout| format!("Second-{}", timeout.as_secs()),
        ),
    )));

    let mut token = dav_element("locktoken");
    token.children.push(DavXmlNode::Element(dav_text_element(
        "href",
        encode_href(&lock.token),
    )));
    active.children.push(DavXmlNode::Element(token));
    active.children.push(DavXmlNode::Element(dav_text_element(
        "depth",
        if lock.deep { "Infinity" } else { "0" },
    )));

    let mut lockroot = dav_element("lockroot");
    lockroot.children.push(DavXmlNode::Element(dav_text_element(
        "href",
        lock.root_href.clone(),
    )));
    active.children.push(DavXmlNode::Element(lockroot));
    active
}

fn error_condition_element(condition: &DavErrorCondition) -> DavXmlElement {
    match condition {
        DavErrorCondition::NoExternalEntities => dav_element("no-external-entities"),
        DavErrorCondition::LockTokenSubmitted { href } => {
            let mut condition = dav_element("lock-token-submitted");
            condition
                .children
                .push(DavXmlNode::Element(dav_text_element("href", href.clone())));
            condition
        }
        DavErrorCondition::LockTokenMatchesRequestUri => {
            dav_element("lock-token-matches-request-uri")
        }
        DavErrorCondition::PropfindFiniteDepth => dav_element("propfind-finite-depth"),
    }
}

fn declare_dav_namespace(element: &mut DavXmlElement) {
    element
        .attributes
        .insert("xmlns:D".to_owned(), "DAV:".to_owned());
}

fn property_element(name: &DavRequestedProperty, child: Option<DavXmlNode>) -> DavXmlElement {
    let prefix = name
        .prefix
        .as_deref()
        .unwrap_or_else(|| default_property_prefix(name.namespace.as_deref()));
    let tag = if name.namespace.is_some() {
        format!("{prefix}:{}", name.name)
    } else {
        name.name.clone()
    };
    let mut element = DavXmlElement::new(&tag);
    element.namespace.clone_from(&name.namespace);
    if let Some(namespace) = &name.namespace
        && (namespace != "DAV:" || prefix != "D")
    {
        element
            .attributes
            .insert(format!("xmlns:{prefix}"), namespace.clone());
    }
    if let Some(child) = child {
        element.children.push(child);
    }
    element
}

fn default_property_prefix(namespace: Option<&str>) -> &str {
    match namespace {
        Some("DAV:") => "D",
        Some(_) => "A",
        None => "",
    }
}

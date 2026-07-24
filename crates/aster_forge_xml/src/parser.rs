//! Event-based XML validation and shared parsing limits.

use std::borrow::Cow;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};

use crate::{DEFAULT_XML_MAX_DEPTH, Error, XmlSafetyError};

const DEFAULT_MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_ELEMENTS: usize = 100_000;
const DEFAULT_MAX_ATTRIBUTES_PER_ELEMENT: usize = 1_024;
const DEFAULT_MAX_TEXT_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_EVENTS: usize = 1_000_000;
const XML_NAMESPACE_URI: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE_URI: &str = "http://www.w3.org/2000/xmlns/";

/// Finite resource and declaration limits applied to untrusted XML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlSafetyPolicy {
    pub max_input_bytes: usize,
    pub max_depth: usize,
    pub max_elements: usize,
    pub max_attributes_per_element: usize,
    pub max_text_bytes: usize,
    pub max_events: usize,
    pub reject_doctype: bool,
}

impl XmlSafetyPolicy {
    /// A conservative policy suitable for network and storage protocol input.
    pub const fn untrusted() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_depth: DEFAULT_XML_MAX_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
            max_attributes_per_element: DEFAULT_MAX_ATTRIBUTES_PER_ELEMENT,
            max_text_bytes: DEFAULT_MAX_TEXT_BYTES,
            max_events: DEFAULT_MAX_EVENTS,
            reject_doctype: true,
        }
    }

    pub(crate) fn validate(self) -> Result<(), XmlSafetyError> {
        if self.max_input_bytes == 0
            || self.max_depth == 0
            || self.max_elements == 0
            || self.max_attributes_per_element == 0
            || self.max_text_bytes == 0
            || self.max_events == 0
        {
            Err(XmlSafetyError::InvalidPolicy)
        } else {
            Ok(())
        }
    }
}

impl Default for XmlSafetyPolicy {
    fn default() -> Self {
        Self::untrusted()
    }
}

/// Tree parsing behavior. Safety limits remain finite by default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParseOptions {
    pub safety: XmlSafetyPolicy,
    /// Drops whitespace-only text nodes and trims retained text nodes.
    pub trim_whitespace: bool,
}

impl ParseOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn safety_policy(mut self, policy: XmlSafetyPolicy) -> Self {
        self.safety = policy;
        self
    }

    pub fn max_depth(mut self, value: usize) -> Self {
        self.safety.max_depth = value;
        self
    }

    pub fn max_elements(mut self, value: usize) -> Self {
        self.safety.max_elements = value;
        self
    }

    pub fn max_size(mut self, value: usize) -> Self {
        self.safety.max_input_bytes = value;
        self
    }

    pub fn max_attributes_per_element(mut self, value: usize) -> Self {
        self.safety.max_attributes_per_element = value;
        self
    }

    pub fn max_text_bytes(mut self, value: usize) -> Self {
        self.safety.max_text_bytes = value;
        self
    }

    pub fn max_events(mut self, value: usize) -> Self {
        self.safety.max_events = value;
        self
    }

    pub fn allow_dtd(mut self, allow: bool) -> Self {
        self.safety.reject_doctype = !allow;
        self
    }

    pub fn trim_whitespace(mut self, trim: bool) -> Self {
        self.trim_whitespace = trim;
        self
    }
}

/// Validates one complete XML document without constructing a DOM.
pub fn validate_xml_input(bytes: &[u8], policy: XmlSafetyPolicy) -> Result<(), XmlSafetyError> {
    scan_xml(bytes, &ParseOptions::new().safety_policy(policy))
        .map(|_| ())
        .map_err(safety_error)
}

/// Returns the local name of a validated document root.
pub fn xml_root_local_name(
    bytes: &[u8],
    policy: XmlSafetyPolicy,
) -> Result<String, XmlSafetyError> {
    scan_xml(bytes, &ParseOptions::new().safety_policy(policy))
        .map_err(safety_error)?
        .ok_or(XmlSafetyError::Malformed)
}

fn safety_error(error: Error) -> XmlSafetyError {
    match error {
        Error::Safety(error) => error,
        Error::InvalidXml(_) | Error::InvalidData(_) | Error::Io(_) => XmlSafetyError::Malformed,
    }
}

#[derive(Debug)]
struct Frame {
    qualified_name: String,
    binding_start: usize,
}

#[derive(Debug)]
struct NamespaceBinding {
    prefix: String,
    uri: Option<String>,
}

#[derive(Debug, Default)]
struct ScanState {
    frames: Vec<Frame>,
    bindings: Vec<NamespaceBinding>,
    root_name: Option<String>,
    root_complete: bool,
    elements: usize,
    text_bytes: usize,
    events: usize,
}

impl ScanState {
    fn count_event(&mut self, policy: XmlSafetyPolicy) -> Result<(), Error> {
        self.events = self
            .events
            .checked_add(1)
            .ok_or(XmlSafetyError::TooManyEvents)?;
        if self.events > policy.max_events {
            return Err(XmlSafetyError::TooManyEvents.into());
        }
        Ok(())
    }

    fn count_element(&mut self, policy: XmlSafetyPolicy) -> Result<(), Error> {
        let depth = self
            .frames
            .len()
            .checked_add(1)
            .ok_or(XmlSafetyError::TooDeep)?;
        if depth > policy.max_depth {
            return Err(XmlSafetyError::TooDeep.into());
        }
        self.elements = self
            .elements
            .checked_add(1)
            .ok_or(XmlSafetyError::TooManyElements)?;
        if self.elements > policy.max_elements {
            return Err(XmlSafetyError::TooManyElements.into());
        }
        Ok(())
    }

    fn count_text(&mut self, text: &str, policy: XmlSafetyPolicy) -> Result<(), Error> {
        self.text_bytes = self
            .text_bytes
            .checked_add(text.len())
            .ok_or(XmlSafetyError::TextTooLarge)?;
        if self.text_bytes > policy.max_text_bytes {
            return Err(XmlSafetyError::TextTooLarge.into());
        }
        if self.frames.is_empty() && !text.chars().all(char::is_whitespace) {
            return Err(XmlSafetyError::Malformed.into());
        }
        Ok(())
    }

    fn namespace(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE_URI);
        }
        self.bindings
            .iter()
            .rev()
            .find(|binding| binding.prefix == prefix)
            .and_then(|binding| binding.uri.as_deref())
    }
}

fn scan_xml(bytes: &[u8], options: &ParseOptions) -> Result<Option<String>, Error> {
    options.safety.validate()?;
    if bytes.len() > options.safety.max_input_bytes {
        return Err(XmlSafetyError::InputTooLarge.into());
    }

    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut state = ScanState::default();

    loop {
        let event = reader
            .read_event()
            .map_err(|error| Error::InvalidXml(error.to_string()))?;
        if !matches!(event, Event::Eof) {
            state.count_event(options.safety)?;
        }
        match event {
            Event::Start(start) => {
                state.count_element(options.safety)?;
                let frame = scan_element(&reader, &mut state, &start, options.safety)?;
                state.frames.push(frame);
            }
            Event::Empty(start) => {
                state.count_element(options.safety)?;
                let frame = scan_element(&reader, &mut state, &start, options.safety)?;
                state.bindings.truncate(frame.binding_start);
                if state.frames.is_empty() {
                    state.root_complete = true;
                }
            }
            Event::End(end) => {
                let end_name = end.name();
                let qualified_name = utf8(end_name.as_ref())?;
                let frame = state.frames.pop().ok_or(XmlSafetyError::Malformed)?;
                if frame.qualified_name != qualified_name {
                    return Err(XmlSafetyError::Malformed.into());
                }
                state.bindings.truncate(frame.binding_start);
                if state.frames.is_empty() {
                    state.root_complete = true;
                }
            }
            Event::Text(text) => {
                let raw = utf8(text.as_ref())?;
                let value = unescape(raw).map_err(|error| Error::InvalidXml(error.to_string()))?;
                state.count_text(value.as_ref(), options.safety)?;
            }
            Event::CData(text) => state.count_text(utf8(text.as_ref())?, options.safety)?,
            Event::GeneralRef(reference) => {
                let value = decode_reference(reference.as_ref(), &reference)?;
                state.count_text(value.as_ref(), options.safety)?;
            }
            Event::Decl(_) => {
                if state.root_name.is_some() || !state.frames.is_empty() || state.root_complete {
                    return Err(XmlSafetyError::Malformed.into());
                }
            }
            Event::DocType(_) => {
                if options.safety.reject_doctype {
                    return Err(XmlSafetyError::ExternalEntity.into());
                }
                if state.root_name.is_some() || !state.frames.is_empty() || state.root_complete {
                    return Err(XmlSafetyError::Malformed.into());
                }
            }
            Event::Comment(comment) => {
                utf8(comment.as_ref())?;
            }
            Event::PI(pi) => {
                utf8(pi.target())?;
                utf8(pi.content())?;
            }
            Event::Eof => {
                if !state.frames.is_empty() || !state.root_complete {
                    return Err(XmlSafetyError::Malformed.into());
                }
                return Ok(state.root_name);
            }
        }
    }
}

fn scan_element(
    reader: &Reader<&[u8]>,
    state: &mut ScanState,
    start: &BytesStart<'_>,
    policy: XmlSafetyPolicy,
) -> Result<Frame, Error> {
    if state.frames.is_empty() && state.root_complete {
        return Err(XmlSafetyError::Malformed.into());
    }
    let start_name = start.name();
    let qualified_name = utf8(start_name.as_ref())?;
    let (prefix, local_name) = validate_qualified_name(qualified_name)?;
    let binding_start = state.bindings.len();
    let mut attribute_count = 0usize;

    for attribute in start.attributes() {
        attribute_count = attribute_count
            .checked_add(1)
            .ok_or(XmlSafetyError::TooManyAttributes)?;
        if attribute_count > policy.max_attributes_per_element {
            return Err(XmlSafetyError::TooManyAttributes.into());
        }
        let attribute = attribute.map_err(|error| Error::InvalidXml(error.to_string()))?;
        let name = utf8(attribute.key.as_ref())?;
        validate_qualified_name(name)?;
        if name == "xmlns" || name.starts_with("xmlns:") {
            let namespace_prefix = name.strip_prefix("xmlns:").unwrap_or("");
            let uri = attribute
                .decoded_and_normalized_value(XmlVersion::Explicit1_0, reader.decoder())
                .map_err(|error| Error::InvalidXml(error.to_string()))?;
            validate_namespace_binding(namespace_prefix, &uri)?;
            state.bindings.push(NamespaceBinding {
                prefix: namespace_prefix.to_owned(),
                uri: (!uri.is_empty()).then(|| uri.into_owned()),
            });
        }
    }

    if let Some(prefix) = prefix
        && state.namespace(prefix).is_none()
    {
        return Err(XmlSafetyError::Malformed.into());
    }
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| Error::InvalidXml(error.to_string()))?;
        let name = utf8(attribute.key.as_ref())?;
        if name == "xmlns" || name.starts_with("xmlns:") {
            continue;
        }
        let (prefix, _) = validate_qualified_name(name)?;
        if let Some(prefix) = prefix
            && prefix != "xml"
            && state.namespace(prefix).is_none()
        {
            return Err(XmlSafetyError::Malformed.into());
        }
    }

    if state.root_name.is_none() {
        state.root_name = Some(local_name.to_owned());
    }
    Ok(Frame {
        qualified_name: qualified_name.to_owned(),
        binding_start,
    })
}

fn decode_reference<'a>(
    bytes: &'a [u8],
    reference: &quick_xml::events::BytesRef<'a>,
) -> Result<Cow<'a, str>, Error> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| Error::InvalidXml(error.to_string()))?
    {
        return Ok(Cow::Owned(character.to_string()));
    }
    Ok(Cow::Borrowed(match utf8(bytes)? {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "apos" => "'",
        "quot" => "\"",
        _ => return Err(XmlSafetyError::ExternalEntity.into()),
    }))
}

fn utf8(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|_| XmlSafetyError::InvalidEncoding.into())
}

fn validate_qualified_name(name: &str) -> Result<(Option<&str>, &str), Error> {
    let (prefix, local) = match name.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, name),
    };
    if !valid_name(local)
        || prefix.is_some_and(|prefix| !valid_name(prefix))
        || name.matches(':').count() > 1
    {
        return Err(XmlSafetyError::Malformed.into());
    }
    Ok((prefix, local))
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

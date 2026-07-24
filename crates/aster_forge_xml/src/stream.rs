//! Bounded, namespace-aware streaming XML reader.

use std::borrow::Cow;
use std::io::{BufRead, Take, Write};
use std::str;

use aster_forge_utils::numbers::usize_to_u64;
use quick_xml::XmlVersion;
use quick_xml::encoding::Decoder;
use quick_xml::escape::unescape;
use quick_xml::events::attributes::{Attribute, Attributes as QuickAttributes};
use quick_xml::events::{BytesCData, BytesEnd, BytesPI, BytesStart, BytesText, Event};
use quick_xml::name::{NamespaceResolver, PrefixDeclaration, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::writer::Writer;

use crate::{Error, ValidatedXml, XmlSafetyError, XmlSafetyPolicy};

/// A namespace-resolved XML name borrowed from one streaming event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamName<'a> {
    qualified: &'a str,
    local: &'a str,
    namespace: Option<&'a str>,
}

impl<'a> StreamName<'a> {
    pub fn qualified(self) -> &'a str {
        self.qualified
    }

    pub fn local(self) -> &'a str {
        self.local
    }

    pub fn namespace(self) -> Option<&'a str> {
        self.namespace
    }

    pub fn matches(self, local: &str, namespace: Option<&str>) -> bool {
        self.local == local && self.namespace == namespace
    }
}

/// A start or empty element event.
pub struct StreamStart<'a> {
    raw: BytesStart<'a>,
    namespace: Option<&'a str>,
    resolver: &'a NamespaceResolver,
    decoder: Decoder,
    cached_attribute_values: &'a [CachedAttributeValue],
}

impl StreamStart<'_> {
    pub fn name(&self) -> Result<StreamName<'_>, Error> {
        let qualified = utf8(self.raw.name().into_inner())?;
        let local = utf8(self.raw.local_name().into_inner())?;
        Ok(StreamName {
            qualified,
            local,
            namespace: self.namespace,
        })
    }

    pub fn attributes(&self) -> StreamAttributes<'_> {
        StreamAttributes {
            inner: self.raw.attributes(),
            resolver: self.resolver,
            decoder: self.decoder,
            cached_values: self.cached_attribute_values,
            cached_index: 0,
            index: 0,
        }
    }

    pub fn attribute(&self, qualified_name: &str) -> Result<Option<Cow<'_, str>>, Error> {
        for attribute in self.attributes() {
            let attribute = attribute?;
            if attribute.name()?.qualified() == qualified_name {
                return attribute.into_value().map(Some);
            }
        }
        Ok(None)
    }

    pub fn attribute_ns(
        &self,
        local: &str,
        namespace: Option<&str>,
    ) -> Result<Option<Cow<'_, str>>, Error> {
        for attribute in self.attributes() {
            let attribute = attribute?;
            if attribute.name()?.matches(local, namespace) {
                return attribute.into_value().map(Some);
            }
        }
        Ok(None)
    }
}

/// Iterator over attributes of a streaming start event.
pub struct StreamAttributes<'a> {
    inner: QuickAttributes<'a>,
    resolver: &'a NamespaceResolver,
    decoder: Decoder,
    cached_values: &'a [CachedAttributeValue],
    cached_index: usize,
    index: usize,
}

impl<'a> Iterator for StreamAttributes<'a> {
    type Item = Result<StreamAttribute<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.index;
        self.index = self.index.saturating_add(1);
        let cached_value = self
            .cached_values
            .get(self.cached_index)
            .and_then(|cached| {
                if cached.index == index {
                    self.cached_index += 1;
                    Some(cached.value.as_str())
                } else {
                    None
                }
            });
        self.inner.next().map(|attribute| {
            attribute
                .map(|raw| StreamAttribute {
                    raw,
                    resolver: self.resolver,
                    decoder: self.decoder,
                    cached_value,
                })
                .map_err(|error| Error::InvalidXml(error.to_string()))
        })
    }
}

/// A namespace-resolved attribute borrowed from a streaming start event.
pub struct StreamAttribute<'a> {
    raw: Attribute<'a>,
    resolver: &'a NamespaceResolver,
    decoder: Decoder,
    cached_value: Option<&'a str>,
}

impl<'a> StreamAttribute<'a> {
    pub fn name(&self) -> Result<StreamName<'_>, Error> {
        let qualified = utf8(self.raw.key.into_inner())?;
        let local = utf8(self.raw.key.local_name().into_inner())?;
        let namespace = resolve_namespace(
            self.resolver.resolve_attribute(self.raw.key).0,
            "attribute namespace",
        )?;
        Ok(StreamName {
            qualified,
            local,
            namespace,
        })
    }

    pub fn value(&self) -> Result<Cow<'_, str>, Error> {
        if let Some(value) = self.cached_value {
            return Ok(Cow::Borrowed(value));
        }
        self.raw
            .decoded_and_normalized_value(XmlVersion::Explicit1_0, self.decoder)
            .map_err(|error| Error::InvalidXml(error.to_string()))
    }

    pub fn into_value(self) -> Result<Cow<'a, str>, Error> {
        if let Some(value) = self.cached_value {
            return Ok(Cow::Borrowed(value));
        }
        self.raw
            .decoded_and_normalized_value(XmlVersion::Explicit1_0, self.decoder)
            .map_err(|error| Error::InvalidXml(error.to_string()))
    }
}

/// An end element event.
#[derive(Debug)]
pub struct StreamEnd<'a> {
    raw: BytesEnd<'a>,
    namespace: Option<&'a str>,
}

impl StreamEnd<'_> {
    pub fn name(&self) -> Result<StreamName<'_>, Error> {
        let qualified = utf8(self.raw.name().into_inner())?;
        let local = utf8(self.raw.local_name().into_inner())?;
        Ok(StreamName {
            qualified,
            local,
            namespace: self.namespace,
        })
    }
}

/// Decoded and unescaped character data.
pub struct StreamText<'a> {
    value: Cow<'a, str>,
}

impl StreamText<'_> {
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Decoded CDATA content.
pub struct StreamCData<'a> {
    value: Cow<'a, str>,
}

impl StreamCData<'_> {
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Decoded XML comment content.
pub struct StreamComment<'a> {
    value: Cow<'a, str>,
}

impl StreamComment<'_> {
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A processing instruction.
pub struct StreamProcessingInstruction<'a> {
    raw: BytesPI<'a>,
}

impl StreamProcessingInstruction<'_> {
    pub fn target(&self) -> Result<&str, Error> {
        utf8(self.raw.target())
    }

    pub fn content(&self) -> Result<Option<&str>, Error> {
        let content = utf8(self.raw.content())?
            .trim_start_matches(|character: char| character.is_ascii_whitespace());
        Ok((!content.is_empty()).then_some(content))
    }
}

/// One bounded streaming XML event.
pub enum XmlStreamEvent<'a> {
    Start(StreamStart<'a>),
    Empty(StreamStart<'a>),
    End(StreamEnd<'a>),
    Text(StreamText<'a>),
    CData(StreamCData<'a>),
    Comment(StreamComment<'a>),
    ProcessingInstruction(StreamProcessingInstruction<'a>),
    Declaration,
    DocType,
    Eof,
}

struct StreamState {
    policy: XmlSafetyPolicy,
    max_input_bytes_u64: u64,
    depth: usize,
    elements: usize,
    text_bytes: usize,
    events: usize,
    root_seen: bool,
    root_complete: bool,
    current_start_available: bool,
    current_start_depth: usize,
    finished: bool,
}

/// A streaming XML reader that enforces [`XmlSafetyPolicy`] without retaining a full document.
pub struct XmlStreamReader<R: BufRead> {
    reader: NsReader<Take<R>>,
    buffer: Vec<u8>,
    cached_attribute_values: Vec<CachedAttributeValue>,
    state: StreamState,
}

struct CachedAttributeValue {
    index: usize,
    value: String,
}

impl<R: BufRead> XmlStreamReader<R> {
    pub fn new(reader: R, policy: XmlSafetyPolicy) -> Result<Self, Error> {
        policy.validate()?;
        let read_limit = policy.max_input_bytes.saturating_add(1);
        let read_limit = usize_to_u64(read_limit, "XML stream byte limit").unwrap_or(u64::MAX);
        let max_input_bytes_u64 =
            usize_to_u64(policy.max_input_bytes, "XML stream byte limit").unwrap_or(u64::MAX);
        let mut reader = NsReader::from_reader(reader.take(read_limit));
        reader.config_mut().trim_text(false);
        reader
            .resolver_mut()
            .set_max_declarations_per_element(policy.max_attributes_per_element);
        Ok(Self {
            reader,
            buffer: Vec::new(),
            cached_attribute_values: Vec::new(),
            state: StreamState {
                policy,
                max_input_bytes_u64,
                depth: 0,
                elements: 0,
                text_bytes: 0,
                events: 0,
                root_seen: false,
                root_complete: false,
                current_start_available: false,
                current_start_depth: 0,
                finished: false,
            },
        })
    }

    pub fn read_event(&mut self) -> Result<XmlStreamEvent<'_>, Error> {
        if self.state.finished {
            return Ok(XmlStreamEvent::Eof);
        }
        self.state.current_start_available = false;
        self.buffer.clear();
        self.cached_attribute_values.clear();
        let event = self
            .reader
            .read_event_into(&mut self.buffer)
            .map_err(map_quick_xml_error)?;
        check_stream_position(&self.reader, &self.state)?;
        count_event(&mut self.state, &event)?;

        match event {
            Event::Start(start) => {
                let namespace = begin_element(
                    &mut self.state,
                    &self.reader,
                    &start,
                    false,
                    &mut self.cached_attribute_values,
                )?;
                Ok(XmlStreamEvent::Start(StreamStart {
                    raw: start,
                    namespace,
                    resolver: self.reader.resolver(),
                    decoder: self.reader.decoder(),
                    cached_attribute_values: &self.cached_attribute_values,
                }))
            }
            Event::Empty(start) => {
                let namespace = begin_element(
                    &mut self.state,
                    &self.reader,
                    &start,
                    true,
                    &mut self.cached_attribute_values,
                )?;
                Ok(XmlStreamEvent::Empty(StreamStart {
                    raw: start,
                    namespace,
                    resolver: self.reader.resolver(),
                    decoder: self.reader.decoder(),
                    cached_attribute_values: &self.cached_attribute_values,
                }))
            }
            Event::End(end) => {
                if self.state.depth == 0 {
                    return Err(XmlSafetyError::Malformed.into());
                }
                let namespace = resolve_namespace(
                    self.reader.resolver().resolve_element(end.name()).0,
                    "element namespace",
                )?;
                utf8(end.name().into_inner())?;
                self.state.depth -= 1;
                if self.state.depth == 0 {
                    self.state.root_complete = true;
                }
                Ok(XmlStreamEvent::End(StreamEnd {
                    raw: end,
                    namespace,
                }))
            }
            Event::Text(text) => {
                let value = decode_text(&text)?;
                count_text(&mut self.state, &value)?;
                Ok(XmlStreamEvent::Text(StreamText { value }))
            }
            Event::CData(cdata) => {
                let value = cdata
                    .decode()
                    .map_err(|_| XmlSafetyError::InvalidEncoding)?;
                count_text(&mut self.state, &value)?;
                Ok(XmlStreamEvent::CData(StreamCData { value }))
            }
            Event::Comment(comment) => {
                let value = comment
                    .decode()
                    .map_err(|_| XmlSafetyError::InvalidEncoding)?;
                Ok(XmlStreamEvent::Comment(StreamComment { value }))
            }
            Event::PI(pi) => {
                utf8(pi.target())?;
                utf8(pi.content())?;
                Ok(XmlStreamEvent::ProcessingInstruction(
                    StreamProcessingInstruction { raw: pi },
                ))
            }
            Event::GeneralRef(reference) => {
                let value = decode_reference(&reference)?;
                count_text(&mut self.state, &value)?;
                Ok(XmlStreamEvent::Text(StreamText { value }))
            }
            Event::Decl(_) => {
                if self.state.root_seen || self.state.depth != 0 || self.state.root_complete {
                    return Err(XmlSafetyError::Malformed.into());
                }
                Ok(XmlStreamEvent::Declaration)
            }
            Event::DocType(_) => {
                if self.state.policy.reject_doctype {
                    return Err(XmlSafetyError::ExternalEntity.into());
                }
                if self.state.root_seen || self.state.depth != 0 || self.state.root_complete {
                    return Err(XmlSafetyError::Malformed.into());
                }
                Ok(XmlStreamEvent::DocType)
            }
            Event::Eof => {
                if self.state.depth != 0 || !self.state.root_complete {
                    return Err(XmlSafetyError::Malformed.into());
                }
                self.state.finished = true;
                Ok(XmlStreamEvent::Eof)
            }
        }
    }

    /// Reads direct text and CDATA until the end of the current start element.
    pub fn read_text_current(&mut self) -> Result<String, Error> {
        self.require_current_start()?;
        self.state.current_start_available = false;
        let mut output = String::new();
        loop {
            match self.read_event()? {
                XmlStreamEvent::Text(text) => output.push_str(text.value()),
                XmlStreamEvent::CData(cdata) => output.push_str(cdata.value()),
                XmlStreamEvent::Comment(_) | XmlStreamEvent::ProcessingInstruction(_) => {}
                XmlStreamEvent::End(_) => return Ok(output),
                XmlStreamEvent::Start(_) | XmlStreamEvent::Empty(_) => {
                    return Err(Error::InvalidXml(
                        "text helper encountered a nested element".into(),
                    ));
                }
                XmlStreamEvent::Declaration | XmlStreamEvent::DocType | XmlStreamEvent::Eof => {
                    return Err(XmlSafetyError::Malformed.into());
                }
            }
        }
    }

    /// Skips the current start element and all descendants with constant retained memory.
    pub fn skip_current(&mut self) -> Result<(), Error> {
        self.require_current_start()?;
        self.state.current_start_available = false;
        let mut nested = 1usize;
        while nested > 0 {
            match self.read_event()? {
                XmlStreamEvent::Start(_) => {
                    nested = nested.checked_add(1).ok_or(XmlSafetyError::TooDeep)?;
                }
                XmlStreamEvent::End(_) => nested -= 1,
                XmlStreamEvent::Eof => return Err(XmlSafetyError::Malformed.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Materializes only the current subtree as a validated owned XML value.
    pub fn capture_current(&mut self, max_bytes: usize) -> Result<ValidatedXml, Error> {
        self.require_current_start()?;
        if max_bytes == 0 {
            return Err(XmlSafetyError::InvalidPolicy.into());
        }
        self.state.current_start_available = false;
        let mut event_reader = quick_xml::Reader::from_reader(self.buffer.as_slice());
        let Event::Start(mut captured_start) =
            event_reader.read_event().map_err(map_quick_xml_error)?
        else {
            return Err(Error::InvalidData(
                "stream start buffer is incomplete".into(),
            ));
        };
        for (prefix, namespace) in self.reader.resolver().bindings() {
            let already_declared = captured_start.attributes().any(|attribute| {
                let Ok(attribute) = attribute else {
                    return false;
                };
                match prefix {
                    PrefixDeclaration::Default => attribute.key.as_ref() == b"xmlns",
                    PrefixDeclaration::Named(prefix) => {
                        attribute.key.as_ref().strip_prefix(b"xmlns:") == Some(prefix)
                    }
                }
            });
            if already_declared {
                continue;
            }
            let namespace = utf8(namespace.into_inner())?;
            match prefix {
                PrefixDeclaration::Default => captured_start.push_attribute(("xmlns", namespace)),
                PrefixDeclaration::Named(prefix) => {
                    let prefix = utf8(prefix)?;
                    let name = format!("xmlns:{prefix}");
                    captured_start.push_attribute((name.as_str(), namespace));
                }
            }
        }
        let mut writer = Writer::new(LimitedVec::new(Vec::new(), max_bytes));
        write_capture_event(&mut writer, Event::Start(captured_start))?;
        let mut nested = 1usize;
        while nested > 0 {
            let event = self.read_event()?;
            match event {
                XmlStreamEvent::Start(start) => {
                    nested = nested.checked_add(1).ok_or(XmlSafetyError::TooDeep)?;
                    write_capture_event(&mut writer, Event::Start(start.raw.borrow()))?;
                }
                XmlStreamEvent::Empty(start) => {
                    write_capture_event(&mut writer, Event::Empty(start.raw.borrow()))?;
                }
                XmlStreamEvent::End(end) => {
                    write_capture_event(&mut writer, Event::End(end.raw.borrow()))?;
                    nested -= 1;
                }
                XmlStreamEvent::Text(text) => {
                    write_capture_event(&mut writer, Event::Text(BytesText::new(text.value())))?
                }
                XmlStreamEvent::CData(cdata) => {
                    write_capture_event(&mut writer, Event::CData(BytesCData::new(cdata.value())))?;
                }
                XmlStreamEvent::Comment(comment) => {
                    write_capture_event(
                        &mut writer,
                        Event::Comment(BytesText::new(comment.value())),
                    )?;
                }
                XmlStreamEvent::ProcessingInstruction(pi) => {
                    write_capture_event(&mut writer, Event::PI(pi.raw.borrow()))?;
                }
                XmlStreamEvent::Declaration | XmlStreamEvent::DocType | XmlStreamEvent::Eof => {
                    return Err(XmlSafetyError::Malformed.into());
                }
            }
        }
        let bytes = writer.into_inner().bytes;
        let policy = XmlSafetyPolicy {
            max_input_bytes: max_bytes,
            ..self.state.policy
        };
        ValidatedXml::with_policy(bytes, policy)
    }

    pub fn into_inner(self) -> R {
        self.reader.into_inner().into_inner()
    }

    fn require_current_start(&self) -> Result<(), Error> {
        if self.state.current_start_available && self.state.current_start_depth == self.state.depth
        {
            Ok(())
        } else {
            Err(Error::InvalidData(
                "operation requires the most recently read event to be Start".into(),
            ))
        }
    }
}

fn check_stream_position<R: BufRead>(
    reader: &NsReader<Take<R>>,
    state: &StreamState,
) -> Result<(), Error> {
    if reader.buffer_position() > state.max_input_bytes_u64 {
        Err(XmlSafetyError::InputTooLarge.into())
    } else {
        Ok(())
    }
}

fn count_event(state: &mut StreamState, event: &Event<'_>) -> Result<(), Error> {
    if matches!(event, Event::Eof) {
        return Ok(());
    }
    if state.events >= state.policy.max_events {
        return Err(XmlSafetyError::TooManyEvents.into());
    }
    state.events += 1;
    Ok(())
}

fn begin_element<'a, R: BufRead>(
    state: &mut StreamState,
    reader: &'a NsReader<Take<R>>,
    start: &BytesStart<'_>,
    empty: bool,
    cached_attribute_values: &mut Vec<CachedAttributeValue>,
) -> Result<Option<&'a str>, Error> {
    if state.depth == 0 && state.root_complete {
        return Err(XmlSafetyError::Malformed.into());
    }
    if state.depth >= state.policy.max_depth {
        return Err(XmlSafetyError::TooDeep.into());
    }
    let depth = state.depth + 1;
    if state.elements >= state.policy.max_elements {
        return Err(XmlSafetyError::TooManyElements.into());
    }
    state.elements += 1;
    let namespace = resolve_namespace(
        reader.resolver().resolve_element(start.name()).0,
        "element namespace",
    )?;
    utf8(start.name().into_inner())?;

    for (index, attribute) in start.attributes().enumerate() {
        let attribute = attribute.map_err(|error| Error::InvalidXml(error.to_string()))?;
        if index >= state.policy.max_attributes_per_element {
            return Err(XmlSafetyError::TooManyAttributes.into());
        }
        utf8(attribute.key.into_inner())?;
        resolve_namespace(
            reader.resolver().resolve_attribute(attribute.key).0,
            "attribute namespace",
        )?;
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Explicit1_0, reader.decoder())
            .map_err(|error| Error::InvalidXml(error.to_string()))?;
        if let Cow::Owned(value) = value {
            cached_attribute_values.push(CachedAttributeValue { index, value });
        }
    }

    state.root_seen = true;
    if empty {
        if state.depth == 0 {
            state.root_complete = true;
        }
    } else {
        state.depth = depth;
        state.current_start_depth = depth;
        state.current_start_available = true;
    }
    Ok(namespace)
}

fn count_text(state: &mut StreamState, value: &str) -> Result<(), Error> {
    let remaining = state.policy.max_text_bytes - state.text_bytes;
    if value.len() > remaining {
        return Err(XmlSafetyError::TextTooLarge.into());
    }
    state.text_bytes += value.len();
    if state.depth == 0 && !value.chars().all(char::is_whitespace) {
        return Err(XmlSafetyError::Malformed.into());
    }
    Ok(())
}

fn resolve_namespace<'a>(result: ResolveResult<'a>, label: &str) -> Result<Option<&'a str>, Error> {
    match result {
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Bound(namespace) => utf8(namespace.into_inner()).map(Some),
        ResolveResult::Unknown(prefix) => Err(Error::InvalidXml(format!(
            "unknown {label} prefix `{}`",
            String::from_utf8_lossy(&prefix)
        ))),
    }
}

fn decode_text<'a>(text: &BytesText<'a>) -> Result<Cow<'a, str>, Error> {
    match text.decode().map_err(|_| XmlSafetyError::InvalidEncoding)? {
        Cow::Borrowed(value) => {
            unescape(value).map_err(|error| Error::InvalidXml(error.to_string()))
        }
        Cow::Owned(value) => unescape(&value)
            .map(Cow::into_owned)
            .map(Cow::Owned)
            .map_err(|error| Error::InvalidXml(error.to_string())),
    }
}

fn decode_reference<'a>(
    reference: &quick_xml::events::BytesRef<'a>,
) -> Result<Cow<'a, str>, Error> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| Error::InvalidXml(error.to_string()))?
    {
        return Ok(Cow::Owned(character.to_string()));
    }
    Ok(Cow::Borrowed(match utf8(reference.as_ref())? {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "apos" => "'",
        "quot" => "\"",
        _ => return Err(XmlSafetyError::ExternalEntity.into()),
    }))
}

fn utf8(bytes: &[u8]) -> Result<&str, Error> {
    str::from_utf8(bytes).map_err(|_| XmlSafetyError::InvalidEncoding.into())
}

fn map_quick_xml_error(error: quick_xml::Error) -> Error {
    match error {
        quick_xml::Error::Encoding(_) => XmlSafetyError::InvalidEncoding.into(),
        error => Error::InvalidXml(error.to_string()),
    }
}

fn write_capture_event(writer: &mut Writer<LimitedVec>, event: Event<'_>) -> Result<(), Error> {
    if let Err(error) = writer.write_event(event) {
        if writer.get_ref().exceeded {
            return Err(XmlSafetyError::InputTooLarge.into());
        }
        return Err(Error::Io(error));
    }
    Ok(())
}

struct LimitedVec {
    bytes: Vec<u8>,
    max_bytes: usize,
    exceeded: bool,
}

impl LimitedVec {
    fn new(bytes: Vec<u8>, max_bytes: usize) -> Self {
        Self {
            bytes,
            max_bytes,
            exceeded: false,
        }
    }
}

impl Write for LimitedVec {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let Some(new_len) = self.bytes.len().checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::other("XML capture exceeds byte limit"));
        };
        if new_len > self.max_bytes {
            self.exceeded = true;
            return Err(std::io::Error::other("XML capture exceeds byte limit"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

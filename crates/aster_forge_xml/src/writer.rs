//! Bounded, namespace-aware streaming XML writer.

use std::io::{self, Write};

use quick_xml::events::{BytesCData, BytesDecl, BytesEnd, BytesPI, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;

use crate::{Error, ValidatedXml, XmlSafetyError};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_DEPTH: usize = 128;
const DEFAULT_MAX_ATTRIBUTES_PER_ELEMENT: usize = 1_024;
const XML_NAMESPACE_URI: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE_URI: &str = "http://www.w3.org/2000/xmlns/";

/// Finite output limits and document options for [`XmlStreamWriter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlWriteOptions {
    pub max_output_bytes: usize,
    pub max_depth: usize,
    pub max_attributes_per_element: usize,
    pub write_document_declaration: bool,
}

impl XmlWriteOptions {
    pub const fn new() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
            max_attributes_per_element: DEFAULT_MAX_ATTRIBUTES_PER_ELEMENT,
            write_document_declaration: false,
        }
    }

    pub const fn max_output_bytes(mut self, value: usize) -> Self {
        self.max_output_bytes = value;
        self
    }

    pub const fn max_depth(mut self, value: usize) -> Self {
        self.max_depth = value;
        self
    }

    pub const fn max_attributes_per_element(mut self, value: usize) -> Self {
        self.max_attributes_per_element = value;
        self
    }

    pub const fn write_document_declaration(mut self, value: bool) -> Self {
        self.write_document_declaration = value;
        self
    }

    fn validate(self) -> Result<(), Error> {
        if self.max_output_bytes == 0 || self.max_depth == 0 || self.max_attributes_per_element == 0
        {
            Err(XmlSafetyError::InvalidPolicy.into())
        } else {
            Ok(())
        }
    }
}

impl Default for XmlWriteOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// One attribute passed to a streaming element write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlWriteAttribute<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

impl<'a> XmlWriteAttribute<'a> {
    pub const fn new(name: &'a str, value: &'a str) -> Self {
        Self { name, value }
    }
}

impl<'a> From<(&'a str, &'a str)> for XmlWriteAttribute<'a> {
    fn from((name, value): (&'a str, &'a str)) -> Self {
        Self { name, value }
    }
}

struct NamespaceBinding {
    prefix: String,
    uri: Option<String>,
}

/// Direct XML event writer with bounded output and namespace/state validation.
pub struct XmlStreamWriter<W: Write> {
    writer: Writer<LimitedWriter<W>>,
    options: XmlWriteOptions,
    depth: usize,
    root_seen: bool,
    root_complete: bool,
    open_names: Vec<String>,
    binding_starts: Vec<usize>,
    bindings: Vec<NamespaceBinding>,
    instruction: String,
}

impl<W: Write> XmlStreamWriter<W> {
    pub fn new(inner: W) -> Result<Self, Error> {
        Self::with_options(inner, XmlWriteOptions::default())
    }

    pub fn with_options(inner: W, options: XmlWriteOptions) -> Result<Self, Error> {
        options.validate()?;
        let mut output = Self {
            writer: Writer::new(LimitedWriter::new(inner, options.max_output_bytes)),
            options,
            depth: 0,
            root_seen: false,
            root_complete: false,
            open_names: Vec::new(),
            binding_starts: Vec::new(),
            bindings: Vec::new(),
            instruction: String::new(),
        };
        if options.write_document_declaration {
            output.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        }
        Ok(output)
    }

    pub fn start_element<'a, I, A>(&mut self, name: &str, attributes: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = A>,
        A: Into<XmlWriteAttribute<'a>>,
    {
        self.write_element(name, attributes, false)
    }

    pub fn empty_element<'a, I, A>(&mut self, name: &str, attributes: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = A>,
        A: Into<XmlWriteAttribute<'a>>,
    {
        self.write_element(name, attributes, true)
    }

    pub fn start(&mut self, name: &str) -> Result<(), Error> {
        self.start_element(name, std::iter::empty::<XmlWriteAttribute<'_>>())
    }

    pub fn empty(&mut self, name: &str) -> Result<(), Error> {
        self.empty_element(name, std::iter::empty::<XmlWriteAttribute<'_>>())
    }

    pub fn end_element(&mut self) -> Result<(), Error> {
        if self.depth == 0 {
            return Err(Error::InvalidData(
                "end_element called without an open element".into(),
            ));
        }
        let index = self.depth - 1;
        let name = self.open_names[index].as_str();
        let result = self.writer.write_event(Event::End(BytesEnd::new(name)));
        if let Err(error) = result {
            return self.map_io_error(error);
        }
        self.depth = index;
        let binding_start = self.binding_starts[index];
        self.bindings.truncate(binding_start);
        if self.depth == 0 {
            self.root_complete = true;
        }
        Ok(())
    }

    pub fn text(&mut self, value: &str) -> Result<(), Error> {
        validate_xml_text(value)?;
        if self.depth == 0 && !value.chars().all(char::is_whitespace) {
            return Err(Error::InvalidData(
                "text cannot appear outside the root".into(),
            ));
        }
        self.write_event(Event::Text(BytesText::new(value)))
    }

    pub fn cdata(&mut self, value: &str) -> Result<(), Error> {
        validate_xml_text(value)?;
        if self.depth == 0 {
            return Err(Error::InvalidData(
                "CDATA cannot appear outside the root".into(),
            ));
        }
        if value.contains("]]>") {
            return Err(Error::InvalidData("CDATA contains `]]>`".into()));
        }
        self.write_event(Event::CData(BytesCData::new(value)))
    }

    pub fn comment(&mut self, value: &str) -> Result<(), Error> {
        validate_xml_text(value)?;
        if value.contains("--") || value.ends_with('-') {
            return Err(Error::InvalidData(
                "comment contains an XML-forbidden hyphen sequence".into(),
            ));
        }
        self.write_event(Event::Comment(BytesText::from_escaped(value)))
    }

    pub fn processing_instruction(
        &mut self,
        target: &str,
        content: Option<&str>,
    ) -> Result<(), Error> {
        validate_name(target, false)?;
        if target.eq_ignore_ascii_case("xml") {
            return Err(Error::InvalidData(
                "processing-instruction target cannot be `xml`".into(),
            ));
        }
        self.instruction.clear();
        self.instruction.push_str(target);
        if let Some(content) = content {
            validate_xml_text(content)?;
            if content.contains("?>") {
                return Err(Error::InvalidData(
                    "processing instruction contains `?>`".into(),
                ));
            }
            if !content.is_empty() {
                self.instruction.push(' ');
                self.instruction.push_str(content);
            }
        }
        let instruction = BytesPI::new(self.instruction.as_str());
        let result = self.writer.write_event(Event::PI(instruction));
        match result {
            Ok(()) => Ok(()),
            Err(error) => self.map_io_error(error),
        }
    }

    /// Embeds one already validated, self-contained XML root below the current element.
    pub fn validated_subtree(&mut self, subtree: &ValidatedXml) -> Result<(), Error> {
        if self.depth == 0 {
            return Err(Error::InvalidData(
                "validated subtree requires an open parent element".into(),
            ));
        }
        for element in subtree.document().root().descendants() {
            if element.attributes().count() > self.options.max_attributes_per_element {
                return Err(XmlSafetyError::TooManyAttributes.into());
            }
            let mut relative_depth = 1usize;
            let mut parent = element.parent();
            while let Some(element) = parent {
                relative_depth = relative_depth
                    .checked_add(1)
                    .ok_or(XmlSafetyError::TooDeep)?;
                parent = element.parent();
            }
            let combined_depth = self
                .depth
                .checked_add(relative_depth)
                .ok_or(XmlSafetyError::TooDeep)?;
            if combined_depth > self.options.max_depth {
                return Err(XmlSafetyError::TooDeep.into());
            }
        }
        self.write_raw(subtree.document().root().raw_xml())
    }

    pub fn written_bytes(&self) -> usize {
        self.writer.get_ref().written
    }

    pub fn get_ref(&self) -> &W {
        &self.writer.get_ref().inner
    }

    pub fn finish(mut self) -> Result<W, Error> {
        if self.depth != 0 {
            return Err(Error::InvalidData(
                "XML document has unclosed elements".into(),
            ));
        }
        if !self.root_seen || !self.root_complete {
            return Err(Error::InvalidData(
                "XML document has no complete root".into(),
            ));
        }
        if let Err(error) = self.writer.get_mut().flush() {
            if self.writer.get_ref().exceeded {
                return Err(XmlSafetyError::OutputTooLarge.into());
            }
            return Err(Error::Io(error));
        }
        Ok(self.writer.into_inner().inner)
    }

    fn write_element<'a, I, A>(
        &mut self,
        name: &str,
        attributes: I,
        empty: bool,
    ) -> Result<(), Error>
    where
        I: IntoIterator<Item = A>,
        A: Into<XmlWriteAttribute<'a>>,
    {
        if self.depth == 0 && self.root_complete {
            return Err(Error::InvalidData(
                "XML document cannot contain multiple roots".into(),
            ));
        }
        let next_depth = self.depth.checked_add(1).ok_or(XmlSafetyError::TooDeep)?;
        if next_depth > self.options.max_depth {
            return Err(XmlSafetyError::TooDeep.into());
        }
        validate_name(name, true)?;

        let binding_start = self.bindings.len();
        let mut start = BytesStart::new(name);
        let result = (|| {
            let mut attribute_count = 0usize;
            for attribute in attributes {
                let attribute = attribute.into();
                attribute_count = attribute_count
                    .checked_add(1)
                    .ok_or(XmlSafetyError::TooManyAttributes)?;
                if attribute_count > self.options.max_attributes_per_element {
                    return Err(XmlSafetyError::TooManyAttributes.into());
                }
                validate_name(attribute.name, true)?;
                validate_xml_text(attribute.value)?;
                self.record_namespace_binding(attribute)?;
                start.push_attribute((attribute.name, attribute.value));
            }
            for attribute in start.attributes() {
                let attribute = attribute.map_err(|error| Error::InvalidData(error.to_string()))?;
                let attribute_name = std::str::from_utf8(attribute.key.as_ref())
                    .map_err(|_| Error::InvalidData("attribute name is not UTF-8".into()))?;
                self.validate_attribute_namespace(attribute_name)?;
            }
            for (index, left) in start.attributes().enumerate() {
                let left = left.map_err(|error| Error::InvalidData(error.to_string()))?;
                let left_name = std::str::from_utf8(left.key.as_ref())
                    .map_err(|_| Error::InvalidData("attribute name is not UTF-8".into()))?;
                for right in start.attributes().skip(index + 1) {
                    let right = right.map_err(|error| Error::InvalidData(error.to_string()))?;
                    let right_name = std::str::from_utf8(right.key.as_ref())
                        .map_err(|_| Error::InvalidData("attribute name is not UTF-8".into()))?;
                    if self.attributes_share_expanded_name(left_name, right_name) {
                        return Err(Error::InvalidData(format!(
                            "attributes `{left_name}` and `{right_name}` have the same expanded name"
                        )));
                    }
                }
            }
            self.validate_element_namespace(name)
        })();
        if let Err(error) = result {
            self.bindings.truncate(binding_start);
            return Err(error);
        }

        let event = if empty {
            Event::Empty(start)
        } else {
            Event::Start(start)
        };
        if let Err(error) = self.write_event(event) {
            self.bindings.truncate(binding_start);
            return Err(error);
        }
        self.root_seen = true;
        if empty {
            self.bindings.truncate(binding_start);
            if self.depth == 0 {
                self.root_complete = true;
            }
        } else {
            if self.open_names.len() == self.depth {
                self.open_names.push(String::new());
                self.binding_starts.push(0);
            }
            self.open_names[self.depth].clear();
            self.open_names[self.depth].push_str(name);
            self.binding_starts[self.depth] = binding_start;
            self.depth = next_depth;
        }
        Ok(())
    }

    fn record_namespace_binding(&mut self, attribute: XmlWriteAttribute<'_>) -> Result<(), Error> {
        let Some(prefix) = namespace_declaration_prefix(attribute.name) else {
            return Ok(());
        };
        validate_namespace_binding(prefix, attribute.value)?;
        self.bindings.push(NamespaceBinding {
            prefix: prefix.to_owned(),
            uri: (!attribute.value.is_empty()).then(|| attribute.value.to_owned()),
        });
        Ok(())
    }

    fn validate_element_namespace(&self, qualified_name: &str) -> Result<(), Error> {
        if let Some((prefix, _)) = qualified_name.split_once(':')
            && self.resolve_namespace(prefix).is_none()
        {
            return Err(Error::InvalidData(format!(
                "element prefix `{prefix}` has no namespace binding"
            )));
        }
        Ok(())
    }

    fn validate_attribute_namespace(&self, qualified_name: &str) -> Result<(), Error> {
        if namespace_declaration_prefix(qualified_name).is_some() {
            return Ok(());
        }
        if let Some((prefix, _)) = qualified_name.split_once(':')
            && prefix != "xml"
            && self.resolve_namespace(prefix).is_none()
        {
            return Err(Error::InvalidData(format!(
                "attribute prefix `{prefix}` has no namespace binding"
            )));
        }
        Ok(())
    }

    fn attributes_share_expanded_name(&self, left: &str, right: &str) -> bool {
        if namespace_declaration_prefix(left).is_some()
            || namespace_declaration_prefix(right).is_some()
        {
            return false;
        }
        let (left_prefix, left_local) = left.split_once(':').unwrap_or(("", left));
        let (right_prefix, right_local) = right.split_once(':').unwrap_or(("", right));
        if left_local != right_local {
            return false;
        }
        let left_namespace = (!left_prefix.is_empty())
            .then(|| self.resolve_namespace(left_prefix))
            .flatten();
        let right_namespace = (!right_prefix.is_empty())
            .then(|| self.resolve_namespace(right_prefix))
            .flatten();
        left_namespace == right_namespace
    }

    fn resolve_namespace(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE_URI);
        }
        self.bindings
            .iter()
            .rev()
            .find(|binding| binding.prefix == prefix)
            .and_then(|binding| binding.uri.as_deref())
    }

    fn write_event(&mut self, event: Event<'_>) -> Result<(), Error> {
        match self.writer.write_event(event) {
            Ok(()) => Ok(()),
            Err(error) => self.map_io_error(error),
        }
    }

    fn write_raw(&mut self, bytes: &[u8]) -> Result<(), Error> {
        match self.writer.get_mut().write_all(bytes) {
            Ok(()) => Ok(()),
            Err(error) => self.map_io_error(error),
        }
    }

    fn map_io_error(&self, error: io::Error) -> Result<(), Error> {
        if self.writer.get_ref().exceeded {
            Err(XmlSafetyError::OutputTooLarge.into())
        } else {
            Err(Error::Io(error))
        }
    }
}

fn namespace_declaration_prefix(name: &str) -> Option<&str> {
    if name == "xmlns" {
        Some("")
    } else {
        name.strip_prefix("xmlns:")
    }
}

fn validate_namespace_binding(prefix: &str, uri: &str) -> Result<(), Error> {
    if prefix == "xmlns"
        || uri == XMLNS_NAMESPACE_URI
        || (prefix == "xml" && uri != XML_NAMESPACE_URI)
        || (prefix != "xml" && uri == XML_NAMESPACE_URI)
        || (!prefix.is_empty() && uri.is_empty())
    {
        Err(Error::InvalidData("invalid namespace binding".into()))
    } else {
        Ok(())
    }
}

fn validate_name(name: &str, qualified: bool) -> Result<(), Error> {
    if name.is_empty() || (!qualified && name.contains(':')) || name.matches(':').count() > 1 {
        return Err(Error::InvalidData(format!("invalid XML name `{name}`")));
    }
    for part in name.split(':') {
        let mut characters = part.chars();
        if !characters.next().is_some_and(is_name_start) || !characters.all(is_name_char) {
            return Err(Error::InvalidData(format!("invalid XML name `{name}`")));
        }
    }
    Ok(())
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

fn validate_xml_text(value: &str) -> Result<(), Error> {
    if value.chars().all(|character| {
        matches!(character, '\u{9}' | '\u{A}' | '\u{D}')
            || ('\u{20}'..='\u{D7FF}').contains(&character)
            || ('\u{E000}'..='\u{FFFD}').contains(&character)
            || ('\u{10000}'..='\u{10FFFF}').contains(&character)
    }) {
        Ok(())
    } else {
        Err(Error::InvalidData(
            "value contains a character forbidden by XML 1.0".into(),
        ))
    }
}

struct LimitedWriter<W> {
    inner: W,
    max_bytes: usize,
    written: usize,
    exceeded: bool,
}

impl<W> LimitedWriter<W> {
    fn new(inner: W, max_bytes: usize) -> Self {
        Self {
            inner,
            max_bytes,
            written: 0,
            exceeded: false,
        }
    }
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(new_written) = self.written.checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(io::Error::other("XML output exceeds byte limit"));
        };
        if new_written > self.max_bytes {
            self.exceeded = true;
            return Err(io::Error::other("XML output exceeds byte limit"));
        }
        self.inner.write_all(buffer)?;
        self.written = new_written;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

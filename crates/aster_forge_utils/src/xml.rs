//! Bounded XML input validation helpers.
//!
//! The helpers intentionally validate XML as an event stream instead of constructing a DOM.
//! Callers that need to retain a document tree must validate the untrusted bytes first, then pass
//! only accepted input to their domain-specific parser.

use quick_xml::Reader;
use quick_xml::events::Event;

/// Default maximum allowed nesting depth for untrusted XML documents.
pub const DEFAULT_XML_MAX_DEPTH: usize = 128;

/// Validation policy for untrusted XML input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlSafetyPolicy {
    /// Maximum number of simultaneously open elements.
    pub max_depth: usize,
    /// Whether document type declarations must be rejected.
    pub reject_doctype: bool,
}

impl XmlSafetyPolicy {
    /// A conservative policy suitable for externally supplied XML.
    pub const fn untrusted() -> Self {
        Self {
            max_depth: DEFAULT_XML_MAX_DEPTH,
            reject_doctype: true,
        }
    }
}

/// Failure produced while validating XML input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum XmlSafetyError {
    /// The supplied policy cannot validate any XML element.
    #[error("XML maximum depth must be positive")]
    InvalidPolicy,
    /// The input declares a document type or entity while declarations are prohibited.
    #[error("XML external entity declarations are not allowed")]
    ExternalEntity,
    /// The input exceeds the configured simultaneous element depth.
    #[error("XML nesting depth exceeds the configured limit")]
    TooDeep,
    /// The input is not a complete, single-root XML document.
    #[error("malformed XML input")]
    Malformed,
}

/// Validates XML without recursively constructing a document tree.
///
/// With `policy.reject_doctype`, a byte-level pre-scan rejects any
/// `<!DOCTYPE` / `<!ENTITY` marker before parsing. The pre-scan does not
/// understand CDATA sections or comments, so documents that legitimately
/// contain those literals in text (e.g. `<![CDATA[<!DOCTYPE x>]]>`) are
/// rejected too. That direction is fail-safe — no real declaration can slip
/// through — and products that must accept such content should use a
/// different parsing channel.
pub fn validate_xml_input(
    bytes: &[u8],
    policy: XmlSafetyPolicy,
) -> std::result::Result<(), XmlSafetyError> {
    if policy.max_depth == 0 {
        return Err(XmlSafetyError::InvalidPolicy);
    }
    if policy.reject_doctype && contains_external_entity_declaration(bytes) {
        return Err(XmlSafetyError::ExternalEntity);
    }

    let mut reader = Reader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut root_count = 0usize;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|_| XmlSafetyError::Malformed)?;
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    root_count += 1;
                    if root_count > 1 {
                        return Err(XmlSafetyError::Malformed);
                    }
                }
                depth = depth.checked_add(1).ok_or(XmlSafetyError::TooDeep)?;
                if depth > policy.max_depth {
                    return Err(XmlSafetyError::TooDeep);
                }
            }
            Event::Empty(_) => {
                if depth == 0 {
                    root_count += 1;
                    if root_count > 1 {
                        return Err(XmlSafetyError::Malformed);
                    }
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or(XmlSafetyError::Malformed)?;
            }
            Event::DocType(_) if policy.reject_doctype => {
                return Err(XmlSafetyError::ExternalEntity);
            }
            Event::Eof => {
                if depth != 0 || root_count != 1 {
                    return Err(XmlSafetyError::Malformed);
                }
                return Ok(());
            }
            _ => {}
        }
        buffer.clear();
    }
}

/// Returns the local name of the document root after applying the safety policy.
pub fn xml_root_local_name(
    bytes: &[u8],
    policy: XmlSafetyPolicy,
) -> std::result::Result<String, XmlSafetyError> {
    validate_xml_input(bytes, policy)?;

    let mut reader = Reader::from_reader(bytes);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| XmlSafetyError::Malformed)?
        {
            Event::Start(element) | Event::Empty(element) => {
                return std::str::from_utf8(element.local_name().as_ref())
                    .map(str::to_owned)
                    .map_err(|_| XmlSafetyError::Malformed);
            }
            Event::Eof => return Err(XmlSafetyError::Malformed),
            _ => buffer.clear(),
        }
    }
}

fn contains_external_entity_declaration(bytes: &[u8]) -> bool {
    let mut index = 0;
    while let Some(offset) = bytes[index..].iter().position(|byte| *byte == b'<') {
        index += offset + 1;
        let Some(marker) = bytes.get(index) else {
            break;
        };
        let Some(after_bang) = bytes.get(index + 1..) else {
            break;
        };
        if *marker == b'!'
            && (after_bang.len() >= b"DOCTYPE".len()
                && after_bang[..b"DOCTYPE".len()].eq_ignore_ascii_case(b"DOCTYPE")
                || after_bang.len() >= b"ENTITY".len()
                    && after_bang[..b"ENTITY".len()].eq_ignore_ascii_case(b"ENTITY"))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_XML_MAX_DEPTH, XmlSafetyError, XmlSafetyPolicy, validate_xml_input,
        xml_root_local_name,
    };

    #[test]
    fn accepts_regular_xml_and_exact_depth_limit() {
        assert!(validate_xml_input(b"<root><child/></root>", XmlSafetyPolicy::untrusted()).is_ok());

        let mut xml = String::new();
        for _ in 0..DEFAULT_XML_MAX_DEPTH {
            xml.push_str("<x>");
        }
        for _ in 0..DEFAULT_XML_MAX_DEPTH {
            xml.push_str("</x>");
        }
        assert!(validate_xml_input(xml.as_bytes(), XmlSafetyPolicy::untrusted()).is_ok());
    }

    #[test]
    fn rejects_doctype_and_excessive_depth() {
        assert_eq!(
            validate_xml_input(b"<!DOCTYPE root><root/>", XmlSafetyPolicy::untrusted()),
            Err(XmlSafetyError::ExternalEntity)
        );
        assert_eq!(
            validate_xml_input(
                b"<!ENTITY x \"value\"><root/>",
                XmlSafetyPolicy::untrusted()
            ),
            Err(XmlSafetyError::ExternalEntity)
        );
        assert_eq!(
            validate_xml_input(b"<!doctype root><root/>", XmlSafetyPolicy::untrusted()),
            Err(XmlSafetyError::ExternalEntity)
        );

        let mut xml = String::new();
        for _ in 0..=DEFAULT_XML_MAX_DEPTH {
            xml.push_str("<x>");
        }
        for _ in 0..=DEFAULT_XML_MAX_DEPTH {
            xml.push_str("</x>");
        }
        assert!(validate_xml_input(xml.as_bytes(), XmlSafetyPolicy::untrusted()).is_err());
    }

    #[test]
    fn rejects_empty_and_malformed_documents() {
        for xml in [
            b"".as_slice(),
            b"<?xml version=\"1.0\"?>",
            b"<root>",
            b"<a></b>",
            b"<first/><second/>",
            b"<",
        ] {
            assert!(validate_xml_input(xml, XmlSafetyPolicy::untrusted()).is_err());
        }
    }

    #[test]
    fn rejects_invalid_policy_and_can_allow_doctype_when_requested() {
        assert!(
            validate_xml_input(
                b"<root/>",
                XmlSafetyPolicy {
                    max_depth: 0,
                    reject_doctype: true,
                }
            )
            .is_err()
        );

        assert!(
            validate_xml_input(
                b"<!DOCTYPE root><root/>",
                XmlSafetyPolicy {
                    max_depth: DEFAULT_XML_MAX_DEPTH,
                    reject_doctype: false,
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn reads_prefixed_and_empty_root_local_name_after_validation() {
        assert_eq!(
            xml_root_local_name(
                br#"<D:version-tree xmlns:D="DAV:"/>"#,
                XmlSafetyPolicy::untrusted(),
            ),
            Ok("version-tree".to_string())
        );
        assert_eq!(
            xml_root_local_name(b"<root><child/></root>", XmlSafetyPolicy::untrusted()),
            Ok("root".to_string())
        );
        assert_eq!(
            xml_root_local_name(b"<first/><second/>", XmlSafetyPolicy::untrusted()),
            Err(XmlSafetyError::Malformed)
        );
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum XmlSafetyError {
    #[error("XML maximum depth must be positive")]
    InvalidPolicy,
    #[error("XML document type declarations are not allowed")]
    Doctype,
    #[error("XML nesting depth exceeds the configured limit")]
    TooDeep,
    #[error("malformed XML input")]
    Malformed,
}

/// Validates XML without recursively constructing a document tree.
pub fn validate_xml_input(
    bytes: &[u8],
    policy: XmlSafetyPolicy,
) -> std::result::Result<(), XmlSafetyError> {
    if policy.max_depth == 0 {
        return Err(XmlSafetyError::InvalidPolicy);
    }

    let mut reader = Reader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_element = false;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|_| XmlSafetyError::Malformed)?;
        match event {
            Event::Start(_) => {
                saw_element = true;
                depth = depth.checked_add(1).ok_or(XmlSafetyError::TooDeep)?;
                if depth > policy.max_depth {
                    return Err(XmlSafetyError::TooDeep);
                }
            }
            Event::Empty(_) => saw_element = true,
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or(XmlSafetyError::Malformed)?;
            }
            Event::DocType(_) if policy.reject_doctype => {
                return Err(XmlSafetyError::Doctype);
            }
            Event::Eof => {
                if depth != 0 || !saw_element {
                    return Err(XmlSafetyError::Malformed);
                }
                return Ok(());
            }
            _ => {}
        }
        buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_XML_MAX_DEPTH, XmlSafetyPolicy, validate_xml_input};

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
        assert!(
            validate_xml_input(b"<!DOCTYPE root><root/>", XmlSafetyPolicy::untrusted()).is_err()
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
}

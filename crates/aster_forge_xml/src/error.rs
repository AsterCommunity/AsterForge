//! Error types for bounded XML parsing and source I/O.

use std::fmt;

/// Failures produced while applying an [`XmlSafetyPolicy`](crate::XmlSafetyPolicy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlSafetyError {
    /// One or more configured limits are zero.
    InvalidPolicy,
    /// The input is larger than the configured byte limit.
    InputTooLarge,
    /// Generated XML exceeds the configured byte limit.
    OutputTooLarge,
    /// The input declares a DTD or custom entity while declarations are prohibited.
    ExternalEntity,
    /// The element nesting depth exceeds the configured limit.
    TooDeep,
    /// The total element count exceeds the configured limit.
    TooManyElements,
    /// An element has more attributes than the configured limit.
    TooManyAttributes,
    /// The total decoded text and CDATA size exceeds the configured limit.
    TextTooLarge,
    /// The parser emitted more events than the configured limit.
    TooManyEvents,
    /// The document contains bytes that are not valid in its declared encoding.
    InvalidEncoding,
    /// The input is not one complete, well-formed, single-root XML document.
    Malformed,
}

impl fmt::Display for XmlSafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPolicy => "XML safety limits must be positive",
            Self::InputTooLarge => "XML input exceeds the configured byte limit",
            Self::OutputTooLarge => "XML output exceeds the configured byte limit",
            Self::ExternalEntity => "XML DTD and custom entity declarations are not allowed",
            Self::TooDeep => "XML nesting depth exceeds the configured limit",
            Self::TooManyElements => "XML element count exceeds the configured limit",
            Self::TooManyAttributes => "XML attribute count exceeds the configured limit",
            Self::TextTooLarge => "XML text size exceeds the configured limit",
            Self::TooManyEvents => "XML event count exceeds the configured limit",
            Self::InvalidEncoding => "XML input contains invalid encoded text",
            Self::Malformed => "malformed XML input",
        })
    }
}

impl std::error::Error for XmlSafetyError {}

/// Errors produced while parsing or retaining an XML document.
#[derive(Debug)]
pub enum Error {
    /// A configured input safety boundary was crossed.
    Safety(XmlSafetyError),
    /// The document is structurally invalid. The message is diagnostic only.
    InvalidXml(String),
    /// A writer operation would produce invalid XML or violate writer state.
    InvalidData(String),
    /// Reading or writing failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Safety(error) => write!(f, "XML safety error: {error}"),
            Self::InvalidXml(message) => write!(f, "invalid XML: {message}"),
            Self::InvalidData(message) => write!(f, "invalid XML data: {message}"),
            Self::Io(error) => write!(f, "XML I/O error: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Safety(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidXml(_) | Self::InvalidData(_) => None,
        }
    }
}

impl From<XmlSafetyError> for Error {
    fn from(error: XmlSafetyError) -> Self {
        Self::Safety(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_error_display_is_distinct_from_its_source() {
        let safety = Error::Safety(XmlSafetyError::TextTooLarge);
        assert_eq!(
            safety.to_string(),
            "XML safety error: XML text size exceeds the configured limit"
        );
        assert_eq!(
            std::error::Error::source(&safety).map(ToString::to_string),
            Some("XML text size exceeds the configured limit".into())
        );

        let io = Error::Io(std::io::Error::other("fixture failure"));
        assert_eq!(io.to_string(), "XML I/O error: fixture failure");
        assert_eq!(
            std::error::Error::source(&io).map(ToString::to_string),
            Some("fixture failure".into())
        );
    }
}

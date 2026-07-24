//! Error types for XML parsing and serialization.

use std::fmt;

/// Errors that can occur during XML parsing and manipulation.
#[derive(Debug, Clone)]
pub enum Error {
    /// XML parse error (from quick-xml or custom parsing)
    Parse(String),
    /// Maximum nesting depth exceeded
    MaxDepthExceeded,
    /// Maximum number of elements exceeded
    MaxElementsExceeded,
    /// Maximum input size exceeded (in bytes)
    MaxSizeExceeded,
    /// DTD declarations are not allowed
    DtdNotAllowed,
    /// ENTITY declarations are not allowed
    EntityNotAllowed,
    /// Invalid XML structure (e.g. extra content after root node)
    InvalidXml(String),
    /// I/O error
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(msg) => write!(f, "parse error: {}", msg),
            Error::MaxDepthExceeded => write!(f, "maximum nesting depth exceeded"),
            Error::MaxElementsExceeded => write!(f, "maximum element count exceeded"),
            Error::MaxSizeExceeded => write!(f, "maximum input size exceeded"),
            Error::DtdNotAllowed => write!(f, "DTD declaration is not allowed"),
            Error::EntityNotAllowed => write!(f, "ENTITY declaration is not allowed"),
            Error::InvalidXml(msg) => write!(f, "invalid XML: {}", msg),
            Error::Io(msg) => write!(f, "I/O error: {}", msg),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

impl From<quick_xml::Error> for Error {
    fn from(e: quick_xml::Error) -> Self {
        Error::Parse(e.to_string())
    }
}

impl From<quick_xml::events::attributes::AttrError> for Error {
    fn from(e: quick_xml::events::attributes::AttrError) -> Self {
        Error::Parse(e.to_string())
    }
}

impl From<quick_xml::encoding::EncodingError> for Error {
    fn from(e: quick_xml::encoding::EncodingError) -> Self {
        Error::Parse(e.to_string())
    }
}

impl From<quick_xml::escape::EscapeError> for Error {
    fn from(e: quick_xml::escape::EscapeError) -> Self {
        Error::Parse(e.to_string())
    }
}

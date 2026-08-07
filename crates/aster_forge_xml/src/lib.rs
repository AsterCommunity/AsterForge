//! Bounded, source-backed XML parsing for Aster services.
//!
//! Parsed documents use a flat arena and retain source spans for names, attributes, text, and
//! subtrees. Values allocate only when XML decoding or configured normalization changes them.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

mod document;
mod error;
mod parser;
mod stream;
mod syntax;
mod writer;

pub use document::{
    AttributeRef, Attributes, BorrowedDocument, ChildElements, Children, DescendantElements,
    ElementRef, NodeId, NodeRef, OwnedDocument, ProcessingInstructionRef, SourceSpan, ValidatedXml,
    XmlDocument,
};
pub use error::{Error, XmlSafetyError};
pub use parser::{ParseOptions, XmlSafetyPolicy, validate_xml_input, xml_root_local_name};
pub use stream::{
    StreamAttribute, StreamAttributes, StreamCData, StreamComment, StreamEnd, StreamName,
    StreamProcessingInstruction, StreamStart, StreamText, XmlStreamEvent, XmlStreamReader,
};
pub use syntax::{is_valid_xml_local_name, is_valid_xml_namespace_name};
pub use writer::{XmlStreamWriter, XmlWriteAttribute, XmlWriteOptions};

/// The default maximum nesting depth accepted from untrusted XML.
pub const DEFAULT_XML_MAX_DEPTH: usize = 128;

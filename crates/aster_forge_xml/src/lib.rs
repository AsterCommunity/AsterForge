//! Bounded, source-backed XML parsing for Aster services.
//!
//! Parsed documents use a flat arena and retain source spans for names, attributes, text, and
//! subtrees. Values allocate only when XML decoding or configured normalization changes them.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
pub use writer::{XmlStreamWriter, XmlWriteAttribute, XmlWriteOptions};

/// The default maximum nesting depth accepted from untrusted XML.
pub const DEFAULT_XML_MAX_DEPTH: usize = 128;

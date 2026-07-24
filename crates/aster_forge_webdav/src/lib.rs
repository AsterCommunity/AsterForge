//! Product-neutral WebDAV protocol engine contracts for Aster services.
//!
//! This crate owns WebDAV paths, request parsing, protocol preconditions, backend ports,
//! response models, and observable operation events. Product repositories own authentication,
//! authorization, workspace scope, persistence, storage policy, quota, and audit semantics.
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

#[cfg(feature = "actix")]
pub mod actix;
pub mod backend;
pub mod event;
pub mod path;
pub mod protocol;
pub mod request;
pub mod response;
pub mod xml;
pub mod xml_response;

pub use backend::{
    DavBackend, DavBackendError, DavBackendErrorKind, DavContentStream, DavDirectoryEntry,
    DavLockBackend, DavLockInfo, DavLockRequest, DavProperty, DavPropertyBackend, DavPropertyName,
    DavPropertyPatch, DavPropertyPatchOutcome, DavReadOutcome, DavResourceBackend, DavResourceKind,
    DavResourceMetadata, DavVersionBackend, DavVersionInfo, DavWriteOutcome, DavWriteRequest,
};
pub use event::{DavEvent, DavEventOutcome, DavEventSink, DavOperation, NoopDavEventSink};
pub use path::{
    DavPath, DavPathError, child_relative_path, decode_relative_path, display_name, encode_href,
    href_for_dav_path, href_for_relative, parent_relative_path,
};
pub use protocol::{
    DavPrecondition, DavProtocolError, DavProtocolErrorKind, Depth, Destination, IfHeader,
    IfResourceGroup, IfStateCondition, IfStateList, destination_relative_path,
    evaluate_http_download_preconditions, evaluate_http_etag_preconditions, parse_copy_depth,
    parse_delete_depth, parse_if_header, parse_lock_depth, parse_lock_timeout,
    parse_lock_token_header, parse_move_depth, parse_overwrite, parse_propfind_depth,
    submitted_lock_tokens, submitted_lock_tokens_for_path,
};
pub use request::{DavMethod, DavRequestHead, DavRequestOrigin};
pub use response::{DavResponse, DavResponseBody};
pub use xml::{
    DavLockRequestBody, DavPropertyPatchRequest, DavPropertyPatchValue, DavPropfindRequest,
    DavRequestedProperty, DavXmlElement, DavXmlError, DavXmlNode, parse_lock_request,
    parse_propfind_request, parse_proppatch_request, parse_report_root,
};
pub use xml_response::{
    DavErrorCondition, DavLockXml, DavMultiStatusItem, DavPropStat, DavVersionXml,
    dav_dead_property_element, dav_element, dav_error_element, dav_lock_discovery_element,
    dav_lock_response_element, dav_multistatus_element, dav_property_child_element,
    dav_property_name_element, dav_property_text_element, dav_propstat_element,
    dav_response_element, dav_status_element, dav_supported_lock_element, dav_text_element,
    dav_version_multistatus_element,
};

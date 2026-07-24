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
pub mod deltav;
pub mod event;
pub mod lock;
pub mod path;
pub mod property;
pub mod protocol;
pub mod put;
pub mod request;
pub mod resource;
pub mod response;
pub mod xml;
pub mod xml_response;

pub use backend::{
    DavBackend, DavBackendError, DavBackendErrorKind, DavContentStream, DavDirectoryEntry,
    DavIfResourceState, DavIfStateResolver, DavLockBackend, DavLockInfo, DavLockRequest,
    DavProperty, DavPropertyBackend, DavPropertyName, DavPropertyPatch, DavPropertyPatchOutcome,
    DavReadOutcome, DavResourceBackend, DavResourceKind, DavResourceMetadata, DavVersionBackend,
    DavVersionInfo, DavWriteOutcome, DavWriteRequest,
};
pub use deltav::{
    DavVersionTreeReportError, validate_version_tree_report, version_control_response,
    version_tree_non_file_response, version_tree_report_error_response, version_tree_response,
};
pub use event::{DavEvent, DavEventOutcome, DavEventSink, DavOperation, NoopDavEventSink};
pub use lock::{
    DavLockPlan, DavLockPlanError, lock_acquire_success_response, lock_conflict_response,
    lock_limit_response, lock_refresh_success_response, lock_xml_error_response, plan_lock_request,
    unlock_success_response, unlock_token_mismatch_response,
};
pub use path::{
    DavPath, DavPathError, child_relative_path, decode_relative_path, display_name, encode_href,
    href_for_dav_path, href_for_relative, parent_relative_path,
};
pub use property::{
    DavProppatchAtomicPlan, build_propfind_item, build_proppatch_item, format_creation_date,
    plan_atomic_proppatch, property_multistatus_response, propfind_finite_depth_response,
    propfind_request_label, propfind_xml_error_response, proppatch_xml_error_response,
};
pub use protocol::{
    DavIfEvaluationError, DavPrecondition, DavProtocolError, DavProtocolErrorKind, Depth,
    Destination, IfHeader, IfResourceGroup, IfStateCondition, IfStateList,
    destination_relative_path, enforce_if_header, evaluate_http_download_preconditions,
    evaluate_http_etag_preconditions, parse_copy_depth, parse_delete_depth, parse_if_header,
    parse_lock_depth, parse_lock_timeout, parse_lock_token_header, parse_move_depth,
    parse_overwrite, parse_propfind_depth, submitted_lock_tokens, submitted_lock_tokens_for_path,
};
pub use put::{
    DavPutPlan, DavPutPlanError, DavPutResourceState, DavPutResponseError, plan_put_request,
    put_plan_error_response, put_success_response,
};
pub use request::{DavBodyPolicy, DavMethod, DavRequestHead, DavRequestOrigin};
pub use resource::{
    DavCopyMoveMethod, DavCopyMovePlan, DavMutationFailure, DavMutationPlanError,
    DavMutationResponseError, collection_created_response, delete_success_response,
    is_descendant_path, mutation_multistatus_response, mutation_plan_error_response,
    mutation_success_response, plan_copy_move_request, resource_identity_path, same_resource_path,
    validate_collection_create_target, validate_delete_target,
};
pub use response::{
    DAV_ALLOW_HEADER, DavBodyError, DavDownloadBody, DavDownloadPlan, DavDownloadPlanError,
    DavResponse, DavResponseBody, backend_error_response, body_error_response,
    method_not_allowed_response, options_response, plan_download_response, protocol_error_response,
    range_not_satisfiable_response,
};
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

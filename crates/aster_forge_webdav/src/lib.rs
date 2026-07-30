//! Product-neutral WebDAV protocol engine contracts for Aster services.
//!
//! This crate owns WebDAV paths, request parsing, protocol preconditions, backend ports,
//! response models, and observable operation events. Product repositories own authentication,
//! authorization, workspace scope, persistence, storage policy, quota, and audit semantics.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo,
        clippy::allow_attributes,
        clippy::allow_attributes_without_reason
    )
)]

#[cfg(feature = "actix")]
pub mod actix;
pub mod backend;
pub mod capability;
pub mod conditional;
pub mod deltav;
pub mod event;
pub mod extension;
pub mod lock;
pub mod multistatus;
pub mod patch;
pub mod path;
pub mod preference;
pub mod property;
pub mod protocol;
pub mod put;
pub mod request;
pub mod resource;
pub mod response;
pub mod traversal;
pub mod xml;
pub mod xml_response;

pub use backend::{
    DavBackendError, DavBackendErrorKind, DavContentStream, DavDirectoryEntry,
    DavDirectoryEnumerator, DavDirectoryPage, DavDirectoryPageRequest, DavDownloadOpenError,
    DavDownloadSource, DavFileSystem, DavIfResourceState, DavIfStateResolver, DavLock,
    DavLockError, DavLockPreflightError, DavLockSystem, DavMetaData, DavOpenedDownload, DavProp,
    DavRandomWriteHandle, DavRandomWriteSystem, DavResourceKind, DavWriteHandle, DavWriteOptions,
    DavWriteSystem, FsError, FsFuture, FsResult, LsFuture,
};
pub use capability::{
    DavAccessControlExtension, DavAccessControlSupport, DavActivityExtension, DavActivitySupport,
    DavAddMemberExtension, DavAddMemberSupport, DavBaselineExtension, DavBaselineSupport,
    DavBindingsExtension, DavBindingsSupport, DavCapabilityContext, DavCapabilityDeclaration,
    DavCapabilityEvaluationError, DavCapabilityPlanError, DavCapabilityProfile,
    DavCapabilityProvider, DavCapabilitySnapshot, DavCapabilityTarget, DavCheckoutInPlaceExtension,
    DavCheckoutInPlaceSupport, DavClass1Profile, DavClass1Support, DavClass2And3Profile,
    DavClass2Profile, DavClass2Support, DavClass3Profile, DavClass3Support,
    DavCollectionSyncExtension, DavCollectionSyncSupport, DavCompatibilityCapabilities,
    DavComplianceClasses, DavCurrentPrincipalExtension, DavCurrentPrincipalSupport,
    DavExtendedMkcolExtension, DavExtendedMkcolSupport, DavExtensionMarker, DavLabelExtension,
    DavLabelSupport, DavLockingCapability, DavMergeExtension, DavMergeSupport, DavMethodGateError,
    DavNonDavProfile, DavOrderedCollectionsExtension, DavOrderedCollectionsSupport,
    DavPartialPutCapability, DavPartialPutSupport, DavPatchBodyPolicy, DavPatchCapability,
    DavPatchFormat, DavPatchSupport, DavPreferExtension, DavPreferSupport,
    DavPrivateUpdateRangeCapability, DavPrivateUpdateRangeSupport, DavQuotaExtension,
    DavQuotaSupport, DavRedirectReferencesExtension, DavRedirectReferencesSupport,
    DavResourceState, DavSearchCapabilities, DavSearchExtension, DavSearchGrammar,
    DavSearchSupport, DavUpdateExtension, DavUpdateSupport, DavVersionControlExtension,
    DavVersionControlSupport, DavVersionControlledCollectionExtension,
    DavVersionControlledCollectionSupport, DavVersionHistoryExtension, DavVersionHistorySupport,
    DavWithExtension, DavWithPartialPut, DavWithPatch, DavWithPrivateUpdateRange,
    DavWorkingResourceExtension, DavWorkingResourceSupport, DavWorkspaceExtension,
    DavWorkspaceSupport, DavWriteCapabilities, DavWritePrecondition, plan_capabilities,
    plan_capabilities_with_provider,
};
pub use conditional::{
    DavConditionalEvaluationError, DavConditionalOutcome, DavConditionalPlan,
    DavConditionalPlanError, DavConditionalResource, DavRangeEvaluation, plan_conditionals,
    plan_conditionals_with_backends, plan_http_conditionals,
};
pub use deltav::{
    DavReportErrorResponsePolicy, DavReportPlanError, plan_report_request,
    report_plan_error_response, validate_version_control_request,
    version_control_request_error_response, version_control_response,
    version_tree_non_file_response, version_tree_response, version_tree_response_with_limits,
};
pub use event::{
    DavEvent, DavEventOutcome, DavEventSink, DavObservationError, DavOperation,
    DavOperationObservations, DavProtocolFailureClass, DavStreamOutcome, NoopDavEventSink,
    publish_non_authoritative,
};
pub use extension::{
    DavExtensionBodyKind, DavExtensionDescriptor, DavExtensionMethod, DavExtensionPackage,
    DavExtensionSet, DavExtensionSetIter, DavLiveProperty, DavPreferenceSet, DavReportType,
    DavResourceStateSet, extension_body_kind, extension_methods,
};
pub use lock::{
    DavLockPlan, DavLockPlanError, enforce_parent_unlocked, enforce_unlocked,
    ensure_lock_target_exists, lock_acquire_success_response, lock_conflict_response,
    lock_discovery_element, lock_limit_response, lock_refresh_success_response,
    lock_xml_error_response, plan_lock_request, unlock_success_response,
    unlock_token_mismatch_response, unsubmitted_lock_conflicts,
};
pub use multistatus::{
    DavMultiStatusError, DavMultiStatusErrorKind, DavMultiStatusLimits, DavMultiStatusProgress,
    DavMultiStatusSourceError, DavMultiStatusStream, DavMultiStatusWriter, dav_multistatus_bytes,
    multistatus_stream_response,
};
pub use patch::{DavPatchPlan, DavPatchPlanError, patch_plan_error_response, plan_patch_request};
pub use path::{
    DavPath, DavPathError, child_relative_path, decode_relative_path, display_name, encode_href,
    href_for_dav_path, href_for_relative, parent_relative_path,
};
pub use preference::{DavPreferencePlan, plan_preferences};
pub use property::{
    DavCurrentPrincipal, DavLivePropertyError, DavLivePropertyEvaluationError,
    DavLivePropertyMetadata, DavLivePropertyProvider, DavLivePropertyRequirements,
    DavLivePropertyValueSnapshot, DavProppatchAtomicPlan, DavQuotaSnapshot,
    build_live_propfind_item, build_live_propfind_item_with_provider, build_proppatch_item,
    format_creation_date, is_protected_live_property, live_property_requirements,
    plan_atomic_proppatch, property_multistatus_response,
    property_multistatus_response_with_limits, propfind_finite_depth_response,
    propfind_request_label, propfind_xml_error_response, proppatch_xml_error_response,
};
pub use protocol::{
    DavIfEvaluationError, DavProtocolError, DavProtocolErrorKind, Depth, Destination, IfHeader,
    IfResourceGroup, IfStateCondition, IfStateList, destination_relative_path, enforce_if_header,
    enforce_if_header_with_backends, parse_copy_depth, parse_delete_depth, parse_if_header,
    parse_lock_depth, parse_lock_timeout, parse_lock_token_header, parse_move_depth,
    parse_overwrite, parse_propfind_depth, submitted_lock_tokens, submitted_lock_tokens_for_path,
};
pub use put::{
    DavPartialPutPlan, DavPutPlan, DavPutPlanError, DavPutResourceState, DavPutResponseError,
    DavPutWritePlan, plan_put_request, put_plan_error_response, put_success_response,
};
pub use request::{
    DavBodyPolicy, DavMethod, DavMethodSet, DavMethodSetIter, DavRequestHead, DavRequestOrigin,
    DavRequestTarget,
};
pub use resource::{
    DavCopyMoveMethod, DavCopyMovePlan, DavMutationFailure, DavMutationPlanError,
    DavMutationResponseError, collection_created_response, delete_success_response,
    enforce_parent_collection, is_descendant_path, mutation_multistatus_response,
    mutation_multistatus_response_with_limits, mutation_plan_error_response,
    mutation_success_response, plan_copy_move_request, replace_relative_prefix,
    resource_identity_path, same_resource_path, validate_collection_create_target,
    validate_delete_target,
};
pub use response::{
    DavBodyError, DavDownloadBody, DavDownloadPlan, DavDownloadPlanError, DavMultiRangeLimits,
    DavMultiRangePolicy, DavMultipartDownloadPlan, DavMultipartSegmentPlan, DavRangeLimitBehavior,
    DavResponse, DavResponseBody, backend_error_response, body_error_response,
    capability_evaluation_error_response, conditional_plan_error_response, gate_method,
    method_not_allowed_response, open_download, options_response, plan_download_response,
    plan_download_response_with_multi_range, protocol_error_response,
    range_not_satisfiable_response,
};
pub use traversal::{
    DavCancellation, DavDirectoryPageLimits, DavDirectoryPageState,
    DavDirectoryPageValidationError, DavDirectoryReadError, DavNeverCancelled, DavTraversalBudget,
    DavTraversalError, DavTraversalErrorKind, DavTraversalLimits, DavTraversalProgress,
    DavValidatedDirectoryPage, read_next_directory_page, validate_directory_page,
};
pub use xml::{
    DavLockRequestBody, DavPropertyPatchRequest, DavPropertyPatchValue, DavPropfindRequest,
    DavRequestedProperty, DavXmlElement, DavXmlError, DavXmlNode, parse_lock_request,
    parse_propfind_request, parse_proppatch_request, parse_report_root,
};
pub use xml_response::{
    DavErrorCondition, DavLockXml, DavMultiStatusItem, DavPropStat, DavVersionXml,
    dav_dead_property_element, dav_element, dav_error_element, dav_lock_discovery_element,
    dav_lock_response_element, dav_property_child_element, dav_property_name_element,
    dav_property_text_element, dav_supported_lock_element, dav_text_element,
    dav_version_multistatus_bytes,
};

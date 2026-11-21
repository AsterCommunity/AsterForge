//! PUT request planning and success response composition.

use std::time::SystemTime;

use headers::{ContentLength, ContentRange, Header};
use http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_LOCATION, CONTENT_RANGE, IF_MATCH, IF_NONE_MATCH,
};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::patch::{DavWritePreconditionError, enforce_write_precondition};
use crate::response::no_store_empty_response;
use crate::{
    DavCapabilitySnapshot, DavConditionalOutcome, DavConditionalPlan, DavConditionalPlanError,
    DavConditionalResource, DavMethod, DavPartialPutCapability, DavPath,
    DavPrivateUpdateRangeCapability, DavProtocolError, DavResponse, DavVersioningPrecondition,
    DavWritePrecondition, href_for_dav_path, plan_http_conditionals, plan_versioning_method,
    versioning_precondition_response,
};

/// Resolved state of the PUT request target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavPutResourceState<'a> {
    /// The target does not exist yet; PUT must create it.
    Missing,
    /// The target is an existing file with the validators reported by the backend.
    File {
        /// Current strong or weak entity-tag of the representation, if any.
        etag: Option<&'a str>,
        /// Last modification time of the representation, if the backend tracks it.
        last_modified: Option<SystemTime>,
    },
    /// The target is a collection, which PUT must never replace.
    Collection,
}

/// Byte range selected for a private-agreement partial PUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavPartialPutPlan {
    /// Start offset of the range relative to the beginning of the representation.
    pub offset: u64,
    /// Number of content bytes the request body contributes at `offset`.
    pub length: u64,
    /// Total representation length after this write, when the client declares it.
    pub complete_length: Option<u64>,
}

/// Product write operation selected from the declared PUT compatibility capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavPutWritePlan {
    /// Replace the complete representation using ordinary PUT semantics.
    Replace,
    /// Replace one byte range relative to the current selected representation.
    Partial(DavPartialPutPlan),
    /// Execute the explicitly enabled private header contract in the product adapter.
    PrivateUpdateRange {
        /// Raw private header value the product adapter must interpret.
        value: HeaderValue,
    },
}

/// Product-side open/write settings selected from capabilities and HTTP preconditions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavPutPlan {
    /// Whether the target had a stored representation before this request.
    pub resource_existed: bool,
    /// Whether the backend should create the target when it is absent.
    pub create: bool,
    /// Whether the contract requires the target to be absent, failing if it exists.
    pub create_new: bool,
    /// Content-Length declared by the request, used only as a size hint.
    pub content_length_hint: Option<u64>,
    /// Write operation the product adapter must execute.
    pub write: DavPutWritePlan,
}

/// Failure while planning a PUT request.
#[derive(Debug, thiserror::Error)]
pub enum DavPutPlanError {
    /// PUT is not declared for the resource kind.
    #[error("PUT is not allowed for this resource")]
    MethodNotAllowed,
    /// The request head violated a protocol contract while parsing PUT inputs.
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    /// An evaluated `If` or entity-tag conditional failed for the target.
    #[error("PUT request precondition failed")]
    PreconditionFailed(DavConditionalPlan),
    /// Content-Length or range metadata was structurally invalid.
    #[error("invalid PUT representation metadata")]
    InvalidRepresentation,
    /// A partial or range write lacks the required strong `If-Match` entity-tag.
    #[error("partial PUT requires If-Match with a strong entity-tag")]
    PreconditionRequired,
    /// A partial write targeted a representation that does not exist.
    #[error("partial PUT requires an existing representation")]
    PartialTargetMissing,
    /// PUT attempted to replace a collection instead of a file.
    #[error("PUT cannot replace a collection")]
    CollectionTarget,
    /// The target is version-controlled and its versioning precondition rejected the write.
    #[error(transparent)]
    Versioning(#[from] DavVersioningPrecondition),
}

/// Failure while composing the successful PUT response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid PUT Content-Location response header")]
pub struct DavPutResponseError;

/// Evaluates PUT preconditions and selects product-side create/open settings.
///
/// # Errors
///
/// Returns [`DavPutPlanError`] when capabilities, validators, or range headers reject the write.
pub fn plan_put_request(
    snapshot: &DavCapabilitySnapshot,
    headers: &HeaderMap,
    state: DavPutResourceState<'_>,
) -> Result<DavPutPlan, DavPutPlanError> {
    if !snapshot.allows(DavMethod::Put) {
        return Err(DavPutPlanError::MethodNotAllowed);
    }
    plan_versioning_method(snapshot, DavMethod::Put)?;
    let resource = match state {
        DavPutResourceState::Missing => DavConditionalResource::missing(),
        DavPutResourceState::File {
            etag,
            last_modified,
        } => DavConditionalResource {
            exists: true,
            etag,
            last_modified,
        },
        DavPutResourceState::Collection => return Err(DavPutPlanError::CollectionTarget),
    };
    let (write, precondition) = plan_put_write(snapshot, headers)?;
    if matches!(write, DavPutWritePlan::Partial(_)) && !resource.exists {
        return Err(DavPutPlanError::PartialTargetMissing);
    }
    enforce_write_precondition(precondition, headers).map_err(|error| match error {
        DavWritePreconditionError::Required => DavPutPlanError::PreconditionRequired,
        DavWritePreconditionError::Protocol(error) => DavPutPlanError::Protocol(error),
    })?;
    let conditional =
        plan_http_conditionals(DavMethod::Put, headers, resource).map_err(|error| match error {
            DavConditionalPlanError::Protocol(error) => DavPutPlanError::Protocol(error),
            DavConditionalPlanError::InvalidRepresentation => {
                DavPutPlanError::InvalidRepresentation
            }
        })?;
    if conditional.outcome != DavConditionalOutcome::Proceed {
        return Err(DavPutPlanError::PreconditionFailed(conditional));
    }
    let content_length_hint = match &write {
        DavPutWritePlan::Partial(partial) => Some(partial.length),
        DavPutWritePlan::Replace | DavPutWritePlan::PrivateUpdateRange { .. } => {
            content_length_hint(headers)
        }
    };
    Ok(DavPutPlan {
        resource_existed: resource.exists,
        create: !header_equals(headers, IF_MATCH, "*"),
        create_new: header_equals(headers, IF_NONE_MATCH, "*"),
        content_length_hint,
        write,
    })
}

/// Maps a PUT planning failure through the same snapshot used for method planning.
#[must_use]
pub fn put_plan_error_response(
    snapshot: &DavCapabilitySnapshot,
    error: &DavPutPlanError,
) -> DavResponse {
    match error {
        DavPutPlanError::MethodNotAllowed | DavPutPlanError::CollectionTarget => {
            crate::method_not_allowed_response(snapshot)
        }
        DavPutPlanError::Protocol(error) => crate::protocol_error_response(error),
        DavPutPlanError::PreconditionFailed(plan) => {
            let mut response = DavResponse::empty(StatusCode::PRECONDITION_FAILED);
            response
                .headers
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            plan.apply_response_headers(response.status, &mut response.headers);
            response
        }
        DavPutPlanError::InvalidRepresentation => {
            no_store_empty_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
        DavPutPlanError::PreconditionRequired => {
            no_store_empty_response(StatusCode::PRECONDITION_REQUIRED)
        }
        DavPutPlanError::PartialTargetMissing => no_store_empty_response(StatusCode::CONFLICT),
        DavPutPlanError::Versioning(error) => versioning_precondition_response(snapshot, *error)
            .unwrap_or_else(|_| no_store_empty_response(StatusCode::INTERNAL_SERVER_ERROR)),
    }
}

fn plan_put_write(
    snapshot: &DavCapabilitySnapshot,
    headers: &HeaderMap,
) -> Result<(DavPutWritePlan, DavWritePrecondition), DavPutPlanError> {
    let has_content_range = headers.contains_key(CONTENT_RANGE);
    let has_private_range = headers.contains_key("X-Update-Range");
    if has_content_range && has_private_range {
        return Err(DavProtocolError::bad_request(
            "Content-Range and X-Update-Range cannot be combined",
        )
        .into());
    }
    if has_content_range {
        let (plan, precondition) = plan_partial_put(snapshot, headers)?;
        return Ok((DavPutWritePlan::Partial(plan), precondition));
    }
    if has_private_range {
        return plan_private_update_range(snapshot, headers);
    }
    Ok((DavPutWritePlan::Replace, DavWritePrecondition::Optional))
}

fn plan_partial_put(
    snapshot: &DavCapabilitySnapshot,
    headers: &HeaderMap,
) -> Result<(DavPartialPutPlan, DavWritePrecondition), DavPutPlanError> {
    let precondition = match snapshot.writes().partial_put {
        DavPartialPutCapability::Disabled => {
            return Err(DavProtocolError::bad_request("Partial PUT is not supported").into());
        }
        DavPartialPutCapability::ContentRangeBytes { precondition } => precondition,
    };
    let mut values = headers.get_all(CONTENT_RANGE).iter();
    let value = values
        .next()
        .ok_or_else(|| DavProtocolError::bad_request("Invalid Content-Range header"))?;
    if values.next().is_some() {
        return Err(DavProtocolError::bad_request("Invalid Content-Range header").into());
    }
    let content_range = ContentRange::decode(&mut std::iter::once(value))
        .map_err(|_| DavProtocolError::bad_request("Invalid Content-Range header"))?;
    let (first, last) = content_range
        .bytes_range()
        .ok_or_else(|| DavProtocolError::bad_request("Invalid Content-Range header"))?;
    if content_range
        .bytes_len()
        .is_some_and(|complete_length| complete_length <= last)
    {
        return Err(DavProtocolError::bad_request("Invalid Content-Range header").into());
    }
    let length = last
        .checked_sub(first)
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| DavProtocolError::bad_request("Invalid Content-Range header"))?;
    if let Some(content_length) = request_content_length(headers)?
        && content_length != length
    {
        return Err(
            DavProtocolError::bad_request("Content-Length does not match Content-Range").into(),
        );
    }
    Ok((
        DavPartialPutPlan {
            offset: first,
            length,
            complete_length: content_range.bytes_len(),
        },
        precondition,
    ))
}

fn plan_private_update_range(
    snapshot: &DavCapabilitySnapshot,
    headers: &HeaderMap,
) -> Result<(DavPutWritePlan, DavWritePrecondition), DavPutPlanError> {
    let precondition = match snapshot.writes().private_update_range {
        DavPrivateUpdateRangeCapability::Disabled => {
            return Err(DavProtocolError::bad_request("X-Update-Range is not supported").into());
        }
        DavPrivateUpdateRangeCapability::XUpdateRange { precondition } => precondition,
    };
    let mut values = headers.get_all("X-Update-Range").iter();
    let value = values
        .next()
        .ok_or_else(|| DavProtocolError::bad_request("Invalid X-Update-Range header"))?;
    if values.next().is_some() || value.to_str().map_or(true, |value| value.trim().is_empty()) {
        return Err(DavProtocolError::bad_request("Invalid X-Update-Range header").into());
    }
    Ok((
        DavPutWritePlan::PrivateUpdateRange {
            value: value.clone(),
        },
        precondition,
    ))
}

fn request_content_length(headers: &HeaderMap) -> Result<Option<u64>, DavPutPlanError> {
    if !headers.contains_key(CONTENT_LENGTH) {
        return Ok(None);
    }
    let content_length = ContentLength::decode(&mut headers.get_all(CONTENT_LENGTH).iter())
        .map_err(|_| DavProtocolError::bad_request("Invalid Content-Length header"))?;
    Ok(Some(content_length.0))
}

/// Builds the 201/204 response selected by the pre-write resource state.
///
/// # Errors
///
/// Returns [`DavPutResponseError`] when the success response headers cannot be encoded.
pub fn put_success_response(
    plan: &DavPutPlan,
    prefix: &str,
    path: &DavPath,
) -> Result<DavResponse, DavPutResponseError> {
    if plan.resource_existed {
        return Ok(DavResponse::empty(StatusCode::NO_CONTENT));
    }
    let mut response = DavResponse::empty(StatusCode::CREATED);
    let location =
        HeaderValue::from_str(&href_for_dav_path(prefix, path)).map_err(|_| DavPutResponseError)?;
    response.headers.insert(CONTENT_LOCATION, location);
    Ok(response)
}

fn content_length_hint(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("X-Expected-Entity-Length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .or_else(|| {
            headers
                .get(http::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
}

fn header_equals(headers: &HeaderMap, name: http::header::HeaderName, expected: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == expected)
}

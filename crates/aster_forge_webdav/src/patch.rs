//! RFC 5789 PATCH format dispatch and request planning.

use aster_forge_utils::http_validators;
use headers::{ContentType, Header, Mime};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::response::no_store_empty_response;
use crate::{
    DavBodyPolicy, DavCapabilitySnapshot, DavConditionalOutcome, DavConditionalPlan,
    DavConditionalPlanError, DavConditionalResource, DavMethod, DavPatchBodyPolicy, DavPatchFormat,
    DavProtocolError, DavResponse, DavWritePrecondition, method_not_allowed_response,
    plan_http_conditionals,
};

/// Selected patch format and transport policy after capability and precondition validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavPatchPlan {
    pub format: &'static DavPatchFormat,
    pub body_policy: DavBodyPolicy,
    pub resource_existed: bool,
}

/// Failure while selecting and validating an RFC 5789 PATCH request.
#[derive(Debug, thiserror::Error)]
pub enum DavPatchPlanError {
    #[error("PATCH is not allowed for this resource")]
    MethodNotAllowed,
    #[error("PATCH Content-Type is not supported for this resource")]
    UnsupportedMediaType,
    #[error("PATCH requires If-Match with a strong entity-tag")]
    PreconditionRequired,
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    #[error("PATCH request precondition failed")]
    PreconditionFailed(DavConditionalPlan),
    #[error("invalid PATCH representation metadata")]
    InvalidRepresentation,
}

/// Selects a declared patch document format and applies its conditional request contract.
///
/// # Errors
///
/// Returns [`DavPatchPlanError`] when PATCH is unavailable or its media type is invalid.
pub fn plan_patch_request(
    snapshot: &DavCapabilitySnapshot,
    headers: &HeaderMap,
    resource: DavConditionalResource<'_>,
) -> Result<DavPatchPlan, DavPatchPlanError> {
    let Some(formats) = snapshot.patch_formats() else {
        return Err(DavPatchPlanError::MethodNotAllowed);
    };
    let content_type = parse_content_type(headers)?;
    let format = formats
        .iter()
        .find(|format| {
            format
                .media_type
                .parse::<Mime>()
                .is_ok_and(|supported| supported == content_type)
        })
        .ok_or(DavPatchPlanError::UnsupportedMediaType)?;

    enforce_write_precondition(format.precondition, headers).map_err(|error| match error {
        DavWritePreconditionError::Required => DavPatchPlanError::PreconditionRequired,
        DavWritePreconditionError::Protocol(error) => DavPatchPlanError::Protocol(error),
    })?;
    let conditional = plan_http_conditionals(DavMethod::Patch, headers, resource).map_err(
        |error| match error {
            DavConditionalPlanError::Protocol(error) => DavPatchPlanError::Protocol(error),
            DavConditionalPlanError::InvalidRepresentation => {
                DavPatchPlanError::InvalidRepresentation
            }
        },
    )?;
    if conditional.outcome != DavConditionalOutcome::Proceed {
        return Err(DavPatchPlanError::PreconditionFailed(conditional));
    }

    let body_policy = match format.body_policy {
        DavPatchBodyPolicy::Bounded { maximum } => DavBodyPolicy::Bounded { maximum },
        DavPatchBodyPolicy::Stream => DavBodyPolicy::Stream,
    };
    Ok(DavPatchPlan {
        format,
        body_policy,
        resource_existed: resource.exists,
    })
}

/// Maps PATCH planning failures to the RFC 5789 response contract.
#[must_use]
pub fn patch_plan_error_response(
    error: &DavPatchPlanError,
    snapshot: &DavCapabilitySnapshot,
) -> DavResponse {
    match error {
        DavPatchPlanError::MethodNotAllowed => method_not_allowed_response(snapshot),
        DavPatchPlanError::UnsupportedMediaType => {
            let mut response = no_store_empty_response(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            if let Some(accept_patch) = snapshot.accept_patch_header() {
                response
                    .headers
                    .insert("Accept-Patch", accept_patch.clone());
            }
            response
        }
        DavPatchPlanError::PreconditionRequired => {
            no_store_empty_response(StatusCode::PRECONDITION_REQUIRED)
        }
        DavPatchPlanError::Protocol(error) => crate::protocol_error_response(error),
        DavPatchPlanError::PreconditionFailed(plan) => {
            let mut response = DavResponse::empty(StatusCode::PRECONDITION_FAILED);
            response
                .headers
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            plan.apply_response_headers(response.status, &mut response.headers);
            response
        }
        DavPatchPlanError::InvalidRepresentation => {
            no_store_empty_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub(crate) enum DavWritePreconditionError {
    Required,
    Protocol(DavProtocolError),
}

pub(crate) fn enforce_write_precondition(
    policy: DavWritePrecondition,
    headers: &HeaderMap,
) -> Result<(), DavWritePreconditionError> {
    if policy == DavWritePrecondition::Optional {
        return Ok(());
    }
    match http_validators::if_match_headers_have_strong_tag(headers) {
        Ok(Some(true)) => Ok(()),
        Ok(Some(false) | None) => Err(DavWritePreconditionError::Required),
        Err(_) => Err(DavWritePreconditionError::Protocol(
            DavProtocolError::bad_request("Invalid If-Match header"),
        )),
    }
}

fn parse_content_type(headers: &HeaderMap) -> Result<Mime, DavPatchPlanError> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let Some(value) = values.next() else {
        return Err(DavPatchPlanError::UnsupportedMediaType);
    };
    if values.next().is_some() {
        return Err(DavPatchPlanError::UnsupportedMediaType);
    }
    let content_type = ContentType::decode(&mut std::iter::once(value))
        .map_err(|_| DavPatchPlanError::UnsupportedMediaType)?;
    Ok(content_type.into())
}

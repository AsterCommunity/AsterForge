//! PUT request planning and success response composition.

use http::header::{CACHE_CONTROL, CONTENT_LOCATION, IF_MATCH, IF_NONE_MATCH};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::{
    DavPath, DavProtocolError, DavResponse, evaluate_http_etag_preconditions, href_for_dav_path,
};

/// Resolved state of the PUT request target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavPutResourceState<'a> {
    Missing,
    File { etag: Option<&'a str> },
    Collection,
}

/// Product-side open/write settings selected from HTTP preconditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavPutPlan {
    pub resource_existed: bool,
    pub create: bool,
    pub create_new: bool,
    pub content_length_hint: Option<u64>,
}

/// Failure while planning a PUT request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavPutPlanError {
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    #[error("PUT cannot replace a collection")]
    CollectionTarget,
}

/// Failure while composing the successful PUT response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid PUT Content-Location response header")]
pub struct DavPutResponseError;

/// Evaluates PUT preconditions and selects product-side create/open settings.
pub fn plan_put_request(
    headers: &HeaderMap,
    state: DavPutResourceState<'_>,
) -> Result<DavPutPlan, DavPutPlanError> {
    let (resource_existed, etag) = match state {
        DavPutResourceState::Missing => (false, None),
        DavPutResourceState::File { etag } => (true, etag),
        DavPutResourceState::Collection => return Err(DavPutPlanError::CollectionTarget),
    };
    evaluate_http_etag_preconditions(headers, resource_existed, etag, false)?;
    Ok(DavPutPlan {
        resource_existed,
        create: !header_equals(headers, IF_MATCH, "*"),
        create_new: header_equals(headers, IF_NONE_MATCH, "*"),
        content_length_hint: content_length_hint(headers),
    })
}

/// Maps a PUT planning failure to its protocol response.
#[must_use]
pub fn put_plan_error_response(error: &DavPutPlanError) -> DavResponse {
    match error {
        DavPutPlanError::Protocol(error) => crate::protocol_error_response(error),
        DavPutPlanError::CollectionTarget => {
            let mut response = DavResponse::empty(StatusCode::METHOD_NOT_ALLOWED);
            response
                .headers
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
    }
}

/// Builds the 201/204 response selected by the pre-write resource state.
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

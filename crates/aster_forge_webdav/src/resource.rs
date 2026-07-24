//! Resource mutation path rules and response composition.

use http::header::{CACHE_CONTROL, CONTENT_LOCATION, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};

use crate::{
    DavErrorCondition, DavMultiStatusItem, DavPath, DavResourceKind, DavResponse, DavXmlError,
    Depth, dav_multistatus_element, href_for_dav_path,
};

/// COPY or MOVE operation selected by the request method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavCopyMoveMethod {
    Copy,
    Move,
}

/// Resource-shape decisions needed by the Drive mutation adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavCopyMovePlan {
    pub recursive_collection: bool,
    pub destination_deep: bool,
}

/// Protocol failure selected after source/destination metadata is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavMutationPlanError {
    #[error("invalid mutation Depth")]
    BadRequest,
    #[error("resource mutation is not supported for this target")]
    MethodNotAllowed,
    #[error("resource mutation conflicts with the current hierarchy")]
    Conflict,
    #[error("forbidden mutation path relation")]
    Forbidden,
    #[error("destination exists while Overwrite is disabled")]
    PreconditionFailed,
}

/// Rejects collection creation for the DAV root resource.
pub fn validate_collection_create_target(path: &str) -> Result<(), DavMutationPlanError> {
    if resource_identity_path(path) == "/" {
        Err(DavMutationPlanError::MethodNotAllowed)
    } else {
        Ok(())
    }
}

/// Failure while composing a mutation success response header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid mutation Content-Location response header")]
pub struct DavMutationResponseError;

/// One resource-level failure from a recursive COPY, MOVE, or DELETE operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavMutationFailure {
    path: DavPath,
    status: u16,
    lock_path: Option<DavPath>,
}

impl DavMutationFailure {
    /// Creates a recursive mutation failure caused by an unsubmitted lock token.
    #[must_use]
    pub fn locked(path: DavPath, lock_path: DavPath) -> Self {
        Self {
            path,
            status: StatusCode::LOCKED.as_u16(),
            lock_path: Some(lock_path),
        }
    }

    /// Creates a recursive mutation failure with an explicit protocol status.
    #[must_use]
    pub fn status(path: DavPath, status: u16) -> Self {
        Self {
            path,
            status,
            lock_path: None,
        }
    }
}

/// Enforces collection DELETE Depth after product metadata resolution.
pub fn validate_delete_target(
    kind: DavResourceKind,
    depth: Depth,
) -> Result<(), DavMutationPlanError> {
    if kind == DavResourceKind::Collection && !depth.is_infinity() {
        Err(DavMutationPlanError::BadRequest)
    } else {
        Ok(())
    }
}

/// Plans COPY/MOVE resource-shape behavior after product metadata resolution.
pub fn plan_copy_move_request(
    method: DavCopyMoveMethod,
    depth: Depth,
    source_kind: DavResourceKind,
    destination_kind: Option<DavResourceKind>,
    source_path: &str,
    destination_path: &str,
    overwrite: bool,
) -> Result<DavCopyMovePlan, DavMutationPlanError> {
    if same_resource_path(source_path, destination_path) {
        return Err(DavMutationPlanError::Forbidden);
    }
    if source_kind == DavResourceKind::Collection {
        match method {
            DavCopyMoveMethod::Move if !depth.is_infinity() => {
                return Err(DavMutationPlanError::BadRequest);
            }
            DavCopyMoveMethod::Copy if depth == Depth::One => {
                return Err(DavMutationPlanError::BadRequest);
            }
            DavCopyMoveMethod::Copy | DavCopyMoveMethod::Move => {}
        }
    }
    let recursive_collection = source_kind == DavResourceKind::Collection
        && (method == DavCopyMoveMethod::Move || depth != Depth::Zero);
    if recursive_collection && is_descendant_path(source_path, destination_path) {
        return Err(DavMutationPlanError::Forbidden);
    }
    if !overwrite && destination_kind.is_some() {
        return Err(DavMutationPlanError::PreconditionFailed);
    }
    let destination_deep = destination_kind == Some(DavResourceKind::Collection)
        || source_kind == DavResourceKind::Collection
            && (method == DavCopyMoveMethod::Move || depth != Depth::Zero);
    Ok(DavCopyMovePlan {
        recursive_collection,
        destination_deep,
    })
}

/// Builds an empty response for resource-shape validation failure.
#[must_use]
pub fn mutation_plan_error_response(error: DavMutationPlanError) -> DavResponse {
    let status = match error {
        DavMutationPlanError::BadRequest => StatusCode::BAD_REQUEST,
        DavMutationPlanError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        DavMutationPlanError::Conflict => StatusCode::CONFLICT,
        DavMutationPlanError::Forbidden => StatusCode::FORBIDDEN,
        DavMutationPlanError::PreconditionFailed => StatusCode::PRECONDITION_FAILED,
    };
    let mut response = DavResponse::empty(status);
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Builds the successful MKCOL response.
pub fn collection_created_response(
    prefix: &str,
    path: &DavPath,
) -> Result<DavResponse, DavMutationResponseError> {
    let mut response = DavResponse::empty(StatusCode::CREATED);
    let location = HeaderValue::from_str(&href_for_dav_path(prefix, path))
        .map_err(|_| DavMutationResponseError)?;
    response.headers.insert(CONTENT_LOCATION, location);
    Ok(response)
}

/// Builds the successful DELETE response.
#[must_use]
pub fn delete_success_response() -> DavResponse {
    DavResponse::empty(StatusCode::NO_CONTENT)
}

/// Compares DAV resource identity while ignoring collection trailing slashes.
#[must_use]
pub fn same_resource_path(left: &str, right: &str) -> bool {
    resource_identity_path(left) == resource_identity_path(right)
}

/// Returns whether `child` is strictly below `parent` on a DAV path-segment boundary.
#[must_use]
pub fn is_descendant_path(parent: &str, child: &str) -> bool {
    let parent = resource_identity_path(parent);
    let child = resource_identity_path(child);
    if parent == "/" || parent == child {
        return false;
    }
    child.starts_with(&format!("{parent}/"))
}

/// Builds the cache-safe 201/204 response selected by destination existence.
#[must_use]
pub fn mutation_success_response(destination_existed: bool) -> DavResponse {
    let status = if destination_existed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    let mut response = DavResponse::empty(status);
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Builds a 207 response for typed recursive mutation failures.
pub fn mutation_multistatus_response(
    prefix: &str,
    failures: &[DavMutationFailure],
) -> Result<DavResponse, DavXmlError> {
    let items = failures
        .iter()
        .map(|failure| {
            let item = DavMultiStatusItem::status(
                href_for_dav_path(prefix, &failure.path),
                failure.status,
            );
            if failure.status == StatusCode::LOCKED.as_u16() {
                let lock_path = failure.lock_path.as_ref().unwrap_or(&failure.path);
                item.with_error(DavErrorCondition::LockTokenSubmitted {
                    href: href_for_dav_path(prefix, lock_path),
                })
            } else {
                item
            }
        })
        .collect();
    let body = dav_multistatus_element(items).to_bytes()?;
    let mut response = DavResponse::bytes(StatusCode::MULTI_STATUS, body);
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Normalizes a DAV resource identity by removing collection trailing slashes.
#[must_use]
pub fn resource_identity_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

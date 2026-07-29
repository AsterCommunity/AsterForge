//! LOCK/UNLOCK protocol request planning and response composition.

use std::time::Duration;

use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::DavLockSystem;
use crate::response::{no_store_empty_response, xml_request_error_response};
use crate::{
    DavBackendError, DavErrorCondition, DavFileSystem, DavLock, DavLockXml, DavPath,
    DavProtocolError, DavRequestHead, DavResponse, DavWriteHandle, DavWriteOptions, DavWriteSystem,
    DavXmlElement, DavXmlError, FsError, IfHeader, dav_error_element, dav_lock_discovery_element,
    dav_lock_response_element, href_for_dav_path, parent_relative_path, parse_lock_request,
    parse_lock_timeout, protocol_error_response, submitted_lock_tokens,
};

/// Backend operation selected from a LOCK request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavLockPlan {
    Acquire {
        owner: Option<DavXmlElement>,
        timeout: Duration,
        shared: bool,
        deep: bool,
    },
    Refresh {
        token: String,
        timeout: Duration,
    },
}

/// Failure while parsing and selecting a LOCK operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavLockPlanError {
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    #[error(transparent)]
    Xml(#[from] DavXmlError),
}

/// Rejects an operation when a conflicting lock token was not submitted for its lock root.
pub async fn enforce_unlocked(
    lock_system: &dyn DavLockSystem,
    path: &DavPath,
    deep: bool,
    prefix: &str,
    if_header: Option<&IfHeader>,
    request_scheme: &str,
    request_host: &str,
) -> Result<(), DavResponse> {
    if let Some(lock) = unsubmitted_lock_conflicts(
        lock_system,
        path,
        deep,
        prefix,
        if_header,
        request_scheme,
        request_host,
    )
    .await
    .into_iter()
    .next()
    {
        return Err(lock_conflict_response(prefix, &lock.path)
            .unwrap_or_else(|_| DavResponse::empty(StatusCode::INTERNAL_SERVER_ERROR)));
    }
    Ok(())
}

/// Returns conflicting locks whose tokens were not submitted for their corresponding lock root.
pub async fn unsubmitted_lock_conflicts(
    lock_system: &dyn DavLockSystem,
    path: &DavPath,
    deep: bool,
    prefix: &str,
    if_header: Option<&IfHeader>,
    request_scheme: &str,
    request_host: &str,
) -> Vec<crate::DavLock> {
    let mut conflicts = lock_system.conflicting_locks(path, deep).await;
    conflicts.retain(|lock| {
        let lock_href = href_for_dav_path(prefix, &lock.path);
        let submitted_tokens = if_header.map_or_else(Vec::new, |if_header| {
            submitted_lock_tokens(if_header, &lock_href, request_scheme, request_host)
        });
        !submitted_tokens.iter().any(|token| token == &lock.token)
    });
    conflicts
}

/// Applies [`enforce_unlocked`] to the canonical parent of a mutation target.
pub async fn enforce_parent_unlocked(
    lock_system: &dyn DavLockSystem,
    path: &DavPath,
    prefix: &str,
    if_header: Option<&IfHeader>,
    request_scheme: &str,
    request_host: &str,
) -> Result<(), DavResponse> {
    let Some(parent) = parent_relative_path(path.as_str()) else {
        return Ok(());
    };
    let parent_path = DavPath::new(&parent).map_err(|_| {
        protocol_error_response(&DavProtocolError::bad_request("Invalid request path"))
    })?;
    enforce_unlocked(
        lock_system,
        &parent_path,
        false,
        prefix,
        if_header,
        request_scheme,
        request_host,
    )
    .await
}

/// Ensures that a LOCK target exists, creating an empty lock-null file when allowed.
///
/// Existing resources are left untouched. A missing collection target remains missing because
/// creating its hierarchy is outside LOCK semantics.
pub async fn ensure_lock_target_exists<WriteSystem: DavWriteSystem>(
    filesystem: &dyn DavFileSystem,
    write_system: &WriteSystem,
    path: &DavPath,
) -> Result<bool, DavBackendError> {
    match filesystem.metadata(path).await {
        Ok(_) => Ok(true),
        Err(FsError::NotFound) if !path.is_collection() => {
            let file = write_system
                .open_write(
                    path,
                    DavWriteOptions {
                        create: true,
                        create_new: true,
                        truncate: true,
                        expected_length: Some(0),
                        ..DavWriteOptions::default()
                    },
                )
                .await?;
            file.finish().await?;
            Ok(false)
        }
        Err(FsError::NotFound) => Err(FsError::NotFound.into()),
        Err(error) => Err(error.into()),
    }
}

/// Selects lock acquisition or refresh and validates all protocol-owned inputs.
pub fn plan_lock_request(
    headers: &HeaderMap,
    body: &[u8],
    request_head: &DavRequestHead,
    prefix: &str,
    maximum_timeout: Duration,
) -> Result<DavLockPlan, DavLockPlanError> {
    let timeout = parse_lock_timeout(headers, maximum_timeout)?;
    if body.is_empty() {
        let request_href = href_for_dav_path(prefix, &request_head.target);
        let tokens = request_head
            .if_header
            .as_ref()
            .map_or_else(Vec::new, |if_header| {
                submitted_lock_tokens(
                    if_header,
                    &request_href,
                    &request_head.origin.scheme,
                    &request_head.origin.host,
                )
            });
        if tokens.len() != 1 {
            return Err(DavProtocolError::bad_request("Invalid LOCK refresh token").into());
        }
        return Ok(DavLockPlan::Refresh {
            token: tokens[0].clone(),
            timeout,
        });
    }

    let request = parse_lock_request(body)?;
    let depth = request_head
        .depth
        .ok_or_else(|| DavProtocolError::bad_request("LOCK Depth was not parsed"))?;
    Ok(DavLockPlan::Acquire {
        owner: request.owner,
        timeout,
        shared: request.shared,
        deep: depth.is_infinity(),
    })
}

fn lock_success_response(
    lock: &DavLock,
    status: StatusCode,
    prefix: &str,
    include_lock_token_header: bool,
) -> Result<DavResponse, DavXmlError> {
    let body = dav_lock_response_element(&[DavLockXml {
        token: lock.token.clone(),
        owner: lock.owner.as_deref().cloned(),
        timeout: lock.timeout,
        shared: lock.shared,
        deep: lock.deep,
        root_href: href_for_dav_path(prefix, &lock.path),
    }])
    .to_bytes()?;
    let mut response = DavResponse::bytes(status, body);
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    if include_lock_token_header {
        let value = HeaderValue::from_str(&format!("<{}>", lock.token))
            .map_err(|_| DavXmlError::Malformed)?;
        response.headers.insert("Lock-Token", value);
    }
    Ok(response)
}

/// Builds the successful response for a LOCK refresh.
pub fn lock_refresh_success_response(
    lock: &DavLock,
    prefix: &str,
) -> Result<DavResponse, DavXmlError> {
    lock_success_response(lock, StatusCode::OK, prefix, false)
}

/// Builds the 200/201 response for a LOCK acquisition.
pub fn lock_acquire_success_response(
    lock: &DavLock,
    prefix: &str,
    resource_existed: bool,
) -> Result<DavResponse, DavXmlError> {
    lock_success_response(
        lock,
        if resource_existed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        prefix,
        true,
    )
}

/// Builds the `DAV:lockdiscovery` property from backend lock values.
#[must_use]
pub fn lock_discovery_element(locks: &[DavLock], prefix: &str) -> DavXmlElement {
    let locks = locks
        .iter()
        .map(|lock| DavLockXml {
            token: lock.token.clone(),
            owner: lock.owner.as_deref().cloned(),
            timeout: lock.timeout,
            shared: lock.shared,
            deep: lock.deep,
            root_href: href_for_dav_path(prefix, &lock.path),
        })
        .collect::<Vec<_>>();
    dav_lock_discovery_element(&locks)
}

/// Builds a 423 response identifying the lock whose token must be submitted.
pub fn lock_conflict_response(prefix: &str, path: &DavPath) -> Result<DavResponse, DavXmlError> {
    lock_condition_response(
        StatusCode::LOCKED,
        DavErrorCondition::LockTokenSubmitted {
            href: href_for_dav_path(prefix, path),
        },
    )
}

/// Builds the 409 response for an UNLOCK token that does not match the request URI.
pub fn unlock_token_mismatch_response() -> Result<DavResponse, DavXmlError> {
    lock_condition_response(
        StatusCode::CONFLICT,
        DavErrorCondition::LockTokenMatchesRequestUri,
    )
}

/// Builds the successful cache-safe UNLOCK response.
#[must_use]
pub fn unlock_success_response() -> DavResponse {
    no_store_empty_response(StatusCode::NO_CONTENT)
}

/// Builds the active-lock capacity response.
#[must_use]
pub fn lock_limit_response() -> DavResponse {
    let mut response = DavResponse::bytes(
        StatusCode::INSUFFICIENT_STORAGE,
        "WebDAV active lock limit exceeded",
    );
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Maps LOCK XML failures to their protocol response.
pub fn lock_xml_error_response(error: DavXmlError) -> Result<DavResponse, DavXmlError> {
    xml_request_error_response(error, "Invalid LOCK body")
}

fn lock_condition_response(
    status: StatusCode,
    condition: DavErrorCondition,
) -> Result<DavResponse, DavXmlError> {
    let mut response = DavResponse::bytes(status, dav_error_element(&condition).to_bytes()?);
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

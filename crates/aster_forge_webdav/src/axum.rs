//! Optional Axum transport adapter for the `WebDAV` protocol model.

use axum::{
    body::{Body, to_bytes},
    extract::Request,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};

use crate::{
    DavBodyPolicy, DavMethod, DavRequestHead, DavRequestOrigin, DavRequestTarget, DavResponse,
    DavResponseBody, protocol::DavProtocolError,
};

pub enum DavPreparedBody {
    None,
    Xml(Vec<u8>),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavBodyError {
    #[error("failed to read WebDAV request body")]
    ReadFailed,
    #[error("WebDAV request body is too large")]
    BodyTooLarge,
    #[error("WebDAV method does not accept a request body")]
    BodyNotAllowed,
}

/// Parses an Axum request into a transport-neutral request head.
///
/// # Errors
///
/// Returns a protocol error when the request target or headers are invalid.
pub fn request_head(
    request: &Request,
    mount_path: &str,
) -> Result<Option<DavRequestHead>, DavProtocolError> {
    let target = request_target(request, mount_path)?;
    let Some(method) = DavMethod::from_name(request.method().as_str()) else {
        return Ok(None);
    };
    let headers = convert_header_map(request.headers())?;
    DavRequestHead::parse_known_method(method, &target, &headers).map(Some)
}

/// Parses and validates the request target.
///
/// # Errors
///
/// Returns a protocol error when the URI or mount path is invalid.
pub fn request_target<'a>(
    request: &Request,
    mount_path: &'a str,
) -> Result<DavRequestTarget<'a>, DavProtocolError> {
    let uri: Uri = request
        .uri()
        .to_string()
        .parse()
        .map_err(|_| DavProtocolError::bad_request("Invalid request URI"))?;
    let origin = DavRequestOrigin {
        scheme: request
            .uri()
            .scheme_str()
            .or_else(|| {
                request
                    .headers()
                    .get("x-forwarded-proto")
                    .and_then(|v| v.to_str().ok())
            })
            .unwrap_or("http")
            .to_string(),
        host: request
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string(),
    };
    DavRequestHead::parse_target(&uri, mount_path, &origin)
}

pub fn into_response(response: DavResponse) -> Response {
    let mut builder = Response::builder().status(response.status);
    for (name, value) in &response.headers {
        builder = builder.header(name, value);
    }
    let body =
        match response.body {
            DavResponseBody::Empty => Body::empty(),
            DavResponseBody::Bytes(body) => Body::from(body),
            DavResponseBody::Stream(stream) => Body::from_stream(stream.map(|item| {
                item.map_err(|_| std::io::Error::other("WebDAV response stream failed"))
            })),
            DavResponseBody::MultiStatus(stream) => Body::from_stream(stream.map(|item| {
                item.map_err(|_| std::io::Error::other("WebDAV response stream failed"))
            })),
        };
    builder.body(body).unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "invalid WebDAV response").into_response()
    })
}

#[must_use]
/// Maps a protocol error into an Axum response.
pub fn protocol_error_response(error: &DavProtocolError) -> Response {
    into_response(crate::protocol_error_response(error))
}

/// Copies headers into the transport-neutral `http` map.
///
/// # Errors
///
/// Returns a protocol error when a header cannot be represented.
pub fn convert_header_map(source: &HeaderMap) -> Result<HeaderMap, DavProtocolError> {
    let mut headers = HeaderMap::with_capacity(source.len());
    for (name, value) in source {
        let name = HeaderName::from_bytes(name.as_str().as_bytes())
            .map_err(|_| DavProtocolError::bad_request("Invalid request header"))?;
        let value = HeaderValue::from_bytes(value.as_bytes())
            .map_err(|_| DavProtocolError::bad_request("Invalid request header"))?;
        headers.append(name, value);
    }
    Ok(headers)
}

/// Collects a request body according to the selected bounded body policy.
///
/// # Errors
///
/// Returns a body error when reading fails, the limit is exceeded, or a body is forbidden.
pub async fn prepare_request_body(
    policy: DavBodyPolicy,
    request: Request,
) -> Result<DavPreparedBody, DavBodyError> {
    match policy {
        DavBodyPolicy::Empty => {
            let body = to_bytes(request.into_body(), 1)
                .await
                .map_err(|_| DavBodyError::ReadFailed)?;
            if body.is_empty() {
                Ok(DavPreparedBody::None)
            } else {
                Err(DavBodyError::BodyNotAllowed)
            }
        }
        DavBodyPolicy::BoundedXml { maximum } => to_bytes(request.into_body(), maximum)
            .await
            .map(|b| DavPreparedBody::Xml(b.to_vec()))
            .map_err(|error| {
                if error.to_string().contains("length limit") {
                    DavBodyError::BodyTooLarge
                } else {
                    DavBodyError::ReadFailed
                }
            }),
        DavBodyPolicy::Bounded { maximum } => to_bytes(request.into_body(), maximum)
            .await
            .map(|b| DavPreparedBody::Bytes(b.to_vec()))
            .map_err(|error| {
                if error.to_string().contains("length limit") {
                    DavBodyError::BodyTooLarge
                } else {
                    DavBodyError::ReadFailed
                }
            }),
        DavBodyPolicy::Stream | DavBodyPolicy::Unused => Ok(DavPreparedBody::None),
    }
}

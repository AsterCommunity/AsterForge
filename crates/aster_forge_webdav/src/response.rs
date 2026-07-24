//! Transport-neutral WebDAV response model and download response planning.

use std::time::SystemTime;

use aster_forge_utils::http_range::{HttpByteRange, parse_single_byte_range};
use aster_forge_utils::http_validators::format_http_date;
use bytes::Bytes;
use http::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, LAST_MODIFIED, RANGE,
};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::{DavContentStream, DavPrecondition, DavProtocolError};

/// Methods advertised by the product-neutral DAV protocol engine.
pub const DAV_ALLOW_HEADER: &str = "OPTIONS, GET, HEAD, PUT, DELETE, MKCOL, COPY, MOVE, PROPFIND, PROPPATCH, LOCK, UNLOCK, REPORT, VERSION-CONTROL";

/// Failure while enforcing a request body policy in the transport adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavBodyError {
    #[error("failed to read WebDAV request body")]
    ReadFailed,
    #[error("WebDAV XML body is too large")]
    XmlTooLarge,
    #[error("WebDAV method does not accept a request body")]
    BodyNotAllowed,
}

/// Whether a successful GET/HEAD response needs content from the product backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavDownloadBody {
    /// The response has no body because it is a HEAD, 304, or 416 response.
    Empty,
    /// Stream the complete representation.
    Full,
    /// Stream only the selected representation range.
    Range(HttpByteRange),
}

/// A complete response shell plus the storage read selected by the protocol layer.
pub struct DavDownloadPlan {
    pub response: DavResponse,
    pub body: DavDownloadBody,
}

/// Failure while building a product-neutral download response plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavDownloadPlanError {
    #[error(transparent)]
    Protocol(#[from] DavProtocolError),
    #[error("invalid WebDAV download representation metadata")]
    InvalidRepresentation,
}

/// WebDAV response body before transport adaptation.
pub enum DavResponseBody {
    Empty,
    Bytes(Bytes),
    Stream(DavContentStream),
}

/// Status, headers, and body produced by the protocol layer.
pub struct DavResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: DavResponseBody,
}

impl DavResponse {
    /// Creates an empty response.
    #[must_use]
    pub fn empty(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: DavResponseBody::Empty,
        }
    }

    /// Creates a byte response.
    #[must_use]
    pub fn bytes(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: DavResponseBody::Bytes(body.into()),
        }
    }
}

/// Builds the response to a DAV `OPTIONS` request.
#[must_use]
pub fn options_response() -> DavResponse {
    let mut response = DavResponse::empty(StatusCode::OK);
    response
        .headers
        .insert(ALLOW, HeaderValue::from_static(DAV_ALLOW_HEADER));
    response
        .headers
        .insert("DAV", HeaderValue::from_static("1, 2, version-control"));
    response
        .headers
        .insert("MS-Author-Via", HeaderValue::from_static("DAV"));
    response
}

/// Builds the response for an unsupported HTTP/WebDAV method.
#[must_use]
pub fn method_not_allowed_response() -> DavResponse {
    let mut response = DavResponse::empty(StatusCode::METHOD_NOT_ALLOWED);
    response
        .headers
        .insert(ALLOW, HeaderValue::from_static(DAV_ALLOW_HEADER));
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Builds the protocol response for a transport body-policy failure.
#[must_use]
pub fn body_error_response(error: DavBodyError) -> DavResponse {
    let (status, body) = match error {
        DavBodyError::ReadFailed => (StatusCode::BAD_REQUEST, Some("Failed to read request body")),
        DavBodyError::XmlTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Some("WebDAV XML body too large"),
        ),
        DavBodyError::BodyNotAllowed => (StatusCode::UNSUPPORTED_MEDIA_TYPE, None),
    };
    let mut response = match body {
        Some(body) => DavResponse::bytes(status, body),
        None => DavResponse::empty(status),
    };
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if body.is_some() {
        response.headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
    }
    response
}

/// Builds the GET/HEAD response and storage-read plan after product metadata has been resolved.
pub fn plan_download_response(
    headers: &HeaderMap,
    head_only: bool,
    content_length: u64,
    content_type: &str,
    etag: Option<&str>,
    last_modified: SystemTime,
) -> Result<DavDownloadPlan, DavDownloadPlanError> {
    match crate::evaluate_http_download_preconditions(headers, etag, Some(last_modified))? {
        DavPrecondition::Proceed => {}
        DavPrecondition::NotModified => {
            let mut response = DavResponse::empty(StatusCode::NOT_MODIFIED);
            insert_validators(&mut response.headers, etag, last_modified)?;
            return Ok(DavDownloadPlan {
                response,
                body: DavDownloadBody::Empty,
            });
        }
    }

    let range = if head_only {
        None
    } else if let Some(value) = headers.get(RANGE) {
        let Ok(raw) = value.to_str() else {
            return Ok(range_not_satisfiable_plan(content_length));
        };
        match parse_single_byte_range(raw, content_length) {
            Ok(range) => Some(range),
            Err(_) => return Ok(range_not_satisfiable_plan(content_length)),
        }
    } else {
        None
    };

    let (status, response_length, body) = match range {
        Some(range) => (
            StatusCode::PARTIAL_CONTENT,
            range.length(),
            DavDownloadBody::Range(range),
        ),
        None if head_only => (StatusCode::OK, content_length, DavDownloadBody::Empty),
        None => (StatusCode::OK, content_length, DavDownloadBody::Full),
    };
    let mut response = DavResponse::empty(status);
    response
        .headers
        .insert(CONTENT_LENGTH, header_value(&response_length.to_string())?);
    response
        .headers
        .insert(CONTENT_TYPE, header_value(content_type)?);
    response
        .headers
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
        .headers
        .insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));
    if let Some(range) = range {
        response
            .headers
            .insert(CONTENT_RANGE, header_value(&range.content_range_header())?);
    }
    insert_validators(&mut response.headers, etag, last_modified)?;

    Ok(DavDownloadPlan { response, body })
}

/// Builds the response required when a byte range cannot be served.
#[must_use]
pub fn range_not_satisfiable_response(content_length: u64) -> DavResponse {
    let mut response = DavResponse::empty(StatusCode::RANGE_NOT_SATISFIABLE);
    response.headers.insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{content_length}"))
            .unwrap_or_else(|_| HeaderValue::from_static("bytes */0")),
    );
    response
        .headers
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn range_not_satisfiable_plan(content_length: u64) -> DavDownloadPlan {
    DavDownloadPlan {
        response: range_not_satisfiable_response(content_length),
        body: DavDownloadBody::Empty,
    }
}

fn insert_validators(
    headers: &mut HeaderMap,
    etag: Option<&str>,
    last_modified: SystemTime,
) -> Result<(), DavDownloadPlanError> {
    headers.insert(
        LAST_MODIFIED,
        header_value(&format_http_date(last_modified))?,
    );
    if let Some(etag) = etag {
        headers.insert(ETAG, header_value(&format!("\"{etag}\""))?);
    }
    Ok(())
}

fn header_value(value: &str) -> Result<HeaderValue, DavDownloadPlanError> {
    HeaderValue::from_str(value).map_err(|_| DavDownloadPlanError::InvalidRepresentation)
}

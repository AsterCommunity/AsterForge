//! Optional Actix transport adapter for the `WebDAV` protocol model.

use actix_web::http::{StatusCode as ActixStatusCode, header as actix_header};
use actix_web::{HttpRequest, HttpResponse};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use http::{HeaderMap, HeaderName, HeaderValue, Uri};

use crate::protocol::DavProtocolError;
use crate::{
    DavBodyError, DavBodyPolicy, DavCapabilityContext, DavCapabilityProvider,
    DavCapabilitySnapshot, DavCapabilityTarget, DavConditionalPlan, DavConditionalResource,
    DavFileSystem, DavIfEvaluationError, DavLockSystem, DavMethod, DavPath, DavRequestHead,
    DavRequestOrigin, DavRequestTarget, DavResponse, DavResponseBody, IfHeader,
};

/// Request body prepared according to the selected `WebDAV` method contract.
pub enum DavPreparedBody {
    None,
    Xml(Vec<u8>),
    Bytes(Vec<u8>),
}

impl DavPreparedBody {
    /// Returns the collected XML bytes, or an empty slice for bodyless methods.
    #[must_use]
    pub fn xml(&self) -> &[u8] {
        match self {
            Self::None | Self::Bytes(_) => &[],
            Self::Xml(body) => body,
        }
    }

    /// Returns collected opaque bytes, or an empty slice for other body policies.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Bytes(body) => body,
            Self::None | Self::Xml(_) => &[],
        }
    }
}

/// Parses an Actix request into the transport-neutral request head.
///
/// # Errors
///
/// Returns [`DavProtocolError`] when the URI, mount path, origin, or headers are invalid.
pub fn request_head(
    request: &HttpRequest,
    mount_path: &str,
) -> Result<Option<DavRequestHead>, DavProtocolError> {
    let request_target = request_target(request, mount_path)?;
    let Some(method) = DavMethod::from_name(request.method().as_str()) else {
        return Ok(None);
    };
    let headers = convert_header_map(request.headers())?;
    DavRequestHead::parse_known_method(method, &request_target, &headers).map(Some)
}

/// Parses the request target before resolving whether the HTTP method is known to Forge.
///
/// # Errors
///
/// Returns [`DavProtocolError`] when the URI, origin, or mount-relative target is invalid.
pub fn request_target<'a>(
    request: &HttpRequest,
    mount_path: &'a str,
) -> Result<DavRequestTarget<'a>, DavProtocolError> {
    let uri: Uri = request
        .uri()
        .to_string()
        .parse()
        .map_err(|_| DavProtocolError::bad_request("Invalid request URI"))?;
    let connection = request.connection_info();
    let origin = DavRequestOrigin {
        scheme: connection.scheme().to_string(),
        host: connection.host().to_string(),
    };
    DavRequestHead::parse_target(&uri, mount_path, &origin)
}

/// Resolves the product declaration and maps the validated capability snapshot to Actix.
///
/// # Errors
///
/// Returns an error response when provider lookup or capability validation fails.
pub async fn capability_snapshot<Provider: DavCapabilityProvider>(
    provider: &Provider,
    target: &DavCapabilityTarget,
    context: &DavCapabilityContext,
) -> Result<DavCapabilitySnapshot, HttpResponse> {
    crate::plan_capabilities_with_provider(provider, target, context)
        .await
        .map_err(|error| into_response(crate::capability_evaluation_error_response(&error)))
}

/// Applies the resource-aware dispatch gate to an Actix request method.
///
/// # Errors
///
/// Returns a 405 response when the snapshot does not dispatch the request method.
pub fn gate_request_method(
    request: &HttpRequest,
    snapshot: &DavCapabilitySnapshot,
) -> Result<DavMethod, HttpResponse> {
    crate::gate_method(DavMethod::from_name(request.method().as_str()), snapshot)
        .map_err(|_| into_response(crate::method_not_allowed_response(snapshot)))
}

/// Converts a transport-neutral response into an Actix response.
pub fn into_response(response: DavResponse) -> HttpResponse {
    let status = ActixStatusCode::from_u16(response.status.as_u16())
        .unwrap_or(ActixStatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = HttpResponse::build(status);
    for (name, value) in &response.headers {
        let name = actix_header::HeaderName::from_bytes(name.as_str().as_bytes());
        let value = actix_header::HeaderValue::from_bytes(value.as_bytes());
        if let (Ok(name), Ok(value)) = (name, value) {
            builder.insert_header((name, value));
        }
    }
    match response.body {
        DavResponseBody::Empty => builder.finish(),
        DavResponseBody::Bytes(body) => builder.body(body),
        DavResponseBody::Stream(stream) => streaming_response(&mut builder, stream),
        DavResponseBody::MultiStatus(stream) => streaming_response(&mut builder, stream),
    }
}

fn streaming_response<S, E>(builder: &mut actix_web::HttpResponseBuilder, stream: S) -> HttpResponse
where
    S: Stream<Item = Result<Bytes, E>> + 'static,
    E: 'static,
{
    let stream = stream.map(|item| {
        item.map_err(|_| {
            actix_web::error::ErrorInternalServerError("WebDAV response stream failed")
        })
    });
    builder.streaming(stream)
}

/// Maps a transport-neutral protocol error into its Actix response.
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "The owned error signature composes directly with Result::map_err at the Actix boundary."
)]
pub fn protocol_error_response(error: DavProtocolError) -> HttpResponse {
    into_response(crate::protocol_error_response(&error))
}

/// Copies Actix headers into the transport-neutral map and maps malformed input to a response.
///
/// # Errors
///
/// Returns an error response when an Actix header cannot be represented by `http` 1.x.
pub fn converted_headers(source: &actix_header::HeaderMap) -> Result<HeaderMap, HttpResponse> {
    convert_header_map(source).map_err(protocol_error_response)
}

/// Resolves and enforces a parsed `WebDAV` `If` header through the canonical backend ports.
///
/// # Errors
///
/// Returns an error response when DAV `If` evaluation or backend access fails.
pub async fn enforce_if_header_with_backends(
    if_header: Option<&IfHeader>,
    filesystem: &dyn DavFileSystem,
    lock_system: &dyn DavLockSystem,
    request_path: &DavPath,
    prefix: &str,
    request_scheme: &str,
    request_host: &str,
) -> Result<(), HttpResponse> {
    match crate::enforce_if_header_with_backends(
        if_header,
        filesystem,
        lock_system,
        request_path,
        prefix,
        request_scheme,
        request_host,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(DavIfEvaluationError::Protocol(error)) => Err(protocol_error_response(error)),
        Err(DavIfEvaluationError::Backend(error)) => {
            Err(into_response(crate::backend_error_response(&error)))
        }
    }
}

/// Enforces resource lock submission and maps the protocol response to Actix.
///
/// # Errors
///
/// Returns an error response when a conflicting lock exists or lock lookup fails.
pub async fn enforce_unlocked(
    lock_system: &dyn DavLockSystem,
    path: &DavPath,
    deep: bool,
    prefix: &str,
    if_header: Option<&IfHeader>,
    request_scheme: &str,
    request_host: &str,
) -> Result<(), HttpResponse> {
    crate::enforce_unlocked(
        lock_system,
        path,
        deep,
        prefix,
        if_header,
        request_scheme,
        request_host,
    )
    .await
    .map_err(into_response)
}

/// Enforces lock submission for the canonical parent and maps the response to Actix.
///
/// # Errors
///
/// Returns an error response when the parent is locked or lock lookup fails.
pub async fn enforce_parent_unlocked(
    lock_system: &dyn DavLockSystem,
    path: &DavPath,
    prefix: &str,
    if_header: Option<&IfHeader>,
    request_scheme: &str,
    request_host: &str,
) -> Result<(), HttpResponse> {
    crate::enforce_parent_unlocked(
        lock_system,
        path,
        prefix,
        if_header,
        request_scheme,
        request_host,
    )
    .await
    .map_err(into_response)
}

/// Converts Actix headers and runs the method-aware conditional request planner.
///
/// # Errors
///
/// Returns an error response when header conversion or conditional planning fails.
pub fn plan_http_conditionals(
    headers: &actix_header::HeaderMap,
    method: DavMethod,
    resource: DavConditionalResource<'_>,
) -> Result<DavConditionalPlan, HttpResponse> {
    let headers = converted_headers(headers)?;
    crate::plan_http_conditionals(method, &headers, resource)
        .map_err(|error| into_response(crate::conditional_plan_error_response(&error)))
}

/// Copies Actix header types into the transport-neutral `http` 1.x map.
///
/// # Errors
///
/// Returns [`DavProtocolError`] when a header name or value cannot be converted.
pub fn convert_header_map(source: &actix_header::HeaderMap) -> Result<HeaderMap, DavProtocolError> {
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

/// Rejects the first non-empty request body chunk without buffering the remaining payload.
///
/// # Errors
///
/// Returns [`DavBodyError`] when payload reading fails or the body is not empty.
pub async fn ensure_empty_body(payload: &mut actix_web::web::Payload) -> Result<(), DavBodyError> {
    while let Some(chunk) = payload.next().await {
        let chunk = chunk.map_err(|_| DavBodyError::ReadFailed)?;
        if !chunk.is_empty() {
            return Err(DavBodyError::BodyNotAllowed);
        }
    }
    Ok(())
}

/// Collects a bounded request body for parsing by the protocol or product layer.
///
/// # Errors
///
/// Returns [`DavBodyError`] when reading fails or the configured maximum is exceeded.
pub async fn collect_bounded_body(
    payload: &mut actix_web::web::Payload,
    maximum: usize,
) -> Result<Vec<u8>, DavBodyError> {
    let mut body = Vec::with_capacity(maximum.min(4096));
    while let Some(chunk) = payload.next().await {
        let chunk = chunk.map_err(|_| DavBodyError::ReadFailed)?;
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(DavBodyError::BodyTooLarge)?;
        if next_len > maximum {
            return Err(DavBodyError::BodyTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Applies an already planned body policy while leaving streaming bodies untouched.
///
/// # Errors
///
/// Returns [`DavBodyError`] when the planned body policy cannot be satisfied.
pub async fn prepare_request_body(
    policy: DavBodyPolicy,
    payload: &mut actix_web::web::Payload,
) -> Result<DavPreparedBody, DavBodyError> {
    match policy {
        DavBodyPolicy::Empty => ensure_empty_body(payload)
            .await
            .map(|()| DavPreparedBody::None),
        DavBodyPolicy::BoundedXml { maximum } => collect_bounded_body(payload, maximum)
            .await
            .map(DavPreparedBody::Xml),
        DavBodyPolicy::Bounded { maximum } => collect_bounded_body(payload, maximum)
            .await
            .map(DavPreparedBody::Bytes),
        DavBodyPolicy::Stream | DavBodyPolicy::Unused => Ok(DavPreparedBody::None),
    }
}

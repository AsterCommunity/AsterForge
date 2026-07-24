//! Optional Actix transport adapter for the WebDAV protocol model.

use actix_web::http::{StatusCode as ActixStatusCode, header as actix_header};
use actix_web::{HttpRequest, HttpResponse};
use futures::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, Uri};

use crate::protocol::DavProtocolError;
use crate::{DavMethod, DavRequestHead, DavRequestOrigin, DavResponse, DavResponseBody};

/// Parses an Actix request into the transport-neutral request head.
pub fn request_head(
    request: &HttpRequest,
    mount_path: &str,
) -> Result<Option<DavRequestHead>, DavProtocolError> {
    let Some(method) = DavMethod::from_name(request.method().as_str()) else {
        return Ok(None);
    };
    let uri: Uri = request
        .uri()
        .to_string()
        .parse()
        .map_err(|_| DavProtocolError::bad_request("Invalid request URI"))?;
    let headers = convert_header_map(request.headers())?;
    let connection = request.connection_info();
    let origin = DavRequestOrigin {
        scheme: connection.scheme().to_string(),
        host: connection.host().to_string(),
    };
    DavRequestHead::parse(method, &uri, &headers, mount_path, &origin).map(Some)
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
        DavResponseBody::Stream(stream) => {
            let stream = stream.map(|item| {
                item.map_err(|error| actix_web::error::ErrorInternalServerError(error.to_string()))
            });
            builder.streaming(stream)
        }
    }
}

/// Copies Actix header types into the transport-neutral `http` 1.x map.
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

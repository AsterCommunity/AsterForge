//! Axum request-id adapter.
use axum::{extract::Request, middleware::Next, response::Response};
use http::header::{HeaderName, HeaderValue};
use tracing::Instrument;

#[derive(Clone, Debug)]
pub struct RequestId(pub String);

pub async fn request_id(request: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let mut request = request;
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    let span = tracing::info_span!("request", request_id = %request_id, method = %method, path = %path, user_id = tracing::field::Empty);
    let mut response = next.run(request).instrument(span).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    response
}

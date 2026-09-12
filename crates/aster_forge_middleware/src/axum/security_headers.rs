//! Axum security-header adapter.
use axum::{extract::Request, middleware::Next, response::Response};
use http::header::{HeaderName, HeaderValue};

pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("SAMEORIGIN"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}

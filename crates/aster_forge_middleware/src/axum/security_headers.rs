//! Axum security-header adapter.
pub use crate::shared::security_headers::{
    REFERRER_POLICY_VALUE, X_CONTENT_TYPE_OPTIONS_VALUE, X_FRAME_OPTIONS_VALUE,
};
use axum::{extract::Request, middleware::Next, response::Response};
use http::header::{HeaderName, HeaderValue};

/// Supplies missing defaults, preserving explicit product response policies.
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        ("x-frame-options", X_FRAME_OPTIONS_VALUE),
        ("referrer-policy", REFERRER_POLICY_VALUE),
        ("x-content-type-options", X_CONTENT_TYPE_OPTIONS_VALUE),
    ] {
        response
            .headers_mut()
            .entry(HeaderName::from_static(name))
            .or_insert(HeaderValue::from_static(value));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware, response::IntoResponse, routing::get};
    use http::{Request as HttpRequest, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn adds_defaults_and_preserves_explicit_product_headers_on_errors() {
        let router = Router::new()
            .route("/", get(|| async { "called" }))
            .route(
                "/strict",
                get(|| async {
                    let mut response = StatusCode::BAD_REQUEST.into_response();
                    response
                        .headers_mut()
                        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
                    response
                        .headers_mut()
                        .insert("x-frame-options", HeaderValue::from_static("DENY"));
                    response
                }),
            )
            .layer(middleware::from_fn(security_headers));
        for (path, referrer, frame, status) in [
            (
                "/",
                REFERRER_POLICY_VALUE,
                X_FRAME_OPTIONS_VALUE,
                StatusCode::OK,
            ),
            ("/strict", "no-referrer", "DENY", StatusCode::BAD_REQUEST),
            (
                "/missing",
                REFERRER_POLICY_VALUE,
                X_FRAME_OPTIONS_VALUE,
                StatusCode::NOT_FOUND,
            ),
        ] {
            let response = router
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["referrer-policy"], referrer);
            assert_eq!(response.headers()["x-frame-options"], frame);
            assert_eq!(
                response.headers()["x-content-type-options"],
                X_CONTENT_TYPE_OPTIONS_VALUE
            );
        }
    }
}

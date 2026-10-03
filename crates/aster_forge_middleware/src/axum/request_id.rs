//! Axum request-id adapter.
use axum::{extract::Request, middleware::Next, response::Response};
use http::header::{HeaderName, HeaderValue};
use tracing::Instrument;

#[derive(Clone, Debug)]
/// Server-generated UUID stored in request extensions.
pub struct RequestId(pub String);

/// Generates a new UUID v4 per request, ignoring inbound IDs, and returns it in the response.
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Extension, Router,
        body::{Body, to_bytes},
        middleware,
        response::IntoResponse,
        routing::get,
    };
    use http::{Request as HttpRequest, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn generates_stores_and_returns_unique_ids_for_success_and_failure() {
        let router = Router::new()
            .route(
                "/",
                get(|Extension(id): Extension<RequestId>| async move {
                    let mut response = (StatusCode::BAD_REQUEST, id.0).into_response();
                    response
                        .headers_mut()
                        .insert("x-request-id", HeaderValue::from_static("downstream-id"));
                    response
                }),
            )
            .layer(middleware::from_fn(request_id));
        let mut ids = Vec::new();
        for inbound in [
            None,
            Some("untrusted-id"),
            Some("11111111-1111-4111-8111-111111111111"),
        ] {
            let mut request = HttpRequest::builder();
            if let Some(inbound) = inbound {
                request = request.header("x-request-id", inbound);
            }
            let response = router
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let id = response.headers()["x-request-id"]
                .to_str()
                .unwrap()
                .to_string();
            assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);
            assert_ne!(Some(id.as_str()), inbound);
            assert_eq!(
                to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
                id.as_bytes()
            );
            ids.push(id);
        }
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
    }
}

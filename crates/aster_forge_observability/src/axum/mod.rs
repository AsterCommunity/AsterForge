//! Injectable Axum exposition; authentication and paths belong to the product.

#[cfg(feature = "prometheus")]
pub fn prometheus_router(provider: aster_forge_metrics::MetricsProvider) -> axum::Router {
    axum::Router::new()
        .route("/metrics", axum::routing::get(prometheus_metrics))
        .layer(axum::Extension(provider))
}

#[cfg(feature = "prometheus")]
async fn prometheus_metrics(
    axum::Extension(provider): axum::Extension<aster_forge_metrics::MetricsProvider>,
) -> axum::response::Response {
    use axum::{
        http::{StatusCode, header},
        response::IntoResponse,
    };
    match provider
        .export_prometheus(std::time::Duration::from_secs(5))
        .await
    {
        Ok(body) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8",
                ),
                (header::CACHE_CONTROL, "no-store"),
            ],
            body,
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CACHE_CONTROL, "no-store")],
            "metrics export is unavailable",
        )
            .into_response(),
    }
}

#[cfg(all(test, feature = "prometheus"))]
mod tests {
    use super::*;
    use aster_forge_metrics::{MetricDescriptor, MetricsProvider, ProductCounter};
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn injected_axum_endpoint_preserves_format_status_and_provider_isolation() {
        let provider = MetricsProvider::builder()
            .with_prometheus()
            .unwrap()
            .build()
            .unwrap();
        let handle = provider
            .register_batch(&[
                MetricDescriptor::counter("test", "requests", "Requests.", &[])
                    .with_unit("{request}"),
            ])
            .unwrap()
            .remove(0);
        ProductCounter::from_handle(handle).unwrap().inc(&[], 3);
        let router = prometheus_router(provider.clone());
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(
            String::from_utf8(body.to_vec())
                .unwrap()
                .contains("test_requests_total 3")
        );
        provider
            .shutdown(std::time::Duration::from_secs(1))
            .await
            .unwrap();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

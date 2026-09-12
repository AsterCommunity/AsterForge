//! Axum observability endpoint adapter.

use axum::Router;

/// Adds `/metrics` when Prometheus support is enabled; otherwise returns the router unchanged.
pub fn configure_prometheus_route<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    #[cfg(feature = "prometheus")]
    {
        router.route("/metrics", axum::routing::get(prometheus_metrics))
    }
    #[cfg(not(feature = "prometheus"))]
    {
        router
    }
}

#[cfg(feature = "prometheus")]
async fn prometheus_metrics() -> impl axum::response::IntoResponse {
    use axum::{http::StatusCode, response::IntoResponse};
    if !aster_forge_metrics::prometheus::is_initialized() {
        tracing::debug!("metrics probe failed because metrics are not initialized");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics registry is not initialized",
        )
            .into_response();
    }
    match aster_forge_metrics::prometheus::export_metrics() {
        Ok(body) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(error) => (StatusCode::SERVICE_UNAVAILABLE, error.clone()).into_response(),
    }
}

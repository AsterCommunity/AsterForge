//! Axum Prometheus endpoint adapter.

use axum::{http::StatusCode, response::IntoResponse};

/// Exposes the shared Prometheus registry as an Axum handler.
pub fn prometheus_metrics() -> impl IntoResponse {
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
        Err(error) => {
            tracing::debug!(error = %error, "metrics probe export failed");
            (StatusCode::SERVICE_UNAVAILABLE, error.clone()).into_response()
        }
    }
}

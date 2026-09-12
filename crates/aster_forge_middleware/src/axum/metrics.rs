//! Axum request-metrics adapter.
use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use std::time::Instant;

pub async fn metrics(request: Request, next: Next) -> Response {
    use aster_forge_metrics::{NoopMetrics, SharedMetricsRecorder};
    let recorder = request
        .extensions()
        .get::<SharedMetricsRecorder>()
        .cloned()
        .unwrap_or_else(NoopMetrics::arc);
    if !recorder.enabled() {
        return next.run(request).await;
    }
    let started = Instant::now();
    let method = request.method().to_string();
    let route = request.extensions().get::<MatchedPath>().map_or_else(
        || unmatched_route(request.uri().path()).to_string(),
        |path| path.as_str().to_string(),
    );
    let response = next.run(request).await;
    recorder.record_http_request(
        &method,
        &route,
        response.status().as_u16(),
        started.elapsed().as_secs_f64(),
    );
    response
}

fn unmatched_route(path: &str) -> &'static str {
    if path.starts_with("/api/") {
        "unmatched_api"
    } else if path.starts_with("/health") {
        "unmatched_health"
    } else {
        "unmatched"
    }
}

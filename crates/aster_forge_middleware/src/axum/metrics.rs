//! Axum request-metrics adapter.
use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use std::time::Instant;

/// Records responses using an optional recorder in request extensions and low-cardinality routes.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingMetrics;
    use axum::{Extension, Router, body::Body, middleware, routing::get};
    use http::{Request as HttpRequest, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn records_success_failure_and_unmatched_paths_with_bounded_labels() {
        let recorder = RecordingMetrics::enabled();
        let router = Router::new()
            .route("/api/profiles/{id}", get(|| async { StatusCode::CREATED }))
            .route(
                "/api/fails",
                get(|| async { Err::<(), _>(StatusCode::BAD_REQUEST) }),
            )
            .layer(middleware::from_fn(metrics))
            .layer(Extension(recorder.shared()));
        for (path, status, route) in [
            ("/api/profiles/42", 201, "/api/profiles/{id}"),
            ("/api/profiles/99", 201, "/api/profiles/{id}"),
            ("/api/fails", 400, "/api/fails"),
            ("/api/missing-secret-id", 404, "unmatched_api"),
            ("/health/secret-id", 404, "unmatched_health"),
            ("/missing-secret-id", 404, "unmatched"),
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
            assert_eq!(response.status().as_u16(), status);
            let record = recorder.records().pop().unwrap();
            assert_eq!(
                (record.method.as_str(), record.route.as_str(), record.status),
                ("GET", route, status)
            );
            assert!(record.duration_seconds >= 0.0);
        }
        assert_eq!(recorder.records().len(), 6);
    }

    #[tokio::test]
    async fn disabled_and_missing_recorders_leave_responses_unchanged() {
        let recorder = RecordingMetrics::disabled();
        let router = Router::new()
            .route("/", get(|| async { StatusCode::ACCEPTED }))
            .layer(middleware::from_fn(metrics));
        for router in [router.clone(), router.layer(Extension(recorder.shared()))] {
            assert_eq!(
                router
                    .oneshot(HttpRequest::new(Body::empty()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::ACCEPTED
            );
        }
        assert!(recorder.records().is_empty());
    }
}

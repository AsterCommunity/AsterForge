//! Axum request-metrics adapter.
use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

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
    if matches!(
        request.uri().path(),
        "/health"
            | "/health/live"
            | "/health/ready"
            | "/health/metrics"
            | "/metrics"
            | "/ready"
            | "/live"
    ) {
        return next.run(request).await;
    }
    let method = request.method().to_string();
    let is_head = request.method() == http::Method::HEAD;
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(String::new, |path| path.as_str().to_string());
    let mut guard = aster_forge_metrics::HttpRequestGuard::new(
        recorder,
        &method,
        request.uri().scheme_str().unwrap_or("http"),
    );
    let response = next.run(request).await;
    guard.response(route, response.status().as_u16());
    let (parts, body) = response.into_parts();
    if is_head || axum::body::HttpBody::is_end_stream(&body) {
        guard.finish();
    }
    Response::from_parts(
        parts,
        axum::body::Body::new(ObservedBody {
            inner: body,
            guard: Some(guard),
        }),
    )
}

struct ObservedBody {
    inner: axum::body::Body,
    guard: Option<aster_forge_metrics::HttpRequestGuard>,
}
impl axum::body::HttpBody for ObservedBody {
    type Data = axum::body::Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let result = Pin::new(&mut self.inner).poll_frame(cx);
        if matches!(&result, Poll::Ready(Some(Ok(_))))
            && self.inner.is_end_stream()
            && let Some(mut guard) = self.guard.take()
        {
            guard.finish();
        }
        if matches!(&result, Poll::Ready(None | Some(Err(_))))
            && let Some(mut guard) = self.guard.take()
        {
            if matches!(&result, Poll::Ready(Some(Err(_)))) {
                guard.fail();
            } else {
                guard.finish();
            }
        }
        result
    }
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn head_response_completes_without_polling_or_canceling_application_body() {
        let recorder = RecordingMetrics::enabled();
        let router = Router::new()
            .route(
                "/head",
                axum::routing::head(|| async {
                    Body::from_stream(futures::stream::pending::<
                        Result<axum::body::Bytes, std::io::Error>,
                    >())
                }),
            )
            .layer(middleware::from_fn(metrics))
            .layer(Extension(recorder.shared()));
        let response = router
            .oneshot(
                HttpRequest::builder()
                    .method("HEAD")
                    .uri("/head")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        drop(response);
        assert_eq!(recorder.active_requests(), 0);
        assert!(
            recorder.stream_failures().is_empty(),
            "HEAD is completed even when its body is discarded"
        );
        let records = recorder.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            (
                records[0].method.as_str(),
                records[0].route.as_str(),
                records[0].status
            ),
            ("HEAD", "/head", 200)
        );
    }

    #[tokio::test]
    async fn response_body_retains_observation_until_eof_or_drop() {
        let recorder = RecordingMetrics::enabled();
        let router = Router::new()
            .route(
                "/stream/{id}",
                get(|| async {
                    Body::from_stream(futures::stream::pending::<
                        Result<axum::body::Bytes, std::io::Error>,
                    >())
                }),
            )
            .route("/complete", get(|| async { "done" }))
            .layer(middleware::from_fn(metrics))
            .layer(Extension(recorder.shared()));
        let response = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/stream/42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(recorder.records().is_empty());
        assert_eq!(recorder.active_requests(), 1);
        drop(response);
        assert_eq!(recorder.records().len(), 1);
        assert_eq!(recorder.stream_failures(), vec![true]);
        assert_eq!(recorder.active_requests(), 0);
        assert_eq!(recorder.records()[0].route, "/stream/{id}");
        let response = router
            .oneshot(
                HttpRequest::builder()
                    .uri("/complete")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(recorder.records().len(), 1);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 32)
                .await
                .unwrap(),
            "done"
        );
        assert_eq!(recorder.records().len(), 2);
        assert_eq!(recorder.stream_failures(), vec![true]);
        assert_eq!(recorder.active_requests(), 0);
    }
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
            ("/api/missing-secret-id", 404, ""),
            ("/health/secret-id", 404, ""),
            ("/missing-secret-id", 404, ""),
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
        assert_eq!(
            recorder.records(),
            [] as [crate::test_support::HttpMetricRecord; 0]
        );
    }
}

//! HTTP request metrics middleware for Actix Web services.
//!
//! The middleware reads a shared [`aster_forge_metrics::SharedMetricsRecorder`] from Actix app
//! data, records one HTTP request metric per successful or failed downstream service call, and
//! groups unmatched routes into stable low-cardinality labels.

use std::rc::Rc;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

use actix_web::{
    Error,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    web,
};
use futures::future::{LocalBoxFuture, Ready, ok};

use aster_forge_metrics::{MetricsRecorder, SharedMetricsRecorder};

/// Actix middleware that records request duration and status into the shared metrics recorder.
pub struct MetricsMiddleware;

impl<S, B> Transform<S, ServiceRequest> for MetricsMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<ObservedBody<B>>;
    type Error = Error;
    type InitError = ();
    type Transform = MetricsService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(MetricsService {
            service: Rc::new(service),
        })
    }
}

/// Service wrapper installed by [`MetricsMiddleware`].
pub struct MetricsService<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for MetricsService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<ObservedBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let svc = self.service.clone();
        let metrics = request_metrics(&req);

        if !metrics.enabled() {
            return Box::pin(async move {
                svc.call(req).await.map(|response| {
                    response.map_body(|_, body| ObservedBody {
                        inner: Box::pin(body),
                        guard: None,
                        remaining: None,
                    })
                })
            });
        }
        if matches!(
            req.path(),
            "/health"
                | "/health/live"
                | "/health/ready"
                | "/health/metrics"
                | "/metrics"
                | "/ready"
                | "/live"
        ) {
            return Box::pin(async move {
                svc.call(req).await.map(|response| {
                    response.map_body(|_, body| ObservedBody {
                        inner: Box::pin(body),
                        guard: None,
                        remaining: None,
                    })
                })
            });
        }

        let method = req.method().clone();
        let route = route_label(&req);
        let mut active = aster_forge_metrics::HttpRequestGuard::new(
            metrics.clone(),
            method.as_str(),
            if req.app_config().secure() {
                "https"
            } else {
                "http"
            },
        );

        Box::pin(async move {
            match svc.call(req).await {
                Ok(response) => {
                    let matched = response.request().match_pattern().unwrap_or_default();
                    active.response(matched, response.status().as_u16());
                    Ok(response.map_body(move |_, body| {
                        if matches!(
                            body.size(),
                            actix_web::body::BodySize::None | actix_web::body::BodySize::Sized(0)
                        ) {
                            active.finish();
                        }
                        let remaining = match body.size() {
                            actix_web::body::BodySize::Sized(length) => Some(length),
                            _ => None,
                        };
                        ObservedBody {
                            inner: Box::pin(body),
                            guard: Some(active),
                            remaining,
                        }
                    }))
                }
                Err(error) => {
                    active.response(route, error.as_response_error().status_code().as_u16());
                    active.finish();
                    Err(error)
                }
            }
        })
    }
}

/// Response body keeps the request observation alive through EOF, error or drop.
pub struct ObservedBody<B> {
    inner: Pin<Box<B>>,
    guard: Option<aster_forge_metrics::HttpRequestGuard>,
    remaining: Option<u64>,
}
impl<B: actix_web::body::MessageBody> actix_web::body::MessageBody for ObservedBody<B> {
    type Error = B::Error;
    fn size(&self) -> actix_web::body::BodySize {
        self.inner.size()
    }
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<actix_web::web::Bytes, Self::Error>>> {
        let result = self.inner.as_mut().poll_next(cx);
        if let Poll::Ready(Some(Ok(bytes))) = &result
            && let Some(remaining) = &mut self.remaining
        {
            *remaining = remaining.saturating_sub(bytes.len() as u64);
            if *remaining == 0
                && let Some(mut guard) = self.guard.take()
            {
                guard.finish();
            }
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
}

fn request_metrics(req: &ServiceRequest) -> SharedMetricsRecorder {
    if let Some(metrics) = req.app_data::<web::Data<dyn MetricsRecorder>>() {
        return metrics.clone().into_inner();
    }

    req.app_data::<web::Data<SharedMetricsRecorder>>()
        .map_or_else(aster_forge_metrics::NoopMetrics::arc, |data| {
            data.get_ref().clone()
        })
}

fn route_label(req: &ServiceRequest) -> String {
    req.match_pattern().unwrap_or_default()
}

#[cfg(test)]
mod tests {

    #[actix_web::test]
    async fn streaming_response_is_counted_at_eof_or_drop_and_probes_are_excluded() {
        use futures::{StreamExt, stream};
        let metrics = RecordingMetrics::enabled();
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(metrics.shared()))
                .wrap(MetricsMiddleware)
                .route(
                    "/stream/{id}",
                    web::get().to(|| async {
                        HttpResponse::Ok().streaming(
                            stream::iter([Ok::<_, std::io::Error>(web::Bytes::from_static(
                                b"part",
                            ))])
                            .chain(stream::pending()),
                        )
                    }),
                )
                .route(
                    "/complete",
                    web::get().to(|| async { HttpResponse::Ok().body("done") }),
                )
                .route(
                    "/health/ready",
                    web::get().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/stream/42")
                .to_request(),
        )
        .await;
        assert!(metrics.records().is_empty());
        drop(response);
        assert_eq!(metrics.records().len(), 1);
        assert_eq!(metrics.records()[0].route, "/stream/{id}");
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get().uri("/complete").to_request(),
        )
        .await;
        assert_eq!(metrics.records().len(), 1);
        assert_eq!(actix_test::read_body(response).await, "done");
        assert_eq!(metrics.records().len(), 2);
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/health/ready")
                .to_request(),
        )
        .await;
        drop(response);
        assert_eq!(metrics.records().len(), 2);
    }

    use crate::test_support::RecordingMetrics;
    use actix_web::{App, HttpResponse, error, test as actix_test, web};
    use aster_forge_metrics::MetricsRecorder;

    use super::{MetricsMiddleware, route_label};

    #[test]
    fn unmatched_route_groups_unknown_paths() {
        let api = actix_test::TestRequest::get()
            .uri("/api/v1/missing")
            .to_srv_request();
        let health = actix_test::TestRequest::get()
            .uri("/health/full")
            .to_srv_request();
        let other = actix_test::TestRequest::get()
            .uri("/missing")
            .to_srv_request();

        assert_eq!(route_label(&api), "");
        assert_eq!(route_label(&health), "");
        assert_eq!(route_label(&other), "");
    }

    #[actix_web::test]
    async fn middleware_records_successful_requests_when_metrics_are_enabled() {
        let metrics = RecordingMetrics::enabled();
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(metrics.shared()))
                .wrap(MetricsMiddleware)
                .route(
                    "/api/v1/profiles/{id}",
                    web::get().to(|| async { HttpResponse::Created().finish() }),
                ),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/v1/profiles/42")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 201);

        let records = metrics.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].method, "GET");
        assert_eq!(records[0].route, "/api/v1/profiles/{id}");
        assert_eq!(records[0].status, 201);
        assert!(records[0].duration_seconds >= 0.0);
    }

    #[actix_web::test]
    async fn middleware_accepts_single_arc_trait_object_app_data() {
        let metrics = RecordingMetrics::enabled();
        let shared = web::Data::<dyn MetricsRecorder>::from(metrics.shared());
        let app =
            actix_test::init_service(App::new().app_data(shared).wrap(MetricsMiddleware).route(
                "/api/v1/profiles/{id}",
                web::get().to(|| async { HttpResponse::Accepted().finish() }),
            ))
            .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/v1/profiles/42")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 202);

        let records = metrics.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].route, "/api/v1/profiles/{id}");
        assert_eq!(records[0].status, 202);
    }

    #[actix_web::test]
    async fn middleware_records_error_responses_when_metrics_are_enabled() {
        let metrics = RecordingMetrics::enabled();
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(metrics.shared()))
                .wrap(MetricsMiddleware)
                .route(
                    "/api/v1/fails",
                    web::get().to(|| async {
                        Err::<HttpResponse, _>(error::ErrorBadRequest("bad request"))
                    }),
                ),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/v1/fails")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 400);
        let _body = actix_test::read_body(resp).await;

        let records = metrics.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].method, "GET");
        assert_eq!(records[0].route, "/api/v1/fails");
        assert_eq!(records[0].status, 400);
    }

    #[actix_web::test]
    async fn middleware_skips_recording_when_metrics_are_disabled() {
        let metrics = RecordingMetrics::disabled();
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(metrics.shared()))
                .wrap(MetricsMiddleware)
                .route(
                    "/health",
                    web::get().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = actix_test::TestRequest::get().uri("/health").to_request();
        assert_eq!(actix_test::call_service(&app, req).await.status(), 200);
        assert_eq!(
            metrics.records(),
            [] as [crate::test_support::HttpMetricRecord; 0]
        );
    }
}

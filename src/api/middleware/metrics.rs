//! API 中间件：`metrics`。

use actix_web::{
    Error,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    web,
};
use futures::future::{LocalBoxFuture, Ready, ok};
use std::rc::Rc;
use std::time::Instant;

use crate::metrics_core::SharedMetricsRecorder;

pub struct MetricsMiddleware;

impl<S, B> Transform<S, ServiceRequest> for MetricsMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
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

pub struct MetricsService<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for MetricsService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let svc = self.service.clone();
        let metrics = req
            .app_data::<web::Data<SharedMetricsRecorder>>()
            .map(|data| data.get_ref().clone())
            .unwrap_or_else(crate::metrics_core::NoopMetrics::arc);

        if !metrics.enabled() {
            return Box::pin(async move { svc.call(req).await });
        }

        let started_at = Instant::now();
        let method = req.method().clone();
        let route = route_label(&req);

        Box::pin(async move {
            match svc.call(req).await {
                Ok(resp) => {
                    metrics.record_http_request(
                        method.as_str(),
                        &route,
                        resp.status().as_u16(),
                        started_at.elapsed().as_secs_f64(),
                    );
                    Ok(resp)
                }
                Err(error) => {
                    metrics.record_http_request(
                        method.as_str(),
                        &route,
                        error.as_response_error().status_code().as_u16(),
                        started_at.elapsed().as_secs_f64(),
                    );
                    Err(error)
                }
            }
        })
    }
}

fn route_label(req: &ServiceRequest) -> String {
    req.match_pattern().unwrap_or_else(|| unmatched_route(req))
}

fn unmatched_route(req: &ServiceRequest) -> String {
    let path = req.path();
    if path.starts_with("/api/") {
        "unmatched_api".to_string()
    } else if path.starts_with("/health") {
        "unmatched_health".to_string()
    } else {
        "unmatched".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{MetricsMiddleware, unmatched_route};
    use crate::metrics_core::{MetricsRecorder, SharedMetricsRecorder};
    use actix_web::{App, HttpResponse, error, test as actix_test, web};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, PartialEq)]
    struct HttpMetricRecord {
        method: String,
        route: String,
        status: u16,
        duration_seconds: f64,
    }

    struct RecordingMetrics {
        enabled: bool,
        records: Mutex<Vec<HttpMetricRecord>>,
    }

    impl RecordingMetrics {
        fn enabled() -> Arc<Self> {
            Arc::new(Self {
                enabled: true,
                records: Mutex::new(Vec::new()),
            })
        }

        fn disabled() -> Arc<Self> {
            Arc::new(Self {
                enabled: false,
                records: Mutex::new(Vec::new()),
            })
        }

        fn shared(self: &Arc<Self>) -> SharedMetricsRecorder {
            self.clone()
        }

        fn records(&self) -> Vec<HttpMetricRecord> {
            self.records.lock().unwrap().clone()
        }
    }

    impl MetricsRecorder for RecordingMetrics {
        fn enabled(&self) -> bool {
            self.enabled
        }

        fn record_http_request(
            &self,
            method: &str,
            route: &str,
            status: u16,
            duration_seconds: f64,
        ) {
            self.records.lock().unwrap().push(HttpMetricRecord {
                method: method.to_string(),
                route: route.to_string(),
                status,
                duration_seconds,
            });
        }
    }

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

        assert_eq!(unmatched_route(&api), "unmatched_api");
        assert_eq!(unmatched_route(&health), "unmatched_health");
        assert_eq!(unmatched_route(&other), "unmatched");
    }

    #[actix_web::test]
    async fn middleware_records_successful_requests_when_metrics_are_enabled() {
        let metrics = RecordingMetrics::enabled();
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(metrics.shared()))
                .wrap(MetricsMiddleware)
                .route(
                    "/api/v1/examples/{id}",
                    web::get().to(|| async { HttpResponse::Created().finish() }),
                ),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/v1/examples/42")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 201);

        let records = metrics.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].method, "GET");
        assert_eq!(records[0].route, "/api/v1/examples/{id}");
        assert_eq!(records[0].status, 201);
        assert!(records[0].duration_seconds >= 0.0);
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
        assert!(metrics.records().is_empty());
    }
}

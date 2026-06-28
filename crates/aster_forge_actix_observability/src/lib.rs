//! Actix Web observability endpoints for Aster services.
//!
//! This crate owns route-level observability glue that is specific to Actix Web, such as the
//! Prometheus text exposition endpoint. Metrics recording traits and concrete backend state remain
//! in `aster_forge_metrics`; product route modules can call these helpers without carrying
//! backend-specific `#[cfg]` blocks.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

use actix_web::Scope;

/// Adds a Prometheus `/metrics` endpoint to an Actix route scope when enabled.
///
/// Without the `prometheus` feature this returns the input scope unchanged. Products should call
/// this unconditionally from their route assembly and let Cargo features decide whether the
/// endpoint exists.
pub fn configure_prometheus_route(scope: Scope) -> Scope {
    #[cfg(feature = "prometheus")]
    {
        scope.route("/metrics", actix_web::web::get().to(prometheus_metrics))
    }

    #[cfg(not(feature = "prometheus"))]
    {
        scope
    }
}

#[cfg(feature = "prometheus")]
async fn prometheus_metrics() -> actix_web::HttpResponse {
    if !aster_forge_metrics::prometheus::is_initialized() {
        tracing::debug!("metrics probe failed because metrics are not initialized");
        return actix_web::HttpResponse::ServiceUnavailable()
            .body("metrics registry is not initialized");
    }

    match aster_forge_metrics::prometheus::export_metrics() {
        Ok(body) => {
            tracing::debug!(bytes = body.len(), "metrics probe exported metrics");
            actix_web::HttpResponse::Ok()
                .content_type("text/plain; version=0.0.4; charset=utf-8")
                .body(body)
        }
        Err(error) => {
            tracing::debug!(error = %error, "metrics probe export failed");
            actix_web::HttpResponse::ServiceUnavailable().body(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use actix_web::{App, http::StatusCode, test, web};

    use super::configure_prometheus_route;

    #[cfg(not(feature = "prometheus"))]
    #[actix_web::test]
    async fn prometheus_route_is_not_registered_without_feature() {
        use actix_web::HttpResponse;

        let app = test::init_service(App::new().service(configure_prometheus_route(
            web::scope("").route("/healthz", web::get().to(HttpResponse::Ok)),
        )))
        .await;

        let health =
            test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;
        assert_eq!(health.status(), StatusCode::OK);

        let metrics =
            test::call_service(&app, test::TestRequest::get().uri("/metrics").to_request()).await;
        assert_eq!(metrics.status(), StatusCode::NOT_FOUND);
    }

    #[cfg(feature = "prometheus")]
    #[actix_web::test]
    async fn prometheus_route_exports_text_after_registry_init() {
        aster_forge_metrics::prometheus::init_metrics()
            .expect("metrics registry should initialize");
        let recorder = aster_forge_metrics::prometheus::PrometheusMetricsRecorder;
        aster_forge_metrics::MetricsRecorder::record_http_request(
            &recorder, "GET", "/healthz", 200, 0.01,
        );
        let app =
            test::init_service(App::new().service(configure_prometheus_route(web::scope(""))))
                .await;

        let response =
            test::call_service(&app, test::TestRequest::get().uri("/metrics").to_request()).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = test::read_body(response).await;
        let body = std::str::from_utf8(&body).expect("metrics body should be utf-8");
        assert!(body.contains("http_requests_total"));
        assert!(body.contains("route=\"/healthz\""));
        assert!(body.contains("process_uptime_seconds"));
    }
}

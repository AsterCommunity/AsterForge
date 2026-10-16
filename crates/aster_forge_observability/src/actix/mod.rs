//! Injectable Prometheus resource. Products own its path and authentication middleware.

#[cfg(feature = "prometheus")]
use actix_web::{HttpRequest, HttpResponse, Resource, http::header, web};

/// This resource does not read global state. Missing injection returns 503;
/// products can wrap the resource so authentication happens before this handler.
#[cfg(feature = "prometheus")]
pub fn prometheus_resource(path: &str) -> Resource {
    web::resource(path).route(web::get().to(prometheus_metrics))
}

#[cfg(feature = "prometheus")]
async fn prometheus_metrics(request: HttpRequest) -> HttpResponse {
    let Some(provider) = request.app_data::<web::Data<aster_forge_metrics::MetricsProvider>>()
    else {
        return HttpResponse::ServiceUnavailable()
            .insert_header((header::CACHE_CONTROL, "no-store"))
            .body("metrics provider is unavailable");
    };
    match provider
        .export_prometheus(std::time::Duration::from_secs(5))
        .await
    {
        Ok(body) => HttpResponse::Ok()
            .insert_header((header::CACHE_CONTROL, "no-store"))
            .content_type("text/plain; version=0.0.4; charset=utf-8")
            .body(body),
        Err(_) => HttpResponse::ServiceUnavailable()
            .insert_header((header::CACHE_CONTROL, "no-store"))
            .body("metrics export is unavailable"),
    }
}

#[cfg(all(test, feature = "prometheus"))]
mod tests {
    use super::*;
    use actix_web::{App, http::StatusCode, test};
    use aster_forge_metrics::{MetricDescriptor, MetricsProvider, ProductCounter};

    #[actix_web::test]
    async fn explicit_instance_injection_preserves_status_format_and_no_store() {
        let app = test::init_service(App::new().service(prometheus_resource("/metrics"))).await;
        let response =
            test::call_service(&app, test::TestRequest::get().uri("/metrics").to_request()).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let provider = MetricsProvider::builder()
            .with_prometheus()
            .unwrap()
            .build()
            .unwrap();
        let handle = provider
            .register_batch(&[MetricDescriptor::counter(
                "test",
                "requests_total",
                "Requests.",
                &[],
            )])
            .unwrap()
            .remove(0);
        ProductCounter::from_handle(handle).unwrap().inc(&[], 2);
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(provider.clone()))
                .service(prometheus_resource("/custom")),
        )
        .await;
        let response =
            test::call_service(&app, test::TestRequest::get().uri("/custom").to_request()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4; charset=utf-8"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert!(
            String::from_utf8(test::read_body(response).await.to_vec())
                .unwrap()
                .contains("test_requests_total 2")
        );
        provider
            .shutdown(std::time::Duration::from_secs(1))
            .await
            .unwrap();
        let response =
            test::call_service(&app, test::TestRequest::get().uri("/custom").to_request()).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

//! HTTP health route integration tests.

#[macro_use]
mod common;

use actix_web::{http::StatusCode, test};

#[actix_web::test]
async fn health_and_ready_routes_return_ok() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let health =
        test::call_service(&app, test::TestRequest::get().uri("/health").to_request()).await;
    assert_eq!(health.status(), StatusCode::OK);
    let health_body: serde_json::Value = test::read_body_json(health).await;
    assert_eq!(health_body["status"], "ok");
    assert!(health_body.get("runtime_id").is_none());
    assert!(health_body.get("cache_backend").is_none());
    assert!(health_body.get("config_sync_enabled").is_none());
    assert!(health_body.get("components").is_none());

    let ready = test::call_service(
        &app,
        test::TestRequest::get().uri("/health/ready").to_request(),
    )
    .await;
    assert_eq!(ready.status(), StatusCode::OK);
    let ready_body: serde_json::Value = test::read_body_json(ready).await;
    assert_eq!(ready_body["status"], "ready");
    assert!(ready_body.get("runtime_id").is_none());
    assert!(ready_body.get("cache_backend").is_none());
    assert!(ready_body.get("config_sync_enabled").is_none());
    assert!(ready_body.get("components").is_none());
}

#[actix_web::test]
async fn api_scope_returns_json_404_instead_of_frontend_fallback() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/missing-route")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "endpoint_not_found");
    assert_eq!(body["message"], "endpoint not found");
}

#[actix_web::test]
async fn frontend_fallback_still_serves_spa_routes() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/settings/runtime")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    );
}

#[actix_web::test]
async fn base_http_middleware_adds_request_id_and_security_headers() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let response =
        test::call_service(&app, test::TestRequest::get().uri("/health").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("x-request-id"));
    assert_eq!(
        response
            .headers()
            .get("x-frame-options")
            .and_then(|value| value.to_str().ok()),
        Some("SAMEORIGIN")
    );
    assert_eq!(
        response
            .headers()
            .get("referrer-policy")
            .and_then(|value| value.to_str().ok()),
        Some("strict-origin-when-cross-origin")
    );
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
}

#[cfg(feature = "metrics")]
#[actix_web::test]
async fn metrics_route_exports_prometheus_text() {
    let state = common::setup().await;
    let app = create_test_app!(state);

    let response = test::call_service(
        &app,
        test::TestRequest::get().uri("/health/metrics").to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = test::read_body(response).await;
    let body = std::str::from_utf8(&body).expect("metrics body should be utf-8");
    assert!(body.contains("db_queries_total"));
    assert!(body.contains("health_report_status"));
    assert!(body.contains("health_component_status"));
    assert!(body.contains("background_tasks_pending"));
    assert!(body.contains("config_reloads_total"));
    assert!(body.contains("process_uptime_seconds"));
    assert!(body.contains("process_memory_rss_bytes"));
}

//! HTTP health route integration tests.

#[macro_use]
mod common;

use actix_web::{http::StatusCode, test};

#[actix_web::test]
async fn health_and_ready_routes_return_ok() {
    let (state, _database) = common::setup().await;
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
async fn health_head_routes_return_probe_status_without_body() {
    let (state, _database) = common::setup().await;
    let app = create_test_app!(state);

    for path in ["/health", "/health/ready"] {
        let response = test::call_service(
            &app,
            test::TestRequest::default()
                .method(actix_web::http::Method::HEAD)
                .uri(path)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-request-id"));
        assert!(response.headers().contains_key("x-content-type-options"));
    }
}

#[actix_web::test]
async fn api_scope_returns_json_404_instead_of_frontend_fallback() {
    let (state, _database) = common::setup().await;
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
async fn api_scope_root_returns_json_404_instead_of_frontend_fallback() {
    let (state, _database) = common::setup().await;
    let app = create_test_app!(state);

    let response =
        test::call_service(&app, test::TestRequest::get().uri("/api/v1").to_request()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "endpoint_not_found");
}

#[actix_web::test]
async fn frontend_fallback_still_serves_spa_routes() {
    let (state, _database) = common::setup().await;
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
async fn frontend_index_sets_csp_header_and_meta_without_header_only_directives() {
    use {{crate_name}}::api::routes::frontend::{FRONTEND_CSP_HEADER, FRONTEND_CSP_META};

    let (state, _database) = common::setup().await;
    let app = create_test_app!(state);

    let response = test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok()),
        Some(FRONTEND_CSP_HEADER)
    );
    assert!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("no-cache"))
    );

    let body = test::read_body(response).await;
    let html = std::str::from_utf8(&body).expect("index html should be utf-8");
    let escaped_csp = FRONTEND_CSP_META.replace('\'', "&#39;");
    assert!(
        html.contains(&format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{escaped_csp}\" />"
        )),
        "expected index.html to include CSP meta tag"
    );
    assert!(
        !html.contains("frame-ancestors"),
        "meta CSP should not include header-only frame-ancestors directive"
    );
    assert!(html.contains(env!("CARGO_PKG_VERSION")));
}

#[actix_web::test]
async fn frontend_csp_constants_split_header_only_directives() {
    use {{crate_name}}::api::routes::frontend::{FRONTEND_CSP_HEADER, FRONTEND_CSP_META};

    assert!(
        FRONTEND_CSP_HEADER.contains("frame-ancestors 'self'"),
        "header CSP should retain frame-ancestors"
    );
    assert!(
        !FRONTEND_CSP_META.contains("frame-ancestors"),
        "meta CSP should exclude frame-ancestors"
    );
    assert!(
        FRONTEND_CSP_META.contains("connect-src 'self' http: https: ws: wss:"),
        "meta CSP should still allow browser connections"
    );
}

#[actix_web::test]
async fn frontend_assets_use_expected_content_types_and_cache_headers() {
    let (state, _database) = common::setup().await;
    let app = create_test_app!(state);

    let favicon = test::call_service(
        &app,
        test::TestRequest::get().uri("/favicon.svg").to_request(),
    )
    .await;
    assert_eq!(favicon.status(), StatusCode::OK);
    assert_eq!(
        favicon
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("image/svg+xml")
    );
    assert_eq!(
        favicon
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=86400")
    );

    let missing_pwa_file = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/manifest.webmanifest")
            .to_request(),
    )
    .await;
    assert_eq!(missing_pwa_file.status(), StatusCode::NOT_FOUND);
    let body = test::read_body(missing_pwa_file).await;
    assert_eq!(&body[..], b"File not found");
}

#[actix_web::test]
async fn frontend_asset_traversal_is_rejected() {
    let (state, _database) = common::setup().await;
    let app = create_test_app!(state);

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/assets/../index.html")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[actix_web::test]
async fn base_http_middleware_adds_request_id_and_security_headers() {
    let (state, _database) = common::setup().await;
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
    let (state, _database) = common::setup().await;
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
